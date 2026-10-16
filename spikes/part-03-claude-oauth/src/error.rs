//! Stable error type for the OAuth flow. Messages never contain codes,
//! state values, or tokens.

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum FlowError {
    /// No valid callback arrived within the configured window.
    Timeout,
    /// Callback `state` missing or different from the one we generated.
    StateMismatch,
    /// Callback had the right state but no (or empty) `code`.
    MissingCode,
    /// Provider redirected with `error=...` (sanitized code, e.g. access_denied).
    ProviderDenied(String),
    Io(String),
    /// Token endpoint returned an OAuth error (sanitized code only).
    TokenEndpoint(String),
    BadTokenResponse(String),
}

impl std::fmt::Display for FlowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlowError::Timeout => write!(f, "timed out waiting for the browser callback"),
            FlowError::StateMismatch => write!(f, "callback state missing or mismatched"),
            FlowError::MissingCode => write!(f, "callback did not include an authorization code"),
            FlowError::ProviderDenied(e) => write!(f, "provider returned error: {e}"),
            FlowError::Io(e) => write!(f, "io error: {e}"),
            FlowError::TokenEndpoint(e) => write!(f, "token endpoint error: {e}"),
            FlowError::BadTokenResponse(e) => write!(f, "unusable token response: {e}"),
        }
    }
}

impl std::error::Error for FlowError {}

impl From<std::io::Error> for FlowError {
    fn from(e: std::io::Error) -> Self {
        FlowError::Io(e.to_string())
    }
}

/// Keep provider-supplied error codes short and inert (alnum, `_`, `-`, `.`).
pub fn sanitize_code(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(64)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_markup_and_limits_length() {
        assert_eq!(sanitize_code("access_denied"), "access_denied");
        assert_eq!(sanitize_code("<script>x</script>"), "scriptxscript");
        assert_eq!(sanitize_code(&"a".repeat(200)).len(), 64);
    }
}
