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

/// Write -> read back -> compare -> delete -> confirm gone, for one synthetic entry.
pub fn roundtrip(tokens: &TokenSet, timeout: Duration) -> StoreReport {
    let account_id = test_account_id();
    let account = account_name(&account_id);
    let blob = credential_blob(tokens);
    let mut steps = Vec::new();

    // 1. Store reachable + entry must not pre-exist.
    let acct = account.clone();
    let pre = with_timeout(timeout, move || {
        keyring::Entry::store_status().as_ref().map_err(|e| format!("store init failed: {e}"))?;
        let entry = keyring::Entry::new(SERVICE, &acct).map_err(kr)?;
        match entry.get_password() {
            Err(keyring::Error::NoEntry) => Ok(()),
            Ok(_) => Err("test entry unexpectedly pre-exists; refusing to touch it".to_string()),
            Err(e) => Err(format!("probe failed: {e}")),
        }
    });
    match pre {
        Ok(()) => steps.push(format!("Secret Service reachable; fresh entry {SERVICE}/{account} absent")),
        Err(e) => return StoreReport::Skipped { reason: format!("store unusable before write: {e}") },
    }

    // 2. Write.
    let (acct, b) = (account.clone(), blob.clone());
    let wrote = with_timeout(timeout, move || {
        let entry = keyring::Entry::new(SERVICE, &acct).map_err(kr)?;
        entry.set_password(b.expose()).map_err(kr)
    });
    if let Err(e) = wrote {
        // Best-effort cleanup in case the write landed after the timeout.
        let _ = cleanup(&account, timeout);
        return StoreReport::Skipped { reason: format!("write failed: {e}") };
    }
    steps.push(format!("wrote {} byte credential blob (synthetic tokens)", blob.expose().len()));

    // 3. Read back and compare.
    let acct = account.clone();
    let read = with_timeout(timeout, move || {
        keyring::Entry::new(SERVICE, &acct).map_err(kr)?.get_password().map_err(kr)
    });
    let mut failure = None;
    match read {
        Ok(got) if got == blob.expose() => {
            steps.push("read back: byte-identical to what was written".into())
        }
        Ok(_) => failure = Some("read back differs from written value".to_string()),
        Err(e) => failure = Some(format!("read back failed: {e}")),
    }

    // 4. Always delete, then confirm gone.
    match cleanup(&account, timeout) {
        Ok(()) => steps.push("deleted test entry; confirmed absent".into()),
        Err(e) => {
            return StoreReport::Failed {
                reason: format!("cleanup failed, test entry {SERVICE}/{account} may remain: {e}"),
                steps,
            };
        }
    }

    match failure {
        None => StoreReport::Verified { account, steps },
        Some(reason) => StoreReport::Failed { reason, steps },
    }
}

fn cleanup(account: &str, timeout: Duration) -> Result<(), String> {
    let acct = account.to_string();
    with_timeout(timeout, move || {
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
}
