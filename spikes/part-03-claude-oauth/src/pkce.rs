//! PKCE (RFC 7636) primitives: verifier, S256 challenge, and `state`.

use crate::secret::Secret;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// `n` random bytes from the OS RNG, base64url without padding.
pub fn random_b64url(n: usize) -> Result<String, String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|e| format!("OS RNG failure: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(&buf))
}

pub fn random_hex(n: usize) -> Result<String, String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|e| format!("OS RNG failure: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// S256: BASE64URL-ENCODE(SHA256(ASCII(code_verifier))), no padding.
pub fn challenge_for(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.as_slice())
}

#[derive(Debug)]
pub struct Pkce {
    pub verifier: Secret,
    pub challenge: String,
}

impl Pkce {
    /// verifier = 32 random bytes -> base64url (43 chars, within RFC 7636's 43..=128).
    pub fn generate() -> Result<Self, String> {
        let verifier = random_b64url(32)?;
        let challenge = challenge_for(&verifier);
        Ok(Pkce {
            verifier: Secret::new(verifier),
            challenge,
        })
    }
}

/// Anti-CSRF `state`: 32 random bytes, base64url.
pub fn new_state() -> Result<String, String> {
    random_b64url(32)
}

/// Length-independent-of-content comparison for state values.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 Appendix B test vector.
    #[test]
    fn rfc7636_appendix_b_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge_for(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_shape_and_uniqueness() {
        let a = Pkce::generate().unwrap();
        let b = Pkce::generate().unwrap();
        let v = a.verifier.expose();
        assert_eq!(v.len(), 43, "32 bytes -> 43 base64url chars, no padding");
        assert!(v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(v, b.verifier.expose());
        assert_eq!(a.challenge, challenge_for(v));
        assert_eq!(a.challenge.len(), 43);
    }

    #[test]
    fn state_is_random_and_compares_correctly() {
        let s1 = new_state().unwrap();
        let s2 = new_state().unwrap();
        assert_ne!(s1, s2);
        assert!(ct_eq(&s1, &s1.clone()));
        assert!(!ct_eq(&s1, &s2));
        assert!(!ct_eq(&s1, "short"));
    }
}
