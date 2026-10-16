//! Local MOCK authorization + token server and a "browser" test client.
//!
//! This stands in for the provider so the whole PKCE flow can be exercised
//! without any real provider call (AGENTS.md rule 12). The mock enforces the
//! protocol rules a real server would, so a green run is not vacuous:
//! S256 challenge/verifier matching, one-shot codes, redirect_uri equality,
//! form-encoded token requests. All tokens are SYNTHETIC.

use crate::http::{read_request, write_response};
use crate::pkce::{challenge_for, random_hex};
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use url::Url;

pub const MOCK_CLIENT_ID: &str = "mock-client-id";
pub const MOCK_SCOPE: &str = "mock.scope.read mock.scope.write";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    Normal,
    /// Redirect back with a tampered `state`.
    WrongState,
    /// Redirect back with the right `state` but no `code`.
    MissingCode,
    /// Redirect back with `error=access_denied`.
    AccessDenied,
    /// Token endpoint always answers `invalid_grant`.
    TokenInvalidGrant,
}

#[derive(Debug, Default, Clone)]
pub struct Observations {
    pub authorize_hits: u32,
    pub token_hits: u32,
    pub saw_s256: bool,
    pub saw_form_encoded: bool,
    pub pkce_verified: bool,
    pub redirect_uri_matched: bool,
    pub replay_rejected: u32,
    pub bad_verifier_rejected: u32,
}

struct IssuedCode {
    challenge: String,
    redirect_uri: String,
    used: bool,
}

pub struct MockProvider {
    pub base: String,
    pub obs: Arc<Mutex<Observations>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl MockProvider {
    pub fn auth_url(&self) -> String {
        format!("{}/authorize", self.base)
    }
    pub fn token_url(&self) -> String {
        format!("{}/token", self.base)
    }
    pub fn observations(&self) -> Observations {
        self.obs.lock().unwrap().clone()
    }

    pub fn start(behavior: Behavior) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let obs = Arc::new(Mutex::new(Observations::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let codes: Arc<Mutex<HashMap<String, IssuedCode>>> = Arc::new(Mutex::new(HashMap::new()));
        let (obs2, stop2) = (obs.clone(), stop.clone());
        let join = thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let Ok(req) = read_request(&mut stream, Duration::from_secs(2)) else {
                            continue;
                        };
                        let url = Url::parse(&format!("http://127.0.0.1{}", req.target)).ok();
                        let path = url.as_ref().map(|u| u.path().to_string()).unwrap_or_default();
                        match (req.method.as_str(), path.as_str()) {
                            ("GET", "/authorize") => {
                                handle_authorize(&mut stream, url.unwrap(), behavior, &obs2, &codes)
                            }
                            ("POST", "/token") => handle_token(&mut stream, &req, behavior, &obs2, &codes),
                            _ => {
                                let _ = write_response(&mut stream, 404, "Not Found", "text/plain", &[], "");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(MockProvider {
            base: format!("http://127.0.0.1:{port}"),
            obs,
            stop,
            join: Some(join),
        })
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn json_response(stream: &mut std::net::TcpStream, status: u16, reason: &str, body: &str) {
    let _ = write_response(stream, status, reason, "application/json", &[], body);
}

fn handle_authorize(
    stream: &mut std::net::TcpStream,
    url: Url,
    behavior: Behavior,
    obs: &Arc<Mutex<Observations>>,
    codes: &Arc<Mutex<HashMap<String, IssuedCode>>>,
) {
    let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
    obs.lock().unwrap().authorize_hits += 1;
    let valid = q.get("response_type").map(String::as_str) == Some("code")
        && q.get("client_id").map(String::as_str) == Some(MOCK_CLIENT_ID)
        && q.get("code_challenge_method").map(String::as_str) == Some("S256")
        && q.get("code_challenge").is_some_and(|c| c.len() >= 43)
        && q.get("state").is_some_and(|s| !s.is_empty())
        && q.contains_key("scope")
        && q.get("redirect_uri").is_some_and(|r| r.starts_with("http://127.0.0.1:"));
    if !valid {
        let _ = write_response(stream, 400, "Bad Request", "text/plain", &[], "invalid_request");
        return;
    }
    obs.lock().unwrap().saw_s256 = true;
    let redirect_uri = q["redirect_uri"].clone();
    let state = q["state"].clone();
    let mut back = Url::parse(&redirect_uri).unwrap();
    {
        let mut pairs = back.query_pairs_mut();
        match behavior {
            Behavior::AccessDenied => {
                pairs.append_pair("error", "access_denied").append_pair("state", &state);
            }
            Behavior::MissingCode => {
                pairs.append_pair("state", &state);
            }
            Behavior::WrongState | Behavior::Normal | Behavior::TokenInvalidGrant => {
                let code = format!("mock-code-{}", random_hex(16).unwrap());
                codes.lock().unwrap().insert(
                    code.clone(),
                    IssuedCode {
                        challenge: q["code_challenge"].clone(),
                        redirect_uri: redirect_uri.clone(),
                        used: false,
                    },
                );
                let st = if behavior == Behavior::WrongState { "tampered-state" } else { &state };
                pairs.append_pair("code", &code).append_pair("state", st);
            }
        }
    }
    let _ = write_response(stream, 302, "Found", "text/plain", &[("Location", back.as_str())], "");
}

fn handle_token(
    stream: &mut std::net::TcpStream,
    req: &crate::http::Request,
    behavior: Behavior,
    obs: &Arc<Mutex<Observations>>,
    codes: &Arc<Mutex<HashMap<String, IssuedCode>>>,
) {
    obs.lock().unwrap().token_hits += 1;
    let form_ok = req
        .header("content-type")
        .is_some_and(|c| c.starts_with("application/x-www-form-urlencoded"));
    if !form_ok {
        json_response(stream, 400, "Bad Request", r#"{"error":"invalid_request"}"#);
        return;
    }
    obs.lock().unwrap().saw_form_encoded = true;
    if behavior == Behavior::TokenInvalidGrant {
        json_response(stream, 400, "Bad Request", r#"{"error":"invalid_grant"}"#);
        return;
    }
    let f: HashMap<String, String> = url::form_urlencoded::parse(&req.body).into_owned().collect();
    if f.get("grant_type").map(String::as_str) != Some("authorization_code") {
        json_response(stream, 400, "Bad Request", r#"{"error":"unsupported_grant_type"}"#);
        return;
    }
    let (Some(code), Some(verifier), Some(redirect)) =
        (f.get("code"), f.get("code_verifier"), f.get("redirect_uri"))
    else {
        json_response(stream, 400, "Bad Request", r#"{"error":"invalid_request"}"#);
        return;
    };
    if f.get("client_id").map(String::as_str) != Some(MOCK_CLIENT_ID) {
        json_response(stream, 401, "Unauthorized", r#"{"error":"invalid_client"}"#);
        return;
    }
    let mut issued = codes.lock().unwrap();
    let Some(entry) = issued.get_mut(code) else {
        json_response(stream, 400, "Bad Request", r#"{"error":"invalid_grant"}"#);
        return;
    };
    if entry.used {
        obs.lock().unwrap().replay_rejected += 1;
        json_response(stream, 400, "Bad Request", r#"{"error":"invalid_grant"}"#);
        return;
    }
    if challenge_for(verifier) != entry.challenge {
        obs.lock().unwrap().bad_verifier_rejected += 1;
        json_response(stream, 400, "Bad Request", r#"{"error":"invalid_grant"}"#);
        return;
    }
    if *redirect != entry.redirect_uri {
        json_response(stream, 400, "Bad Request", r#"{"error":"invalid_grant"}"#);
        return;
    }
    entry.used = true;
    {
        let mut o = obs.lock().unwrap();
        o.pkce_verified = true;
        o.redirect_uri_matched = true;
    }
    let body = format!(
        r#"{{"access_token":"SYNTH-ACCESS-{}","refresh_token":"SYNTH-REFRESH-{}","expires_in":3600,"token_type":"Bearer","scope":"{}"}}"#,
        random_hex(24).unwrap(),
        random_hex(24).unwrap(),
        MOCK_SCOPE
    );
    json_response(stream, 200, "OK", &body);
}

/// Plays the user's browser: GET the authorize URL and follow the redirect to
/// the loopback callback. Returns the final HTTP status.
pub fn play_browser(authorize_url: String) -> JoinHandle<Result<u16, String>> {
    thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(5)))
            .build()
            .into();
        agent
            .get(&authorize_url)
            .call()
            .map(|r| r.status().as_u16())
            .map_err(|e| e.to_string())
    })
}

/// Run the full flow against a fresh mock provider with the browser played by
/// `play_browser` (or no browser at all, to exercise the timeout path).
pub fn run_against_mock(
    behavior: Behavior,
    callback_timeout: Duration,
    with_browser: bool,
) -> (Result<crate::token::TokenSet, crate::error::FlowError>, Observations) {
    let mock = MockProvider::start(behavior).expect("mock bind");
    let cfg = crate::flow::FlowConfig {
        client_id: MOCK_CLIENT_ID.into(),
        auth_url: mock.auth_url(),
        token_url: mock.token_url(),
        scope: MOCK_SCOPE.into(),
        callback_timeout,
        http_timeout: Duration::from_secs(5),
    };
    let mut browser = None;
    let result = crate::flow::run_flow(&cfg, |url| {
        if with_browser {
            browser = Some(play_browser(url.to_string()));
        }
    });
    if let Some(b) = browser {
        let _ = b.join();
    }
    let obs = mock.observations();
    (result, obs)
}

/// GET without following redirects; returns the `Location` header if any.
pub fn authorize_no_follow(authorize_url: &str) -> Result<Option<String>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .proxy(None)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();
    let resp = agent.get(authorize_url).call().map_err(|e| e.to_string())?;
    Ok(resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string))
}
