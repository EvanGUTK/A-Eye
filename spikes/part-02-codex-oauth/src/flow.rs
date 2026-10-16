//! Orchestration: PKCE -> loopback callback server -> authorize URL -> (browser)
//! -> callback -> token exchange. Storage is a separate step (see `store`).

use std::time::Duration;

use crate::callback::{CallbackError, CallbackServer};
use crate::pkce;
use crate::token::{self, ExchangeRequest, TokenError, TokenSet};

#[derive(Clone, Debug)]
pub struct FlowConfig {
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scope: String,
    /// 0 = random free port.
    pub callback_port: u16,
    pub callback_path: String,
    pub callback_timeout: Duration,
}

#[derive(Debug)]
pub enum FlowError {
    Config(String),
    Callback(CallbackError),
    Token(TokenError),
}

impl std::fmt::Display for FlowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlowError::Config(m) => write!(f, "configuration error: {m}"),
            FlowError::Callback(e) => write!(f, "{e}"),
            FlowError::Token(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FlowError {}

pub struct FlowOutcome {
    pub tokens: TokenSet,
    pub redirect_uri: String,
    pub callback_port: u16,
}

/// Build the authorization request URL (RFC 6749 4.1.1 + RFC 7636 4.3).
pub fn build_authorize_url(
    cfg: &FlowConfig,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> Result<url::Url, FlowError> {
    let mut u = url::Url::parse(&cfg.authorize_url)
        .map_err(|e| FlowError::Config(format!("authorize URL is not a valid URL: {e}")))?;
    u.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &cfg.scope)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    Ok(u)
}

/// Run the flow. `open_browser` receives the authorization URL after the
/// callback server is already listening. The mock run drives it from a test
/// "browser"; the live mode prints it for the user.
pub fn run(
    cfg: &FlowConfig,
    open_browser: impl FnOnce(&url::Url),
) -> Result<FlowOutcome, FlowError> {
    let pkce = pkce::generate();
    let state = pkce::generate_state();

    let server = CallbackServer::bind(cfg.callback_port)
        .map_err(FlowError::Callback)?
        .with_path(&cfg.callback_path);
    let redirect_uri = server.redirect_uri();
    let callback_port = server.local_addr().port();

    let auth_url = build_authorize_url(cfg, &redirect_uri, &pkce.challenge, &state)?;
    open_browser(&auth_url);

    let code = server.wait(&state, cfg.callback_timeout).map_err(FlowError::Callback)?;

    let client = token::http_client().map_err(FlowError::Token)?;
    let tokens = token::exchange_code(
        &client,
        &ExchangeRequest {
            token_url: &cfg.token_url,
            client_id: &cfg.client_id,
            code: &code,
            code_verifier: &pkce.verifier,
            redirect_uri: &redirect_uri,
        },
    )
    .map_err(FlowError::Token)?;

    Ok(FlowOutcome { tokens, redirect_uri, callback_port })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockProvider, Mode, simulate_browser};
    use std::net::{Ipv4Addr, TcpStream};

    fn cfg(m: &MockProvider, timeout_ms: u64) -> FlowConfig {
        FlowConfig {
            authorize_url: m.authorize_url(),
            token_url: m.token_url(),
            client_id: m.client_id().to_string(),
            scope: "openid profile email offline_access".into(),
            callback_port: 0,
            callback_path: "/callback".into(),
            callback_timeout: Duration::from_millis(timeout_ms),
        }
    }

    fn browser(url: &url::Url) -> std::thread::JoinHandle<Result<u16, String>> {
        let u = url.to_string();
        std::thread::spawn(move || simulate_browser(&u))
    }

    #[test]
    fn authorize_url_carries_pkce_and_state() {
        let m = MockProvider::start("cid").unwrap();
        let u = build_authorize_url(&cfg(&m, 1000), "http://127.0.0.1:1/callback", "CH", "ST").unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge"], "CH");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], "ST");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:1/callback");
        assert!(!q.contains_key("code_verifier"), "verifier must never be in the authorize URL");
    }

    #[test]
    fn full_flow_against_mock_succeeds_and_frees_port() {
        let m = MockProvider::start("cid").unwrap();
        let mut h = None;
        let out = run(&cfg(&m, 5000), |u| h = Some(browser(u))).unwrap();
        assert_eq!(h.unwrap().join().unwrap().unwrap(), 200);
        assert!(out.tokens.access_token.expose().starts_with("SYNTHETIC-access-"));
        assert!(out.tokens.refresh_token.is_some());
        assert_eq!(out.tokens.expires_in, Some(3600));
        assert!(m.events().iter().any(|e| e.contains("PKCE S256 verified")));
        assert!(
            TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, out.callback_port).into(), Duration::from_millis(300))
                .is_err(),
            "callback port must be closed after the flow"
        );
    }

    #[test]
    fn tampered_state_never_reaches_token_exchange() {
        let m = MockProvider::start("cid").unwrap();
        m.set_mode(Mode::TamperState);
        let mut h = None;
        let err = run(&cfg(&m, 800), |u| h = Some(browser(u))).err().unwrap();
        assert!(matches!(err, FlowError::Callback(CallbackError::Timeout(_))), "{err}");
        // browser saw the 400 from the callback server
        assert_eq!(h.unwrap().join().unwrap().unwrap(), 400);
        assert_eq!(m.token_requests(), 0);
    }

    #[test]
    fn user_denial_is_a_clear_provider_error() {
        let m = MockProvider::start("cid").unwrap();
        m.set_mode(Mode::Deny);
        let mut h = None;
        let err = run(&cfg(&m, 3000), |u| h = Some(browser(u))).err().unwrap();
        let _ = h.unwrap().join();
        assert!(err.to_string().contains("access_denied"), "{err}");
        assert_eq!(m.token_requests(), 0);
    }

    #[test]
    fn no_browser_means_timeout_err() {
        let m = MockProvider::start("cid").unwrap();
        let err = run(&cfg(&m, 300), |_| {}).err().unwrap();
        assert!(matches!(err, FlowError::Callback(CallbackError::Timeout(_))));
    }

    #[test]
    fn unreachable_token_endpoint_after_valid_callback_is_token_error() {
        let m = MockProvider::start("cid").unwrap();
        let mut c = cfg(&m, 3000);
        let dead = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        c.token_url = format!("http://127.0.0.1:{dead}/oauth/token");
        let mut h = None;
        let err = run(&c, |u| h = Some(browser(u))).err().unwrap();
        let _ = h.unwrap().join();
        assert!(matches!(err, FlowError::Token(TokenError::Network(_))), "{err}");
    }

    #[test]
    fn bad_authorize_url_is_config_error() {
        let m = MockProvider::start("cid").unwrap();
        let mut c = cfg(&m, 300);
        c.authorize_url = "not a url".into();
        assert!(matches!(run(&c, |_| {}).err().unwrap(), FlowError::Config(_)));
    }
}
