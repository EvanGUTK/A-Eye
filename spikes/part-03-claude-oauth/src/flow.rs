//! Orchestrates PKCE + loopback callback + token exchange.
//!
//! The caller supplies `present`, which receives the authorize URL. In tests
//! it plays the browser against the mock provider; in `--live` mode it prints
//! the URL for the user (no browser is auto-launched).

use crate::callback::CallbackServer;
use crate::error::FlowError;
use crate::pkce::{new_state, Pkce};
use crate::token::{exchange_code, TokenSet};
use std::time::Duration;
use url::Url;

#[derive(Debug, Clone)]
pub struct FlowConfig {
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub scope: String,
    /// Docs/product value is 5 minutes; tests use 1-2 s.
    pub callback_timeout: Duration,
    pub http_timeout: Duration,
}

pub const PRODUCT_CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub fn build_authorize_url(
    cfg: &FlowConfig,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> Result<Url, FlowError> {
    Url::parse_with_params(
        &cfg.auth_url,
        [
            ("response_type", "code"),
            ("client_id", cfg.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("scope", cfg.scope.as_str()),
            ("state", state),
        ],
    )
    .map_err(|e| FlowError::Io(format!("bad auth_url: {e}")))
}

pub fn run_flow(cfg: &FlowConfig, present: impl FnOnce(&str)) -> Result<TokenSet, FlowError> {
    let pkce = Pkce::generate().map_err(FlowError::Io)?;
    let state = new_state().map_err(FlowError::Io)?;
    let server = CallbackServer::bind()?;
    let redirect_uri = server.redirect_uri();
    let url = build_authorize_url(cfg, &redirect_uri, &pkce.challenge, &state)?;
    present(url.as_str());
    let code = server.wait(&state, cfg.callback_timeout)?;
    exchange_code(
        &cfg.token_url,
        &cfg.client_id,
        &code,
        &pkce.verifier,
        &redirect_uri,
        cfg.http_timeout,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_has_required_pkce_params() {
        let cfg = FlowConfig {
            client_id: "test-client".into(),
            auth_url: "http://127.0.0.1:1/authorize".into(),
            token_url: "http://127.0.0.1:1/token".into(),
            scope: "a b".into(),
            callback_timeout: Duration::from_secs(1),
            http_timeout: Duration::from_secs(1),
        };
        let u = build_authorize_url(&cfg, "http://127.0.0.1:9/callback", "CHAL", "ST").unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], "test-client");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:9/callback");
        assert_eq!(q["code_challenge"], "CHAL");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["scope"], "a b");
        assert_eq!(q["state"], "ST");
        assert!(!q.contains_key("code_verifier"), "verifier must never be in the URL");
    }
}
