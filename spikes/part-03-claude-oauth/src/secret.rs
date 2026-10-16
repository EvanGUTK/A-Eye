//! Redacting wrapper for secret strings (tokens, codes, PKCE verifiers).
//!
//! `Debug` never prints the value and there is deliberately no `Display`.
//! The only way to read the value is `expose()`, which makes every use
//! greppable. Best-effort zeroization on drop is NOT implemented in this spike
//! (no `zeroize` dependency); it is listed as an open item in the record.

#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// Explicit access to the raw value. Callers must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret([REDACTED len={}])", self.0.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_is_redacted() {
        let s = Secret::new("SYNTH-super-secret-value-123");
        let rendered = format!("{s:?} / {s:#?}");
        assert!(!rendered.contains("super-secret"), "leaked: {rendered}");
        assert!(rendered.contains("REDACTED"));
    }

    #[test]
    fn nested_debug_is_redacted() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Holder {
            token: Secret,
            other: Option<Secret>,
        }
        let h = Holder {
            token: Secret::new("SYNTH-AAA"),
            other: Some(Secret::new("SYNTH-BBB")),
        };
        let rendered = format!("{h:?} {h:#?}");
        assert!(!rendered.contains("SYNTH-AAA") && !rendered.contains("SYNTH-BBB"));
    }
}
