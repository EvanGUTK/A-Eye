//! Local MOCK of the ChatGPT plan-usage endpoints (`/api/accounts/authorize`,
//! `/api/accounts/oauth/token`). Test scaffolding only: no real provider is contacted,
//! every token it mints is clearly synthetic, and its JWT signature is a fixed dummy.
//!
//! It enforces what the public docs say the real server requires, so a client that
//! forgets `resource`, sends the wrong redirect, or exchanges with the registration
//! entrypoint id is rejected here the way the docs say it would be. It cannot show
//! that the real server agrees: that needs the live run.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use crate::mock::{Req, json_err, rand_token, read_req, write_resp};
use crate::{pkce, siwc};

const SYNTHETIC_JWT_SIGNATURE: &str = "SYNTHETIC-signature";
const SYNTHETIC_TOKEN_LIFETIME_SECS: u64 = 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanMode {
    Approve,
    Deny,
    /// Callback carries no issued client_id.
    OmitClientId,
    /// Callback echoes `dynamic_agent_client` instead of an issued id.
    EchoEntrypointId,
    /// Token response omits the plan-usage scope.
    NoPlanScope,
    /// ID token carries a nonce other than the one the client sent.
    WrongNonce,
}

struct Pending {
    challenge: String,
    redirect_uri: String,
    nonce: String,
    issued_client_id: String,
    used: bool,
}

#[derive(Default)]
struct State {
    pending: HashMap<String, Pending>,
    events: Vec<String>,
    token_requests: usize,
}

pub struct PlanMock {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PlanMock {
    pub fn start(mode: PlanMode) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let (state, stop) = (state.clone(), stop.clone());
            std::thread::spawn(move || serve(listener, addr, mode, &state, &stop))
        };
        Ok(PlanMock { addr, state, stop, handle: Some(handle) })
    }

    pub fn authorize_url(&self) -> String {
        format!("http://{}/api/accounts/authorize", self.addr)
    }
    pub fn token_url(&self) -> String {
        format!("http://{}/api/accounts/oauth/token", self.addr)
    }
    /// The `iss` claim this mock puts in its ID tokens.
    pub fn issuer(&self) -> String {
        format!("http://{}", self.addr)
    }
    pub fn events(&self) -> Vec<String> {
        self.state.lock().unwrap().events.clone()
    }
    pub fn token_requests(&self) -> usize {
        self.state.lock().unwrap().token_requests
    }
}

impl Drop for PlanMock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Real-shaped header and payload with a dummy signature.
pub fn issue_jwt(claims: &serde_json::Value) -> String {
    let header = serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": "SYNTHETIC-kid"});
    let enc = |v: &serde_json::Value| URL_SAFE_NO_PAD.encode(v.to_string());
    format!("{}.{}.{SYNTHETIC_JWT_SIGNATURE}", enc(&header), enc(claims))
}

fn serve(listener: TcpListener, addr: SocketAddr, mode: PlanMode, state: &Mutex<State>, stop: &AtomicBool) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => handle(stream, addr, mode, state),
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn handle(mut stream: TcpStream, addr: SocketAddr, mode: PlanMode, state: &Mutex<State>) {
    let Some(req) = read_req(&mut stream) else {
        write_resp(&mut stream, "400 Bad Request", "", "text/plain", "bad request");
        return;
    };
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/accounts/authorize") => authorize(&mut stream, &req, mode, state),
        ("POST", "/api/accounts/oauth/token") => token(&mut stream, &req, addr, mode, state),
        _ => write_resp(&mut stream, "404 Not Found", "", "text/plain", "not found"),
    }
}

fn authorize(stream: &mut TcpStream, req: &Req, mode: PlanMode, state: &Mutex<State>) {
    let q: HashMap<String, String> = url::form_urlencoded::parse(req.query.as_bytes()).into_owned().collect();
    let get = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let redirect = url::Url::parse(get("redirect_uri")).ok();
    let redirect_ok = redirect.as_ref().is_some_and(|u| {
        u.scheme() == "http" && u.host_str() == Some("127.0.0.1") && u.path() == siwc::REDIRECT_PATH
    });
    let problems: Vec<&str> = [
        (get("client_id") != siwc::REGISTRATION_CLIENT_ID, "first registration must use client_id=dynamic_agent_client"),
        (!get("ext_agent_host_id").starts_with("urn:uuid:"), "ext_agent_host_id missing or not a urn:uuid"),
        (get("agent_name_hint").is_empty(), "agent_name_hint missing on first registration"),
        (get("response_type") != "code", "response_type must be code"),
        (!redirect_ok, "redirect_uri must be http://127.0.0.1:<port>/auth/callback"),
        (!get("scope").split_whitespace().any(|s| s == siwc::PLAN_USAGE_SCOPE), "scope must include the plan-usage scope"),
        (get("resource") != siwc::RESOURCE, "resource must be https://api.openai.com/v1"),
        (get("state").is_empty(), "state missing"),
        (get("nonce").is_empty(), "nonce missing"),
        (get("code_challenge_method") != "S256", "code_challenge_method must be S256"),
        (get("code_challenge").len() < 43, "code_challenge missing/short"),
    ]
    .into_iter()
    .filter_map(|(bad, msg)| bad.then_some(msg))
    .collect();
    if !problems.is_empty() {
        state.lock().unwrap().events.push(format!("authorize rejected: {}", problems.join("; ")));
        return json_err(stream, "400 Bad Request", "invalid_request", &problems.join("; "));
    }

    let mut target = redirect.expect("validated above");
    let issued = rand_token("oaiapp_SYNTHETIC-");
    {
        let mut st = state.lock().unwrap();
        st.events.push("authorize ok: host id seen, registration entrypoint accepted".into());
        let mut qp = target.query_pairs_mut();
        if mode == PlanMode::Deny {
            qp.append_pair("error", "access_denied").append_pair("state", get("state"));
        } else {
            let code = rand_token("SYNTHETIC-code-");
            st.pending.insert(
                code.clone(),
                Pending {
                    challenge: get("code_challenge").into(),
                    redirect_uri: get("redirect_uri").into(),
                    nonce: get("nonce").into(),
                    issued_client_id: issued.clone(),
                    used: false,
                },
            );
            qp.append_pair("code", &code)
                .append_pair("scope", "chatgpt.tokens.use.direct email offline_access openid profile resource.invoke")
                .append_pair("state", get("state"));
            match mode {
                PlanMode::OmitClientId => {}
                PlanMode::EchoEntrypointId => {
                    qp.append_pair("client_id", siwc::REGISTRATION_CLIENT_ID);
                }
                _ => {
                    qp.append_pair("client_id", &issued);
                }
            }
        }
    }
    write_resp(stream, "302 Found", &format!("Location: {target}\r\n"), "text/plain", "redirecting");
}

fn token(stream: &mut TcpStream, req: &Req, addr: SocketAddr, mode: PlanMode, state: &Mutex<State>) {
    let f: HashMap<String, String> = url::form_urlencoded::parse(&req.body).into_owned().collect();
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    let mut st = state.lock().unwrap();
    st.token_requests += 1;

    if get("grant_type") != "authorization_code" {
        return json_err(stream, "400 Bad Request", "unsupported_grant_type", "grant_type must be authorization_code");
    }
    if get("client_id") == siwc::REGISTRATION_CLIENT_ID {
        st.events.push("token rejected: exchanged with the registration entrypoint id".into());
        return json_err(stream, "401 Unauthorized", "invalid_client", "use the issued client_id");
    }
    let Some(p) = st.pending.get_mut(get("code")) else {
        st.events.push("token rejected: unknown code".into());
        return json_err(stream, "400 Bad Request", "invalid_grant", "authorization code is invalid");
    };
    let already_used = p.used;
    p.used = true; // single use, even when a check below fails
    let (issued, nonce) = (p.issued_client_id.clone(), p.nonce.clone());
    let failure = if already_used {
        Some("authorization code already used")
    } else if get("client_id") != issued {
        Some("client_id is not the one issued for this code")
    } else if p.redirect_uri != get("redirect_uri") {
        Some("redirect_uri mismatch")
    } else if pkce::challenge_for(get("code_verifier")) != p.challenge {
        Some("PKCE verification failed")
    } else if get("resource") != siwc::RESOURCE {
        Some("resource missing or wrong")
    } else {
        None
    };
    if let Some(why) = failure {
        st.events.push(format!("token rejected: {why}"));
        return json_err(stream, "400 Bad Request", "invalid_grant", why);
    }
    st.events.push("token ok: PKCE S256 verified, code consumed, resource sent".into());
    drop(st);

    let exp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) + SYNTHETIC_TOKEN_LIFETIME_SECS;
    let id_token = issue_jwt(&serde_json::json!({
        "iss": format!("http://{addr}"),
        "aud": issued,
        "nonce": if mode == PlanMode::WrongNonce { "replayed-nonce".to_string() } else { nonce },
        "exp": exp,
        "sub": "SYNTHETIC-subject",
    }));
    let scope = if mode == PlanMode::NoPlanScope {
        "openid profile email offline_access resource.invoke"
    } else {
        "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
    };
    let body = serde_json::json!({
        "access_token": rand_token("SYNTHETIC-access-"),
        "refresh_token": rand_token("SYNTHETIC-refresh-"),
        "id_token": id_token,
        "token_type": "Bearer",
        "expires_in": SYNTHETIC_TOKEN_LIFETIME_SECS,
        "scope": scope,
    })
    .to_string();
    write_resp(stream, "200 OK", "Cache-Control: no-store\r\n", "application/json", &body);
}
