//! Authorization-code -> token exchange (RFC 6749 section 4.1.3 + RFC 7636 section 4.5).
//!
//! Form-encoded POST: grant_type=authorization_code, code, code_verifier,
//! redirect_uri, client_id. Errors never contain the response body verbatim,
//! only the RFC 6749 `error` / `error_description` fields.

use std::io::Read;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::secret::Secret;

const MAX_RESPONSE_BYTES: u64 = 256 * 1024;

pub struct ExchangeRequest<'a> {
    pub token_url: &'a str,
    pub client_id: &'a str,
    pub code: &'a Secret,
    pub code_verifier: &'a Secret,
    pub redirect_uri: &'a str,
}

#[derive(Debug)]
pub struct TokenSet {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    pub id_token: Option<Secret>,
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub expires_in: Option<u64>,
    pub obtained_at: SystemTime,
}

impl TokenSet {
    pub fn expires_at(&self) -> Option<SystemTime> {
        self.expires_in.map(|s| self.obtained_at + Duration::from_secs(s))
    }
}

#[derive(Debug)]
pub enum TokenError {
    /// Transport failure (DNS, connect, TLS, timeout). Message has no URL or body.
    Network(String),
    /// The endpoint answered with a non-2xx status.
    Rejected { status: u16, error: Option<String>, description: Option<String> },
    /// 2xx, but the body was not a usable token response.
    Malformed(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::Network(m) => write!(f, "token endpoint unreachable: {m}"),
            TokenError::Rejected { status, error, description } => {
                write!(f, "token endpoint rejected the exchange (HTTP {status}")?;
                if let Some(e) = error {
                    write!(f, ", error={e}")?;
                }
                if let Some(d) = description {
                    write!(f, ", description={d}")?;
                }
                write!(f, ")")
            }
            TokenError::Malformed(m) => write!(f, "token response unusable: {m}"),
        }
    }
}

impl std::error::Error for TokenError {}

// No Debug derive: fields are raw strings until wrapped.
#[derive(Deserialize)]
struct RawTokenResponse {
    access_token: Secret,
    refresh_token: Option<Secret>,
    id_token: Option<Secret>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct RawError {
    error: Option<String>,
    error_description: Option<String>,
}

pub fn http_client() -> Result<reqwest::blocking::Client, TokenError> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        // A token endpoint must not bounce the code/verifier to another URL.
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("a-eye-spike-02/0.1")
        .build()
        .map_err(|e| TokenError::Network(format!("client init failed: {}", e.without_url())))
}

pub fn exchange_code(
    client: &reqwest::blocking::Client,
    req: &ExchangeRequest<'_>,
) -> Result<TokenSet, TokenError> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", req.code.expose())
        .append_pair("code_verifier", req.code_verifier.expose())
        .append_pair("redirect_uri", req.redirect_uri)
        .append_pair("client_id", req.client_id)
        .finish();

    let resp = client
        .post(req.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .map_err(|e| TokenError::Network(describe_transport(e)))?;

    let status = resp.status();
    let mut bytes = Vec::new();
    resp.take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| TokenError::Network(format!("reading response failed: {e}")))?;

    if !status.is_success() {
        let parsed: Option<RawError> = serde_json::from_slice(&bytes).ok();
        return Err(TokenError::Rejected {
            status: status.as_u16(),
            error: parsed.as_ref().and_then(|p| p.error.clone()),
            description: parsed.and_then(|p| p.error_description),
        });
    }

    parse_token_response(&bytes, SystemTime::now())
}

pub fn parse_token_response(bytes: &[u8], now: SystemTime) -> Result<TokenSet, TokenError> {
    // serde error text can quote input; do not forward it, only the category.
    let raw: RawTokenResponse = serde_json::from_slice(bytes).map_err(|e| {
        TokenError::Malformed(format!("not a token response JSON ({:?} at line {})", e.classify(), e.line()))
    })?;
    if raw.access_token.expose().is_empty() {
        return Err(TokenError::Malformed("empty access_token".into()));
    }
    Ok(TokenSet {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        id_token: raw.id_token,
        token_type: raw.token_type,
        scope: raw.scope,
        expires_in: raw.expires_in,
        obtained_at: now,
    })
}

fn describe_transport(e: reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timed out"
    } else if e.is_connect() {
        "connection failed"
    } else if e.is_redirect() {
        "unexpected redirect"
    } else if e.is_request() {
        "request error"
    } else {
        "transport error"
    };
    format!("{kind}: {}", e.without_url())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: SystemTime = SystemTime::UNIX_EPOCH;

    #[test]
    fn parses_full_response() {
        let body = br#"{"access_token":"at-1","refresh_token":"rt-1","id_token":"id-1",
            "token_type":"Bearer","expires_in":3600,"scope":"openid profile"}"#;
        let t = parse_token_response(body, NOW).unwrap();
        assert_eq!(t.access_token.expose(), "at-1");
        assert_eq!(t.refresh_token.as_ref().unwrap().expose(), "rt-1");
        assert_eq!(t.id_token.as_ref().unwrap().expose(), "id-1");
        assert_eq!(t.expires_in, Some(3600));
        assert_eq!(t.expires_at(), Some(NOW + Duration::from_secs(3600)));
    }

    #[test]
    fn id_token_and_expiry_are_optional() {
        let t = parse_token_response(br#"{"access_token":"a","refresh_token":"r"}"#, NOW).unwrap();
        assert!(t.id_token.is_none());
        assert!(t.expires_at().is_none());
    }

    #[test]
    fn rejects_missing_or_empty_access_token_and_garbage() {
        assert!(matches!(parse_token_response(br#"{"refresh_token":"r"}"#, NOW), Err(TokenError::Malformed(_))));
        assert!(matches!(parse_token_response(br#"{"access_token":""}"#, NOW), Err(TokenError::Malformed(_))));
        assert!(matches!(parse_token_response(b"<html>", NOW), Err(TokenError::Malformed(_))));
    }

    #[test]
    fn malformed_error_does_not_echo_input() {
        let err = parse_token_response(br#"{"access_token": 12345 "oops-secret-xyz"}"#, NOW).unwrap_err();
        assert!(!err.to_string().contains("oops-secret-xyz"));
    }

    #[test]
    fn token_set_debug_redacts() {
        let t = parse_token_response(br#"{"access_token":"AT-SECRET","refresh_token":"RT-SECRET","id_token":"ID-SECRET"}"#, NOW)
            .unwrap();
        let out = format!("{t:?} {t:#?}");
        for s in ["AT-SECRET", "RT-SECRET", "ID-SECRET"] {
            assert!(!out.contains(s), "leaked {s}");
        }
    }
}
