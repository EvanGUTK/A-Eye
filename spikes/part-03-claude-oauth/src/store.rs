//! OS credential store via `keyring` (Secret Service on Linux).
//!
//! service = "a-eye", account = "claude:<account_id>". Every call is bounded
//! by a timeout because a locked login keyring can block on an unlock prompt.
//! There is NO plaintext disk fallback: if the store is unusable the caller
//! reports UNVERIFIED.

use crate::secret::Secret;
use crate::token::TokenSet;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub const SERVICE: &str = "a-eye";
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);

pub fn account_name(account_id: &str) -> String {
    format!("claude:{account_id}")
}

/// Run `f` on a helper thread; give up after `timeout`. A hung thread is
/// abandoned (it dies with the process).
pub fn with_timeout<T: Send + 'static>(
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(timeout)
        .map_err(|_| format!("timed out after {}s (keyring locked or unresponsive?)", timeout.as_secs()))
}

fn entry(account_id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &account_name(account_id)).map_err(|e| e.to_string())
}

fn serialize(t: &TokenSet) -> String {
    serde_json::json!({
        "access_token": t.access_token.expose(),
        "refresh_token": t.refresh_token.as_ref().map(Secret::expose),
        "expires_at_unix": t.expires_at_unix,
    })
    .to_string()
}

pub fn store_tokens(account_id: &str, t: &TokenSet) -> Result<(), String> {
    let (id, blob) = (account_id.to_string(), serialize(t));
    with_timeout(CALL_TIMEOUT, move || {
        entry(&id)?.set_password(&blob).map_err(|e| e.to_string())
    })?
}

pub fn load_access_token(account_id: &str) -> Result<Secret, String> {
    let id = account_id.to_string();
    let blob = with_timeout(CALL_TIMEOUT, move || {
        entry(&id)?.get_password().map_err(|e| e.to_string())
    })??;
    let v: serde_json::Value = serde_json::from_str(&blob).map_err(|_| "stored blob not JSON".to_string())?;
    v.get("access_token")
        .and_then(|a| a.as_str())
        .map(Secret::new)
        .ok_or_else(|| "stored blob has no access_token".to_string())
}

pub fn entry_exists(account_id: &str) -> Result<bool, String> {
    let id = account_id.to_string();
    with_timeout(CALL_TIMEOUT, move || match entry(&id)?.get_password() {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(e.to_string()),
    })?
}

pub fn delete(account_id: &str) -> Result<(), String> {
    let id = account_id.to_string();
    with_timeout(CALL_TIMEOUT, move || match entry(&id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    })?
}

/// Result of a store -> load -> delete round trip with a unique synthetic account.
#[derive(Debug)]
pub struct Roundtrip {
    pub account_id: String,
    pub loaded: Option<Secret>,
    pub matched: bool,
    pub deleted_and_gone: bool,
}

/// Always attempts deletion of the entry it created (and only that entry).
/// `keep_loaded` returns the loaded token so the caller can chain the handoff test.
pub fn roundtrip(tokens: &TokenSet) -> Result<Roundtrip, String> {
    let account_id = format!("spike03-test-{}", crate::pkce::random_hex(8)?);
    store_tokens(&account_id, tokens)?;
    let loaded = load_access_token(&account_id);
    let cleanup = delete(&account_id);
    let gone = matches!(entry_exists(&account_id), Ok(false));
    let loaded = loaded?;
    cleanup?;
    let matched = loaded.expose() == tokens.access_token.expose();
    Ok(Roundtrip {
        account_id,
        loaded: Some(loaded),
        matched,
        deleted_and_gone: gone,
    })
}

/// Cheap availability probe (does not create an entry).
pub fn store_status() -> Result<(), String> {
    with_timeout(CALL_TIMEOUT, || match keyring::Entry::store_status() {
        Ok(()) => Ok(()),
        Err(e) => Err(e.to_string()),
    })?
}
