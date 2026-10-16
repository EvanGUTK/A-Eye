//! Backend abstraction, Secret Service backend (issue #4), a hang-proof timeout
//! wrapper, and backend selection (issues #5/#6).

#[cfg(test)]
use std::collections::HashMap;
use std::fmt;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use zeroize::Zeroizing;

/// Upper bound for any single Secret Service call (a locked login keyring can
/// block on a GUI unlock prompt indefinitely).
pub const SS_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq, Eq)]
pub enum BackendError {
    /// The call did not finish within the timeout (typically: locked keyring waiting on a GUI prompt).
    Timeout,
    /// The store/daemon cannot be reached or refused access.
    Unavailable(String),
    NotFound,
    Other(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendError::Timeout => write!(f, "timed out"),
            BackendError::Unavailable(e) => write!(f, "unavailable: {e}"),
            BackendError::NotFound => write!(f, "not found"),
            BackendError::Other(e) => write!(f, "{e}"),
        }
    }
}

pub trait CredentialBackend {
    fn name(&self) -> &'static str;
    fn set(&self, account: &str, secret: &str) -> Result<(), BackendError>;
    fn get(&self, account: &str) -> Result<Zeroizing<String>, BackendError>;
    fn delete(&self, account: &str) -> Result<(), BackendError>;
}

/// Run `f` on a helper thread and give up after `timeout`. The helper thread is
/// detached on timeout (it may finish later); the caller is never blocked longer
/// than `timeout`.
pub fn run_with_timeout<T, F>(timeout: Duration, f: F) -> Result<T, BackendError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("aeye-timeout-worker".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| BackendError::Other(format!("thread spawn: {e}")))?;
    rx.recv_timeout(timeout).map_err(|_| BackendError::Timeout)
}

fn map_keyring_err(e: keyring::Error) -> BackendError {
    // Display only: `Debug` on some variants can carry raw secret bytes.
    match e {
        keyring::Error::NoEntry => BackendError::NotFound,
        keyring::Error::NoDefaultStore
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::PlatformFailure(_) => BackendError::Unavailable(e.to_string()),
        other => BackendError::Other(other.to_string()),
    }
}

/// The OS credential store through the `keyring` crate (Secret Service on Linux).
pub struct SecretServiceBackend {
    pub service: &'static str,
    pub timeout: Duration,
}

impl SecretServiceBackend {
    pub const SERVICE: &'static str = "a-eye";

    pub fn new(timeout: Duration) -> Self {
        SecretServiceBackend {
            service: Self::SERVICE,
            timeout,
        }
    }

    /// Initialise the platform store (connects to the session bus and opens a
    /// Secret Service session). Does not touch any item.
    pub fn store_init(&self) -> Result<(), BackendError> {
        run_with_timeout(self.timeout, || match keyring::Entry::store_status() {
            Ok(()) => Ok(()),
            Err(e) => Err(map_keyring_err_ref(e)),
        })?
    }
}

fn map_keyring_err_ref(e: &keyring::Error) -> BackendError {
    match e {
        keyring::Error::NoDefaultStore
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::PlatformFailure(_) => BackendError::Unavailable(e.to_string()),
        other => BackendError::Other(other.to_string()),
    }
}

impl CredentialBackend for SecretServiceBackend {
    fn name(&self) -> &'static str {
        "secret-service (keyring crate -> org.freedesktop.secrets)"
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), BackendError> {
        let (service, account) = (self.service, account.to_string());
        let secret = Zeroizing::new(secret.to_string());
        run_with_timeout(self.timeout, move || {
            keyring::Entry::new(service, &account)
                .and_then(|e| e.set_password(&secret))
                .map_err(map_keyring_err)
        })?
    }

    fn get(&self, account: &str) -> Result<Zeroizing<String>, BackendError> {
        let (service, account) = (self.service, account.to_string());
        run_with_timeout(self.timeout, move || {
            keyring::Entry::new(service, &account)
                .and_then(|e| e.get_password())
                .map(Zeroizing::new)
                .map_err(map_keyring_err)
        })?
    }

    fn delete(&self, account: &str) -> Result<(), BackendError> {
        let (service, account) = (self.service, account.to_string());
        run_with_timeout(self.timeout, move || {
            keyring::Entry::new(service, &account)
                .and_then(|e| e.delete_credential())
                .map_err(map_keyring_err)
        })?
    }
}

#[cfg(test)]
/// In-memory stand-in for the Secret Service path, used by unit tests (the
/// "mock" required by issue #5). `available = false` simulates a system with no
/// keyring daemon.
pub struct MockSecretService {
    available: bool,
    fail_get: bool,
    fail_delete: bool,
    items: Mutex<HashMap<String, Zeroizing<String>>>,
}

#[cfg(test)]
impl MockSecretService {
    pub fn new(available: bool) -> Self {
        MockSecretService {
            available,
            fail_get: false,
            fail_delete: false,
            items: Mutex::new(HashMap::new()),
        }
    }

    /// Simulate a keyring whose reads time out and/or whose deletes fail.
    pub fn with_faults(fail_get: bool, fail_delete: bool) -> Self {
        MockSecretService {
            fail_get,
            fail_delete,
            ..Self::new(true)
        }
    }

    pub fn contains(&self, account: &str) -> bool {
        self.items.lock().expect("mock lock").contains_key(account)
    }

    fn check(&self) -> Result<(), BackendError> {
        if self.available {
            Ok(())
        } else {
            Err(BackendError::Unavailable(
                "mock: no org.freedesktop.secrets owner".into(),
            ))
        }
    }
}

#[cfg(test)]
impl CredentialBackend for MockSecretService {
    fn name(&self) -> &'static str {
        "mock-secret-service"
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), BackendError> {
        self.check()?;
        self.items
            .lock()
            .expect("mock lock")
            .insert(account.into(), Zeroizing::new(secret.into()));
        Ok(())
    }

    fn get(&self, account: &str) -> Result<Zeroizing<String>, BackendError> {
        self.check()?;
        if self.fail_get {
            return Err(BackendError::Timeout);
        }
        self.items
            .lock()
            .expect("mock lock")
            .get(account)
            .cloned()
            .ok_or(BackendError::NotFound)
    }

    fn delete(&self, account: &str) -> Result<(), BackendError> {
        self.check()?;
        if self.fail_delete {
            return Err(BackendError::Timeout);
        }
        self.items
            .lock()
            .expect("mock lock")
            .remove(account)
            .map(|_| ())
            .ok_or(BackendError::NotFound)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePath {
    SecretService,
    EncryptedFile,
}

/// Pure selection rule: use the OS keyring only if a full write/read round trip
/// worked and nothing forced the fallback.
pub fn select_active(secret_service_usable: bool, force_file: bool) -> ActivePath {
    if secret_service_usable && !force_file {
        ActivePath::SecretService
    } else {
        ActivePath::EncryptedFile
    }
}

/// Why a round trip failed, and whether it may have left an entry in the store.
#[derive(Debug, PartialEq, Eq)]
pub struct RoundtripFailure {
    pub err: BackendError,
    /// The write call returned success.
    pub wrote: bool,
    /// After best-effort cleanup the entry could still exist (or a timed-out write
    /// might still complete later). The caller must report this, never swallow it.
    pub entry_may_remain: bool,
}

/// Best-effort delete; returns true if the entry might still exist afterwards.
fn cleanup_may_leave(backend: &dyn CredentialBackend, account: &str) -> bool {
    !matches!(
        backend.delete(account),
        Ok(()) | Err(BackendError::NotFound)
    )
}

/// Write -> read -> compare -> delete -> confirm gone. Returns Ok(()) only if
/// every step behaved. After any failure following a successful write it attempts
/// to delete the entry and reports whether it might remain. Never exposes the secret.
pub fn roundtrip(
    backend: &dyn CredentialBackend,
    account: &str,
    secret: &str,
) -> Result<(), RoundtripFailure> {
    if let Err(err) = backend.set(account, secret) {
        // A timed-out write may still complete later (e.g. after a GUI unlock).
        let entry_may_remain = err == BackendError::Timeout;
        return Err(RoundtripFailure {
            err,
            wrote: false,
            entry_may_remain,
        });
    }
    let fail = |err: BackendError, entry_may_remain: bool| RoundtripFailure {
        err,
        wrote: true,
        entry_may_remain,
    };
    let got = match backend.get(account) {
        Ok(got) => got,
        Err(err) => return Err(fail(err, cleanup_may_leave(backend, account))),
    };
    if got.as_str() != secret {
        let remain = cleanup_may_leave(backend, account);
        return Err(fail(
            BackendError::Other("read-back mismatch".into()),
            remain,
        ));
    }
    if let Err(err) = backend.delete(account) {
        let remain = !matches!(backend.get(account), Err(BackendError::NotFound));
        return Err(fail(err, remain));
    }
    match backend.get(account) {
        Err(BackendError::NotFound) => Ok(()),
        Ok(_) => Err(fail(
            BackendError::Other("entry still present after delete".into()),
            true,
        )),
        Err(e) => Err(fail(e, true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filestore::{FileStore, KdfParams};
    use std::time::Instant;

    #[test]
    fn timeout_wrapper_never_blocks_past_deadline() {
        let t = Instant::now();
        let r = run_with_timeout(Duration::from_millis(100), || {
            thread::sleep(Duration::from_secs(30));
            1
        });
        assert_eq!(r.unwrap_err(), BackendError::Timeout);
        assert!(t.elapsed() < Duration::from_secs(5));
        assert_eq!(run_with_timeout(Duration::from_secs(5), || 7).unwrap(), 7);
    }

    #[test]
    fn mock_keyring_path_roundtrip() {
        let m = MockSecretService::new(true);
        roundtrip(&m, "test-provider:mock", "AEYE-SPIKE-CANARY-mock").unwrap();
    }

    #[test]
    fn selection_prefers_keyring_when_usable() {
        assert_eq!(select_active(true, false), ActivePath::SecretService);
    }

    #[test]
    fn selection_falls_back_when_daemon_absent_or_forced() {
        assert_eq!(select_active(false, false), ActivePath::EncryptedFile);
        assert_eq!(select_active(true, true), ActivePath::EncryptedFile);
    }

    #[test]
    fn no_daemon_mock_triggers_file_path_and_file_path_works() {
        let mock = MockSecretService::new(false);
        let usable = roundtrip(&mock, "test-provider:probe", "AEYE-SPIKE-CANARY-probe").is_ok();
        assert!(!usable);
        let active = select_active(usable, false);
        assert_eq!(active, ActivePath::EncryptedFile);

        let dir = std::env::temp_dir().join(format!("aeye-spike01-sel-{}", std::process::id()));
        let file = FileStore::new(dir.join("credentials.enc"), "pw", KdfParams::FAST_TEST);
        roundtrip(&file, "test-provider:sel", "AEYE-SPIKE-CANARY-sel").unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn roundtrip_cleans_up_when_read_fails_after_write() {
        let m = MockSecretService::with_faults(true, false);
        let f = roundtrip(&m, "test-provider:r", "AEYE-SPIKE-CANARY-r").unwrap_err();
        assert!(f.wrote);
        assert_eq!(f.err, BackendError::Timeout);
        assert!(
            !f.entry_may_remain,
            "delete succeeded, nothing should remain"
        );
        assert!(!m.contains("test-provider:r"));
    }

    #[test]
    fn roundtrip_reports_leftover_entry_when_cleanup_also_fails() {
        let m = MockSecretService::with_faults(true, true);
        let f = roundtrip(&m, "test-provider:r", "AEYE-SPIKE-CANARY-r").unwrap_err();
        assert!(f.wrote && f.entry_may_remain);
        assert!(m.contains("test-provider:r"));
    }

    #[test]
    fn roundtrip_with_no_daemon_writes_nothing() {
        let f = roundtrip(
            &MockSecretService::new(false),
            "test-provider:n",
            "AEYE-SPIKE-CANARY-n",
        )
        .unwrap_err();
        assert!(!f.wrote && !f.entry_may_remain);
    }

    /// Active-backend cycle against whatever this machine really has. Uses only
    /// a synthetic canary. A locked/absent keyring degrades to the file path, but a
    /// keyring entry that might be left behind FAILS the test loudly.
    #[test]
    fn live_active_backend_cycle() {
        let ss = SecretServiceBackend::new(SS_TIMEOUT);
        let account = format!("test-provider:cargo-test-{}", std::process::id());
        let canary = format!("AEYE-SPIKE-CANARY-test-{}", std::process::id());
        let mut usable = false;
        if ss.store_init().is_ok() {
            match roundtrip(&ss, &account, &canary) {
                Ok(()) => usable = true,
                Err(f) => {
                    assert!(
                        !f.entry_may_remain,
                        "keyring entry may be left behind after {:?} (wrote={}); remove with: \
                         secret-tool clear service a-eye username {account}",
                        f.err, f.wrote
                    );
                    eprintln!(
                        "UNVERIFIED: keyring round trip failed ({}); entry cleaned up; using file path",
                        f.err
                    );
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("aeye-spike01-live-{}", std::process::id()));
        let file = FileStore::new(dir.join("credentials.enc"), "pw", KdfParams::FAST_TEST);
        match select_active(usable, false) {
            ActivePath::SecretService => {
                // Already proved by `roundtrip` above; confirm cleanup.
                assert_eq!(ss.get(&account).unwrap_err(), BackendError::NotFound);
            }
            ActivePath::EncryptedFile => roundtrip(&file, &account, &canary).unwrap(),
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
