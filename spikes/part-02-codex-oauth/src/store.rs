//! Token storage in the OS credential store via the `keyring` crate
//! (Secret Service on Linux). service = "a-eye", account = "codex:<account_id>".
//!
//! Safety rules for this spike:
//! * only SYNTHETIC tokens are ever written (the caller passes mock tokens);
//! * the account id carries a random suffix and the entry must NOT already
//!   exist, so a pre-existing item can never be read, overwritten or deleted;
//! * every keyring call has a hard timeout (a locked login keyring can block on
//!   a GUI unlock prompt);
//! * no plaintext file fallback exists here.

use std::sync::mpsc;
use std::time::Duration;

use rand::Rng;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::secret::Secret;
use crate::token::TokenSet;

pub const SERVICE: &str = "a-eye";
pub const KEYRING_TIMEOUT: Duration = Duration::from_secs(15);

pub fn account_name(account_id: &str) -> String {
    format!("codex:{account_id}")
}

/// Fresh, collision-proof account id for a spike test entry.
pub fn test_account_id() -> String {
    let mut b = [0u8; 6];
    rand::rng().fill_bytes(&mut b);
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("spike02-test-{hex}")
}

pub fn iso8601(t: std::time::SystemTime) -> Option<String> {
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    OffsetDateTime::from_unix_timestamp(secs as i64).ok()?.format(&Rfc3339).ok()
}

/// Credential blob written to the keyring (OAUTH.md "Stored attributes").
pub fn credential_blob(t: &TokenSet) -> Secret {
    let expires_at = t.expires_at().and_then(iso8601);
    let v = serde_json::json!({
        "access_token": t.access_token.expose(),
        "refresh_token": t.refresh_token.as_ref().map(Secret::expose),
        "id_token": t.id_token.as_ref().map(Secret::expose),
        "expires_at": expires_at,
    });
    Secret::new(v.to_string())
}

/// Run `f` on its own thread and give up waiting after `timeout`. A timed-out
/// thread is abandoned (it may still complete later; callers report that).
pub fn with_timeout<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err(format!("timed out after {}s (keyring locked or unresponsive?)", timeout.as_secs()))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("keyring worker thread panicked".into()),
    }
}

#[derive(Debug)]
pub enum StoreReport {
    /// Full cycle ran. `detail` lines are factual observations.
    Verified { account: String, steps: Vec<String> },
    /// Could not exercise the store; nothing is claimed.
    Skipped { reason: String },
    /// The store was reachable but misbehaved (e.g. readback mismatch).
    Failed { reason: String, steps: Vec<String> },
}

fn kr(e: keyring::Error) -> String {
    // keyring errors do not carry secret values.
    e.to_string()
}

/// The four keyring operations the round trip needs. A seam so failure paths
/// (write fails, cleanup fails) can be exercised without a real keyring.
trait Backend {
    /// Store reachable and `account` absent.
    fn probe(&self, account: &str) -> Result<(), String>;
    fn write(&self, account: &str, blob: &Secret) -> Result<(), String>;
    fn read(&self, account: &str) -> Result<String, String>;
    /// Delete, then confirm absent.
    fn cleanup(&self, account: &str) -> Result<(), String>;
}

struct KeyringBackend {
    timeout: Duration,
}

impl Backend for KeyringBackend {
    fn probe(&self, account: &str) -> Result<(), String> {
        let acct = account.to_string();
        with_timeout(self.timeout, move || {
            keyring::Entry::store_status().as_ref().map_err(|e| format!("store init failed: {e}"))?;
            let entry = keyring::Entry::new(SERVICE, &acct).map_err(kr)?;
            match entry.get_password() {
                Err(keyring::Error::NoEntry) => Ok(()),
                Ok(_) => Err("test entry unexpectedly pre-exists; refusing to touch it".to_string()),
                Err(e) => Err(format!("probe failed: {e}")),
            }
        })
    }

    fn write(&self, account: &str, blob: &Secret) -> Result<(), String> {
        let (acct, b) = (account.to_string(), blob.clone());
        with_timeout(self.timeout, move || {
            let entry = keyring::Entry::new(SERVICE, &acct).map_err(kr)?;
            entry.set_password(b.expose()).map_err(kr)
        })
    }

    fn read(&self, account: &str) -> Result<String, String> {
        let acct = account.to_string();
        with_timeout(self.timeout, move || {
            keyring::Entry::new(SERVICE, &acct).map_err(kr)?.get_password().map_err(kr)
        })
    }

    fn cleanup(&self, account: &str) -> Result<(), String> {
        let acct = account.to_string();
        with_timeout(self.timeout, move || {
            let entry = keyring::Entry::new(SERVICE, &acct).map_err(kr)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(format!("delete failed: {e}")),
            }
            match entry.get_password() {
                Err(keyring::Error::NoEntry) => Ok(()),
                Ok(_) => Err("entry still present after delete".to_string()),
                Err(e) => Err(format!("post-delete probe failed: {e}")),
            }
        })
    }
}

/// Text appended to a failure reason when a test entry may still be in the keyring.
fn leftover_note(account: &str, cleanup_err: &str) -> String {
    format!(
        "test entry {SERVICE}/{account} may remain ({cleanup_err}); remove it with: \
         secret-tool clear service {SERVICE} username {account}"
    )
}

/// Write -> read back -> compare -> delete -> confirm gone, for one synthetic entry.
pub fn roundtrip(tokens: &TokenSet, timeout: Duration) -> StoreReport {
    roundtrip_with(&KeyringBackend { timeout }, tokens)
}

fn roundtrip_with(backend: &dyn Backend, tokens: &TokenSet) -> StoreReport {
    let account_id = test_account_id();
    let account = account_name(&account_id);
    let blob = credential_blob(tokens);
    let mut steps = Vec::new();

    // 1. Store reachable + entry must not pre-exist. Nothing was written yet, so
    //    failing here claims nothing about the store: Skipped.
    match backend.probe(&account) {
        Ok(()) => steps.push(format!("Secret Service reachable; fresh entry {SERVICE}/{account} absent")),
        Err(e) => return StoreReport::Skipped { reason: format!("store unusable before write: {e}") },
    }

    // 2. Write. The store answered the probe, so a failure here is a misbehaving
    //    store (Failed), and a timed-out write may still land later, so always
    //    attempt cleanup and report it if that fails too.
    if let Err(e) = backend.write(&account, &blob) {
        let mut reason = format!("write failed: {e}");
        match backend.cleanup(&account) {
            Ok(()) => steps.push("cleanup after failed write: entry confirmed absent".into()),
            Err(ce) => reason = format!("{reason}; {}", leftover_note(&account, &ce)),
        }
        return StoreReport::Failed { reason, steps };
    }
    steps.push(format!("wrote {} byte credential blob (synthetic tokens)", blob.expose().len()));

    // 3. Read back and compare.
    let mut failure = None;
    match backend.read(&account) {
        Ok(got) if got == blob.expose() => {
            steps.push("read back: byte-identical to what was written".into())
        }
        Ok(_) => failure = Some("read back differs from written value".to_string()),
        Err(e) => failure = Some(format!("read back failed: {e}")),
    }

    // 4. Always delete, then confirm gone.
    match backend.cleanup(&account) {
        Ok(()) => steps.push("deleted test entry; confirmed absent".into()),
        Err(e) => {
            let mut reason = leftover_note(&account, &e);
            if let Some(f) = failure {
                reason = format!("{f}; {reason}");
            }
            return StoreReport::Failed { reason, steps };
        }
    }

    match failure {
        None => StoreReport::Verified { account, steps },
        Some(reason) => StoreReport::Failed { reason, steps },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::parse_token_response;
    use std::time::SystemTime;

    #[test]
    fn account_format_matches_docs() {
        assert_eq!(account_name("abc"), "codex:abc");
        let id = test_account_id();
        assert!(id.starts_with("spike02-test-") && id.len() == "spike02-test-".len() + 12);
        assert_ne!(test_account_id(), test_account_id());
    }

    #[test]
    fn blob_has_expected_fields_and_iso8601_expiry() {
        let t = parse_token_response(
            br#"{"access_token":"AT","refresh_token":"RT","id_token":"ID","expires_in":3600}"#,
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(credential_blob(&t).expose()).unwrap();
        assert_eq!(v["access_token"], "AT");
        assert_eq!(v["refresh_token"], "RT");
        assert_eq!(v["id_token"], "ID");
        assert_eq!(v["expires_at"], "1970-01-01T01:00:00Z");
    }

    #[test]
    fn blob_debug_is_redacted() {
        let t = parse_token_response(br#"{"access_token":"AT-SECRET"}"#, SystemTime::UNIX_EPOCH).unwrap();
        assert!(!format!("{:?}", credential_blob(&t)).contains("AT-SECRET"));
    }

    #[test]
    fn with_timeout_returns_err_when_call_hangs() {
        let started = std::time::Instant::now();
        let r: Result<(), String> = with_timeout(Duration::from_millis(100), || {
            std::thread::sleep(Duration::from_secs(5));
            Ok(())
        });
        assert!(r.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn with_timeout_passes_through_result_and_panics_are_errors() {
        assert_eq!(with_timeout(Duration::from_secs(2), || Ok(7)), Ok(7));
        let r: Result<(), String> = with_timeout(Duration::from_secs(2), || panic!("boom"));
        assert!(r.unwrap_err().contains("panicked"));
    }

    struct Fake {
        write: Result<(), String>,
        read: Result<String, String>,
        cleanup: Result<(), String>,
        calls: std::cell::RefCell<Vec<&'static str>>,
    }

    impl Fake {
        fn new(write: Result<(), String>, cleanup: Result<(), String>) -> Self {
            Fake { write, read: Ok(String::new()), cleanup, calls: Default::default() }
        }
    }

    impl Backend for Fake {
        fn probe(&self, _: &str) -> Result<(), String> {
            self.calls.borrow_mut().push("probe");
            Ok(())
        }
        fn write(&self, _: &str, _: &Secret) -> Result<(), String> {
            self.calls.borrow_mut().push("write");
            self.write.clone()
        }
        fn read(&self, _: &str) -> Result<String, String> {
            self.calls.borrow_mut().push("read");
            self.read.clone()
        }
        fn cleanup(&self, _: &str) -> Result<(), String> {
            self.calls.borrow_mut().push("cleanup");
            self.cleanup.clone()
        }
    }

    fn toks() -> TokenSet {
        parse_token_response(br#"{"access_token":"AT","refresh_token":"RT"}"#, SystemTime::UNIX_EPOCH).unwrap()
    }

    #[test]
    fn write_failure_with_cleanup_failure_is_failed_and_names_leftover() {
        let f = Fake::new(Err("write timed out".into()), Err("delete failed: locked".into()));
        match roundtrip_with(&f, &toks()) {
            StoreReport::Failed { reason, steps } => {
                assert!(reason.contains("write failed: write timed out"), "{reason}");
                assert!(reason.contains("may remain") && reason.contains("delete failed: locked"), "{reason}");
                assert!(reason.contains("secret-tool clear service a-eye username codex:spike02-test-"), "{reason}");
                assert_eq!(steps.len(), 1, "probe step must be preserved: {steps:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(*f.calls.borrow(), ["probe", "write", "cleanup"]);
    }

    #[test]
    fn write_failure_with_clean_cleanup_is_failed_without_leftover_wording() {
        let f = Fake::new(Err("denied".into()), Ok(()));
        match roundtrip_with(&f, &toks()) {
            StoreReport::Failed { reason, steps } => {
                assert!(reason.contains("write failed: denied"), "{reason}");
                assert!(!reason.contains("may remain") && !reason.contains("secret-tool"), "{reason}");
                assert!(steps.iter().any(|s| s.contains("confirmed absent")), "{steps:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn readback_mismatch_and_cleanup_failure_report_both() {
        let mut f = Fake::new(Ok(()), Err("still present".into()));
        f.read = Ok("something else".into());
        match roundtrip_with(&f, &toks()) {
            StoreReport::Failed { reason, .. } => {
                assert!(reason.contains("read back differs") && reason.contains("may remain"), "{reason}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn probe_failure_stays_skipped_and_writes_nothing() {
        struct Dead;
        impl Backend for Dead {
            fn probe(&self, _: &str) -> Result<(), String> {
                Err("no Secret Service".into())
            }
            fn write(&self, _: &str, _: &Secret) -> Result<(), String> {
                panic!("must not write when the store is unusable")
            }
            fn read(&self, _: &str) -> Result<String, String> {
                unreachable!()
            }
            fn cleanup(&self, _: &str) -> Result<(), String> {
                unreachable!()
            }
        }
        assert!(matches!(roundtrip_with(&Dead, &toks()), StoreReport::Skipped { .. }));
    }
}
