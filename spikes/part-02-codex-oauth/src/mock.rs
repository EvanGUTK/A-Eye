//! Local MOCK OAuth provider + simulated browser. Test/spike scaffolding only.
//!
//! No real provider is ever contacted (AGENTS.md rule 12). The mock behaves like
//! a standards-following authorization server:
//!   GET  /oauth/authorize  validates the request and 302s to the redirect_uri
//!                          with ?code=..&state=.. (the "user approved" step)
//!   POST /oauth/token      form-encoded; verifies grant_type, client_id,
//!                          redirect_uri, single-use code, and PKCE
//!                          (S256(code_verifier) == stored code_challenge).
//! All tokens it mints are clearly synthetic.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rand::Rng;

use crate::pkce;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// User approves; redirect carries the genuine state.
    Approve,
    /// User denies: redirect carries error=access_denied and the genuine state.
    Deny,
    /// Hostile/buggy redirect carrying a different state.
    TamperState,
}

struct Pending {
    challenge: String,
    redirect_uri: String,
    used: bool,
}

#[derive(Default)]
struct State {
    pending: HashMap<String, Pending>,
    events: Vec<String>,
    token_requests: usize,
}

pub struct MockProvider {
    addr: SocketAddr,
    client_id: String,
    state: Arc<Mutex<State>>,
    mode: Arc<Mutex<Mode>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockProvider {
    pub fn start(client_id: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let mode = Arc::new(Mutex::new(Mode::Approve));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let (state, mode, stop, client_id) =
                (state.clone(), mode.clone(), stop.clone(), client_id.to_string());
            std::thread::spawn(move || serve(listener, &client_id, &state, &mode, &stop))
        };
        Ok(MockProvider { addr, client_id: client_id.to_string(), state, mode, stop, handle: Some(handle) })
    }

    pub fn set_mode(&self, m: Mode) {
        *self.mode.lock().unwrap() = m;
    }
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
    pub fn authorize_url(&self) -> String {
        format!("http://{}/oauth/authorize", self.addr)
    }
    pub fn token_url(&self) -> String {
        format!("http://{}/oauth/token", self.addr)
    }
    pub fn events(&self) -> Vec<String> {
        self.state.lock().unwrap().events.clone()
    }
    pub fn token_requests(&self) -> usize {
        self.state.lock().unwrap().token_requests
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(
    listener: TcpListener,
    client_id: &str,
    state: &Mutex<State>,
    mode: &Mutex<Mode>,
    stop: &AtomicBool,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => handle(stream, client_id, state, *mode.lock().unwrap()),
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

struct Req {
    method: String,
    path: String,
    query: String,
    body: Vec<u8>,
}

fn read_req(stream: &mut TcpStream) -> Option<Req> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 16 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8(buf[..head_end].to_vec()).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let content_length: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    if content_length > 16 * 1024 {
        return None;
    }
    let mut body = buf[head_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Req { method, path: path.to_string(), query: query.to_string(), body })
}

fn write_resp(stream: &mut TcpStream, status: &str, extra: &str, ctype: &str, body: &str) {
    let msg = format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes());
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn json_err(stream: &mut TcpStream, status: &str, error: &str, desc: &str) {
    let body = serde_json::json!({"error": error, "error_description": desc}).to_string();
    write_resp(stream, status, "", "application/json", &body);
}

fn rand_token(prefix: &str) -> String {
    let mut b = [0u8; 18];
    rand::rng().fill_bytes(&mut b);
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(b))
}

fn handle(mut stream: TcpStream, client_id: &str, state: &Mutex<State>, mode: Mode) {
    let Some(req) = read_req(&mut stream) else {
        write_resp(&mut stream, "400 Bad Request", "", "text/plain", "bad request");
        return;
    };
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/oauth/authorize") => authorize(&mut stream, &req, client_id, state, mode),
        ("POST", "/oauth/token") => token(&mut stream, &req, client_id, state),
        _ => write_resp(&mut stream, "404 Not Found", "", "text/plain", "not found"),
    }
}

fn authorize(stream: &mut TcpStream, req: &Req, client_id: &str, state: &Mutex<State>, mode: Mode) {
    let q: HashMap<String, String> = url::form_urlencoded::parse(req.query.as_bytes()).into_owned().collect();
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let problems: Vec<&str> = [
        (get("response_type") != "code", "response_type must be code"),
        (get("client_id") != client_id, "unknown client_id"),
        (!get("redirect_uri").starts_with("http://127.0.0.1:"), "redirect_uri must be http://127.0.0.1:<port>"),
        (get("code_challenge").len() < 43, "code_challenge missing/short"),
        (get("code_challenge_method") != "S256", "code_challenge_method must be S256"),
        (get("state").is_empty(), "state missing"),
    ]
    .into_iter()
    .filter_map(|(bad, msg)| bad.then_some(msg))
    .collect();
    if !problems.is_empty() {
        state.lock().unwrap().events.push(format!("authorize rejected: {}", problems.join("; ")));
        json_err(stream, "400 Bad Request", "invalid_request", &problems.join("; "));
        return;
    }

    let redirect_uri = get("redirect_uri").to_string();
    let mut target = match url::Url::parse(&redirect_uri) {
        Ok(u) => u,
        Err(_) => return json_err(stream, "400 Bad Request", "invalid_request", "bad redirect_uri"),
    };
    {
        let mut qp = target.query_pairs_mut();
        match mode {
            Mode::Approve | Mode::TamperState => {
                let code = rand_token("SYNTHETIC-code-");
                state.lock().unwrap().pending.insert(
                    code.clone(),
                    Pending { challenge: get("code_challenge").to_string(), redirect_uri: redirect_uri.clone(), used: false },
                );
                qp.append_pair("code", &code);
                qp.append_pair("state", if mode == Mode::TamperState { "attacker-state" } else { get("state") });
            }
            Mode::Deny => {
                qp.append_pair("error", "access_denied");
                qp.append_pair("error_description", "user denied the request");
                qp.append_pair("state", get("state"));
            }
        }
    }
    state.lock().unwrap().events.push(format!(
        "authorize 302 ({mode:?}): S256 challenge recorded, redirect_uri port {}",
        target.port().unwrap_or(0)
    ));
    let loc = format!("Location: {target}\r\n");
    write_resp(stream, "302 Found", &loc, "text/plain", "redirecting");
}

fn token(stream: &mut TcpStream, req: &Req, client_id: &str, state: &Mutex<State>) {
    let f: HashMap<String, String> = url::form_urlencoded::parse(&req.body).into_owned().collect();
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    let mut st = state.lock().unwrap();
    st.token_requests += 1;

    if get("grant_type") != "authorization_code" {
        return json_err(stream, "400 Bad Request", "unsupported_grant_type", "grant_type must be authorization_code");
    }
    if get("client_id") != client_id {
        return json_err(stream, "401 Unauthorized", "invalid_client", "unknown client_id");
    }
    let Some(p) = st.pending.get_mut(get("code")) else {
        st.events.push("token rejected: unknown code".into());
        return json_err(stream, "400 Bad Request", "invalid_grant", "authorization code is invalid");
    };
    if p.used {
        st.events.push("token rejected: code reuse".into());
        return json_err(stream, "400 Bad Request", "invalid_grant", "authorization code already used");
    }
    p.used = true; // single use, even if the checks below fail (RFC 6749 4.1.2)
    if p.redirect_uri != get("redirect_uri") {
        st.events.push("token rejected: redirect_uri mismatch".into());
        return json_err(stream, "400 Bad Request", "invalid_grant", "redirect_uri mismatch");
    }
    if pkce::challenge_for(get("code_verifier")) != p.challenge {
        st.events.push("token rejected: PKCE verifier mismatch".into());
        return json_err(stream, "400 Bad Request", "invalid_grant", "PKCE verification failed");
    }
    st.events.push("token ok: PKCE S256 verified, code consumed".into());
    let body = serde_json::json!({
        "access_token": rand_token("SYNTHETIC-access-"),
        "refresh_token": rand_token("SYNTHETIC-refresh-"),
        "id_token": rand_token("SYNTHETIC-id."),
        "token_type": "Bearer",
        "expires_in": 3600,
        "scope": "openid profile email offline_access"
    })
    .to_string();
    write_resp(stream, "200 OK", "Cache-Control: no-store\r\n", "application/json", &body);
}

/// Plays the browser: GET the authorization URL without auto-following, then
/// GET the Location it returns (the loopback callback). Returns the HTTP status
/// of the callback response.
pub fn simulate_browser(authorize_url: &str) -> Result<u16, String> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let first = client.get(authorize_url).send().map_err(|e| format!("authorize request failed: {}", e.without_url()))?;
    if first.status().as_u16() != 302 {
        return Err(format!("authorize endpoint answered HTTP {} (expected 302)", first.status().as_u16()));
    }
    let loc = first
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .ok_or("302 without Location")?
        .to_string();
    let cb = client.get(&loc).send().map_err(|e| format!("callback request failed: {}", e.without_url()))?;
    Ok(cb.status().as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::Secret;
    use crate::token::{self, ExchangeRequest, TokenError};

    const CID: &str = "mock-client-id";

    /// Run authorize -> capture code from the Location header, like a browser would.
    fn authorize_for_code(mock: &MockProvider, redirect_uri: &str, challenge: &str, state: &str) -> (String, String) {
        let mut u = url::Url::parse(&mock.authorize_url()).unwrap();
        u.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CID)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state);
        let client = reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let r = client.get(u).send().unwrap();
        assert_eq!(r.status().as_u16(), 302);
        let loc = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
        let q: HashMap<_, _> = loc.query_pairs().into_owned().collect();
        (q["code"].clone(), q["state"].clone())
    }

    const REDIRECT: &str = "http://127.0.0.1:40000/callback";

    fn exchange(mock: &MockProvider, code: &str, verifier: &str, redirect: &str) -> Result<token::TokenSet, TokenError> {
        let client = token::http_client().unwrap();
        token::exchange_code(
            &client,
            &ExchangeRequest {
                token_url: &mock.token_url(),
                client_id: CID,
                code: &Secret::new(code),
                code_verifier: &Secret::new(verifier),
                redirect_uri: redirect,
            },
        )
    }

    #[test]
    fn exchange_succeeds_with_valid_pkce() {
        let mock = MockProvider::start(CID).unwrap();
        let p = pkce::generate();
        let (code, state) = authorize_for_code(&mock, REDIRECT, &p.challenge, "s1");
        assert_eq!(state, "s1");
        let t = exchange(&mock, &code, p.verifier.expose(), REDIRECT).unwrap();
        assert!(t.access_token.expose().starts_with("SYNTHETIC-access-"));
        assert!(t.refresh_token.is_some() && t.id_token.is_some());
        assert_eq!(t.expires_in, Some(3600));
    }

    #[test]
    fn wrong_verifier_is_rejected() {
        let mock = MockProvider::start(CID).unwrap();
        let p = pkce::generate();
        let (code, _) = authorize_for_code(&mock, REDIRECT, &p.challenge, "s");
        let other = pkce::generate();
        match exchange(&mock, &code, other.verifier.expose(), REDIRECT) {
            Err(TokenError::Rejected { status: 400, error: Some(e), .. }) => assert_eq!(e, "invalid_grant"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn invalid_code_and_code_reuse_are_rejected_with_clear_message() {
        let mock = MockProvider::start(CID).unwrap();
        let p = pkce::generate();
        let err = exchange(&mock, "not-a-real-code", p.verifier.expose(), REDIRECT).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("HTTP 400") && msg.contains("invalid_grant"), "{msg}");

        let (code, _) = authorize_for_code(&mock, REDIRECT, &p.challenge, "s");
        exchange(&mock, &code, p.verifier.expose(), REDIRECT).unwrap();
        let again = exchange(&mock, &code, p.verifier.expose(), REDIRECT).unwrap_err();
        assert!(again.to_string().contains("already used"), "{again}");
    }

    #[test]
    fn redirect_uri_mismatch_is_rejected() {
        let mock = MockProvider::start(CID).unwrap();
        let p = pkce::generate();
        let (code, _) = authorize_for_code(&mock, REDIRECT, &p.challenge, "s");
        let err = exchange(&mock, &code, p.verifier.expose(), "http://127.0.0.1:40001/callback").unwrap_err();
        assert!(err.to_string().contains("redirect_uri mismatch"), "{err}");
    }

    #[test]
    fn authorize_rejects_non_s256_and_plain_http_redirects() {
        let mock = MockProvider::start(CID).unwrap();
        let client = reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let mut u = url::Url::parse(&mock.authorize_url()).unwrap();
        u.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", CID)
            .append_pair("redirect_uri", "http://evil.example/cb")
            .append_pair("code_challenge", "x")
            .append_pair("code_challenge_method", "plain")
            .append_pair("state", "s");
        assert_eq!(client.get(u).send().unwrap().status().as_u16(), 400);
    }

    #[test]
    fn network_failure_is_a_clear_error_without_url_or_secrets() {
        // Bind then drop to obtain a port nothing listens on.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port();
        let client = token::http_client().unwrap();
        let err = token::exchange_code(
            &client,
            &ExchangeRequest {
                token_url: &format!("http://127.0.0.1:{port}/oauth/token"),
                client_id: CID,
                code: &Secret::new("CODE-SECRET"),
                code_verifier: &Secret::new("VERIFIER-SECRET"),
                redirect_uri: REDIRECT,
            },
        )
        .unwrap_err();
        assert!(matches!(err, TokenError::Network(_)), "{err:?}");
        let msg = format!("{err} {err:?}");
        assert!(!msg.contains("SECRET") && !msg.contains("127.0.0.1"), "{msg}");
    }

    #[test]
    fn non_json_error_body_is_handled() {
        // A server that answers 502 with HTML.
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 6\r\nConnection: close\r\n\r\n<html>");
        });
        let client = token::http_client().unwrap();
        let err = token::exchange_code(
            &client,
            &ExchangeRequest {
                token_url: &format!("http://127.0.0.1:{port}/oauth/token"),
                client_id: CID,
                code: &Secret::new("c"),
                code_verifier: &Secret::new("v"),
                redirect_uri: REDIRECT,
            },
        )
        .unwrap_err();
        h.join().unwrap();
        assert!(matches!(err, TokenError::Rejected { status: 502, error: None, .. }), "{err:?}");
    }
}
