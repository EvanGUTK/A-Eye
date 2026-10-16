//! A string wrapper that never prints its contents.
//!
//! Every credential-like value in this spike (PKCE verifier, authorization
//! code, access/refresh/id tokens) is held in a [`Secret`]. `Debug` redacts,
//! there is deliberately no `Display`, and the only way to read the value is
//! the explicit [`Secret::expose`] call, which is grep-able in review.

use sha2::{Digest, Sha256};

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// Explicit access to the raw value. Use only at the point of transmission
    /// (HTTP body, keyring write); never format the result into a log line.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Short non-reversible tag (first 8 hex chars of SHA-256) so a human can
    /// tell two secrets apart without any bytes of the secret itself being
    /// printed. Deliberately NOT a prefix of the token (a JWT prefix is the
    /// constant header and tells nothing; an opaque token prefix is a leak).
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.0.as_bytes());
        digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_value() {
        let s = Secret::new("super-sensitive-token-value");
        let out = format!("{s:?}");
        assert!(!out.contains("super-sensitive"), "leaked: {out}");
        assert!(out.contains("REDACTED"));
        // pretty and nested formatting too
        let nested = format!("{:#?}", Some(vec![s.clone()]));
        assert!(!nested.contains("super-sensitive"), "leaked: {nested}");
    }

    #[test]
    fn fingerprint_is_not_a_prefix() {
        let s = Secret::new("abcdefghijklmnop");
        let fp = s.fingerprint();
        assert_eq!(fp.len(), 8);
        assert!(!"abcdefghijklmnop".contains(&fp));
    }
}
