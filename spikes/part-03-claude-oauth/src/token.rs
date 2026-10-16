//! Form-encoded authorization-code -> token exchange and response parsing.

use crate::error::{sanitize_code, FlowError};
use crate::secret::Secret;
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct TokenSet {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    pub expires_in: Option<u64>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    /// Unix seconds when we parsed the response + expires_in (OAUTH.md wants ISO 8601;
    /// kept numeric here to avoid a date dependency in the spike).
    pub expires_at_unix: Option<u64>,
}

pub fn parse_token_response(body: &str) -> Result<TokenSet, FlowError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|_| FlowError::BadTokenResponse("body is not JSON".into()))?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(FlowError::TokenEndpoint(sanitize_code(err)));
    }
    let access = v
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| FlowError::BadTokenResponse("missing access_token".into()))?;
    let refresh = v
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(Secret::new);
    let expires_in = v.get("expires_in").and_then(Value::as_u64);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(TokenSet {
        access_token: Secret::new(access),
        refresh_token: refresh,
        expires_in,
        token_type: v.get("token_type").and_then(Value::as_str).map(str::to_string),
        scope: v.get("scope").and_then(Value::as_str).map(str::to_string),
        expires_at_unix: expires_in.map(|e| now + e),
    })
}

/// POST application/x-www-form-urlencoded to the token endpoint.
/// `http_status_as_error(false)` so OAuth error bodies (400 + JSON) are parsed.
pub fn exchange_code(
    token_url: &str,
    client_id: &str,
    code: &Secret,
    verifier: &Secret,
    redirect_uri: &str,
    timeout: Duration,
) -> Result<TokenSet, FlowError> {
    // Parsed-URL decision: cleartext only for a real loopback host, else https.
    let parsed = crate::endpoint::require_secure_or_loopback(token_url).map_err(FlowError::Io)?;
    let is_loopback = crate::endpoint::is_loopback_url(&parsed);
    let mut cfg = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout));
    if is_loopback {
        // Never route loopback mock traffic through an env proxy.
        cfg = cfg.proxy(None);
    }
    let agent: ureq::Agent = cfg.build().into();
    let mut resp = agent
        .post(token_url)
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("code_verifier", verifier.expose()),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
        ])
        .map_err(|e| FlowError::Io(format!("token request failed: {e}")))?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| FlowError::Io(format!("reading token response: {e}")))?;
    let parsed = parse_token_response(&body);
    match (status, parsed) {
        (200..=299, Ok(t)) => Ok(t),
        (_, Err(e @ FlowError::TokenEndpoint(_))) => Err(e),
        (s, Err(_)) if !(200..=299).contains(&s) => {
            Err(FlowError::TokenEndpoint(format!("http_{s}")))
        }
        (_, Err(e)) => Err(e),
        (s, Ok(_)) => Err(FlowError::TokenEndpoint(format!("http_{s}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_response() {
        let t = parse_token_response(
            r#"{"access_token":"SYNTH-A","refresh_token":"SYNTH-R","expires_in":3600,"token_type":"Bearer","scope":"a b"}"#,
        )
        .unwrap();
        assert_eq!(t.access_token.expose(), "SYNTH-A");
        assert_eq!(t.refresh_token.as_ref().unwrap().expose(), "SYNTH-R");
        assert_eq!(t.expires_in, Some(3600));
        assert_eq!(t.token_type.as_deref(), Some("Bearer"));
        assert_eq!(t.scope.as_deref(), Some("a b"));
        assert!(t.expires_at_unix.unwrap() > 1_700_000_000);
    }

    #[test]
    fn refresh_and_expiry_are_optional() {
        let t = parse_token_response(r#"{"access_token":"SYNTH-A"}"#).unwrap();
        assert!(t.refresh_token.is_none() && t.expires_in.is_none());
    }

    #[test]
    fn oauth_error_and_garbage_are_errors() {
        assert_eq!(
            parse_token_response(r#"{"error":"invalid_grant","error_description":"x"}"#)
                .unwrap_err(),
            FlowError::TokenEndpoint("invalid_grant".into())
        );
        assert!(matches!(
            parse_token_response("<html>"),
            Err(FlowError::BadTokenResponse(_))
        ));
        assert!(matches!(
            parse_token_response(r#"{"access_token":""}"#),
            Err(FlowError::BadTokenResponse(_))
        ));
    }

    #[test]
    fn token_set_debug_is_redacted() {
        let t = parse_token_response(
            r#"{"access_token":"SYNTH-LEAKME-A","refresh_token":"SYNTH-LEAKME-R","expires_in":1}"#,
        )
        .unwrap();
        let rendered = format!("{t:?} {t:#?}");
        assert!(!rendered.contains("LEAKME"), "{rendered}");
    }
}
