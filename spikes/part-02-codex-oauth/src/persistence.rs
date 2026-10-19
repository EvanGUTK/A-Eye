//! Durable, versioned OAuth sessions over part 01's credential backend.

use std::fs::{self, File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use spike_01_credential_store::backend::{BackendError, CredentialBackend};
use zeroize::Zeroizing;

use crate::token::{self, RefreshEncoding, TokenSet};

const VERSION: u32 = 1;
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
const MAX_CREDENTIAL_BYTES: usize = 256 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    pub token_url: String,
    pub client_id: String,
    pub resource: Option<String>,
    pub encoding: RefreshEncoding,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    version: u32,
    pub registration: Registration,
    pub tokens: TokenSet,
}

impl Session {
    pub fn new(registration: Registration, tokens: TokenSet) -> Self {
        Self {
            version: VERSION,
            registration,
            tokens,
        }
    }
}

/// Opaque diagnostics: backend/JSON errors can themselves contain credential data.
#[derive(Debug)]
pub enum SessionError {
    Store(&'static str),
    InvalidRecord,
    BindingMismatch,
    Refresh(token::TokenError),
    SignInRequired,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(op) => write!(f, "credential store {op} failed; no plaintext fallback; a timed-out write may complete later"),
            Self::InvalidRecord => f.write_str("stored OAuth record is invalid or unsupported; use a new account alias to sign in"),
            Self::BindingMismatch => f.write_str("stored OAuth registration does not match this endpoint/client; select its configuration or a new account alias"),
            Self::Refresh(e) => write!(f, "stored session refresh failed: {e}; credentials retained; retry or use a new account alias to sign in"),
            Self::SignInRequired => f.write_str("stored access token needs renewal but has no refresh token; use a new account alias to sign in"),
        }
    }
}

impl std::error::Error for SessionError {}

pub struct SessionStore<'a> {
    backend: &'a dyn CredentialBackend,
    account: String,
}

impl<'a> SessionStore<'a> {
    pub fn new(backend: &'a dyn CredentialBackend, account_id: &str) -> Result<Self, String> {
        if account_id.is_empty()
            || account_id.len() > 128
            || !account_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        {
            return Err(
                "account alias must be 1..128 ASCII letters, digits, '-', '_' or '.'".into(),
            );
        }
        Ok(Self {
            backend,
            account: crate::store::account_name(account_id),
        })
    }

    pub fn save(&self, session: &Session) -> Result<(), SessionError> {
        validate(session)?;
        // Avoid serde_json::Value, which creates extra unprotected secret copies.
        let mut bytes = Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *bytes, session).map_err(|_| SessionError::InvalidRecord)?;
        if bytes.len() > MAX_CREDENTIAL_BYTES {
            return Err(SessionError::InvalidRecord);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| SessionError::InvalidRecord)?;
        self.backend
            .set(&self.account, text)
            .map_err(|_| SessionError::Store("write"))?;
        let got = self
            .backend
            .get(&self.account)
            .map_err(|_| SessionError::Store("readback"))?;
        if got.as_bytes() != bytes.as_slice() {
            return Err(SessionError::Store("readback comparison"));
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<Session>, SessionError> {
        let text = match self.backend.get(&self.account) {
            Ok(text) => text,
            Err(BackendError::NotFound) => return Ok(None),
            Err(_) => return Err(SessionError::Store("read")),
        };
        if text.len() > MAX_CREDENTIAL_BYTES {
            return Err(SessionError::InvalidRecord);
        }
        let session: Session =
            serde_json::from_str(&text).map_err(|_| SessionError::InvalidRecord)?;
        validate(&session)?;
        Ok(Some(session))
    }

    /// Call only while holding the CLI process lock, from load through readback.
    /// A missing record starts OAuth; every other failure leaves the record intact.
    pub fn resume(
        &self,
        token_url: &str,
        client_id: Option<&str>,
        resource: Option<&str>,
        encoding: RefreshEncoding,
        now: SystemTime,
    ) -> Result<Option<Session>, SessionError> {
        let Some(mut session) = self.load()? else {
            return Ok(None);
        };
        let reg = &session.registration;
        if reg.token_url != token_url
            || client_id.is_some_and(|id| id != reg.client_id)
            || reg.resource.as_deref() != resource
            || reg.encoding != encoding
        {
            return Err(SessionError::BindingMismatch);
        }
        let fresh = now.checked_add(REFRESH_MARGIN).is_some_and(|threshold| {
            session.tokens.obtained_at <= now
                && session
                    .tokens
                    .expires_at()
                    .is_some_and(|expires| expires > threshold)
        });
        if fresh {
            return Ok(Some(session));
        }
        let refresh_token = session
            .tokens
            .refresh_token
            .as_ref()
            .ok_or(SessionError::SignInRequired)?;
        let client = token::http_client().map_err(SessionError::Refresh)?;
        let mut updated = token::refresh(
            &client,
            &reg.token_url,
            &reg.client_id,
            refresh_token,
            reg.encoding,
            reg.resource.as_deref(),
        )
        .map_err(SessionError::Refresh)?;
        // RFC 6749: a replacement refresh token is optional; omitted fields retain
        // the existing grant. Never carry the old access-token expiry forward.
        updated.refresh_token = updated
            .refresh_token
            .or_else(|| session.tokens.refresh_token.take());
        updated.id_token = updated.id_token.or_else(|| session.tokens.id_token.take());
        updated.scope = updated.scope.or_else(|| session.tokens.scope.take());
        updated.token_type = updated
            .token_type
            .or_else(|| session.tokens.token_type.take());
        session.tokens = updated;
        self.save(&session)?;
        Ok(Some(session))
    }
}

fn validate(session: &Session) -> Result<(), SessionError> {
    if session.version != VERSION
        || session.registration.client_id.is_empty()
        || crate::flow::validate_endpoint(&session.registration.token_url).is_err()
        || session.tokens.access_token.expose().is_empty()
        || session
            .tokens
            .refresh_token
            .as_ref()
            .is_some_and(|s| s.expose().is_empty())
        || (session.tokens.expires_in.is_some() && session.tokens.expires_at().is_none())
    {
        return Err(SessionError::InvalidRecord);
    }
    Ok(())
}

pub fn lock_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
        .ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/a-eye/codex-oauth.lock"))
}

/// One writer across both live modes. The OS releases the lock after a crash;
/// leave the inode in place, since unlinking it would allow independent locks.
/// Rust 1.89 is required for the standard library's file-lock API.
pub fn lock(path: &Path) -> Result<File, String> {
    let parent = path.parent().ok_or("lock path has no parent")?;
    #[cfg(unix)]
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|_| "cannot create OAuth lock directory")?;
    #[cfg(not(unix))]
    fs::DirBuilder::new()
        .recursive(true)
        .create(parent)
        .map_err(|_| "cannot create OAuth lock directory")?;
    #[cfg(unix)]
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|_| "cannot open OAuth lock file")?;
    #[cfg(not(unix))]
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|_| "cannot open OAuth lock file")?;
    file.try_lock().map_err(
        |_| "another OAuth process is active (or locking is unavailable); retry after it exits",
    )?;
    Ok(file)
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
