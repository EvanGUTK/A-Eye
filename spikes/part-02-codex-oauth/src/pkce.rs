//! RFC 7636 PKCE (method S256) and OAuth `state` generation.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::secret::Secret;

pub struct Pkce {
    pub verifier: Secret,
    pub challenge: String,
}

impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &self.verifier)
            .field("challenge", &self.challenge)
            .finish()
    }
}

/// `BASE64URL(SHA256(ASCII(verifier)))`, no padding (RFC 7636 section 4.2).
pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// 32 random bytes -> 43-char unpadded base64url verifier (docs/architecture/OAUTH.md).
pub fn generate() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = challenge_for(&verifier);
    Pkce { verifier: Secret::new(verifier), challenge }
}

/// Random CSRF `state` (32 random bytes, base64url).
pub fn generate_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B.
    #[test]
    fn rfc7636_appendix_b_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(challenge_for(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn verifier_length_and_charset() {
        for _ in 0..50 {
            let p = generate();
            let v = p.verifier.expose();
            assert!((43..=128).contains(&v.len()), "len {}", v.len());
            assert_eq!(v.len(), 43); // 32 bytes -> 43 chars unpadded
            assert!(
                v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "non-unreserved char in {v}"
            );
            assert!(!v.contains('='));
        }
    }

    #[test]
    fn challenge_matches_verifier() {
        let p = generate();
        assert_eq!(p.challenge, challenge_for(p.verifier.expose()));
        assert_eq!(p.challenge.len(), 43); // 32-byte digest
    }

    #[test]
    fn values_are_not_repeated() {
        assert_ne!(generate().verifier, generate().verifier);
        assert_ne!(generate_state(), generate_state());
    }

    #[test]
    fn pkce_debug_redacts_verifier() {
        let p = generate();
        let out = format!("{p:?}");
        assert!(!out.contains(p.verifier.expose()));
    }
}
