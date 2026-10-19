//! OpenAI "Sign in with ChatGPT" plan-usage flow for open-source apps.
//!
//! This is the route OpenAI documents for third-party apps
//! (developers.openai.com/siwc/token-sharing-open-source/sign-in, read 2026-10-18):
//! the app registers itself through the authorize endpoint with the fixed
//! `dynamic_agent_client` id and receives its own issued `client_id` on the callback.
//! It does not borrow the Codex CLI's client id (the legacy `--live` route).
//!
//! Endpoints are from the provider's OIDC discovery document (fetched 2026-10-18).
//! The ID token signature is NOT verified here: the token arrives directly from the
//! token endpoint over TLS, which OIDC Core 3.1.3.7 step 6 allows in place of a
//! signature check. The product must still verify it against the provider JWKS.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;

use crate::callback::{CallbackError, CallbackServer};
use crate::pkce;
use crate::persistence::{Registration, Session, SessionError, SessionStore};
use crate::token::{self, ExchangeRequest, TokenError, TokenSet};

pub const ISSUER: &str = "https://auth.openai.com";
pub const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
/// First-registration entrypoint. Never the id to save or to use for the token exchange.
pub const REGISTRATION_CLIENT_ID: &str = "dynamic_agent_client";
pub const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// Scope that authorizes ChatGPT plan usage; a valid ID token alone does not.
pub const PLAN_USAGE_SCOPE: &str = "chatgpt.tokens.use.direct";
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// First registration must use exactly this redirect (later sign-ins may change the port).
pub const REDIRECT_PORT: u16 = 1455;
pub const REDIRECT_PATH: &str = "/auth/callback";
pub const AGENT_NAME: &str = "A-Eye";
pub const HOST_ID_FILE_ENV: &str = "A_EYE_HOST_ID_FILE";

const URN_UUID_PREFIX: &str = "urn:uuid:";
const UUID_TEXT_LEN: usize = 36;

#[derive(Clone, Debug)]
pub struct PlanUsageConfig {
    pub authorize_url: String,
    pub token_url: String,
    pub issuer: String,
    pub agent_name: String,
    pub host_id: String,
    /// 0 = random free port (tests only; the real first registration needs 1455).
    pub callback_port: u16,
    pub callback_timeout: Duration,
}

impl PlanUsageConfig {
    pub fn production(host_id: String) -> Self {
        PlanUsageConfig {
            authorize_url: AUTHORIZE_URL.into(),
            token_url: TOKEN_URL.into(),
            issuer: ISSUER.into(),
            agent_name: AGENT_NAME.into(),
            host_id,
            callback_port: REDIRECT_PORT,
            callback_timeout: crate::callback::DEFAULT_TIMEOUT,
        }
    }
}

#[derive(Debug)]
pub enum PlanUsageError {
    Callback(CallbackError),
    Token(TokenError),
    /// The callback carried no usable issued client_id: registration is incomplete.
    RegistrationIncomplete(String),
    Config(String),
    Session(SessionError),
    IdToken(IdTokenError),
}

impl std::fmt::Display for PlanUsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanUsageError::Callback(e) => write!(f, "{e}"),
            PlanUsageError::Token(e) => write!(f, "{e}"),
            PlanUsageError::RegistrationIncomplete(m) => write!(f, "registration incomplete: {m}"),
            PlanUsageError::Config(m) => write!(f, "configuration error: {m}"),
            PlanUsageError::Session(e) => write!(f, "{e}"),
            PlanUsageError::IdToken(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PlanUsageError {}

/// Non-secret facts read from an ID token. Signature NOT verified (see module docs).
#[derive(Debug, PartialEq, Eq)]
pub struct IdTokenInfo {
    pub alg: String,
    pub kid: Option<String>,
    pub expires_at_unix: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdTokenError {
    Missing,
    Malformed(&'static str),
    IssuerMismatch,
    AudienceMismatch,
    NonceMismatch,
    Expired,
}

impl std::fmt::Display for IdTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdTokenError::Missing => f.write_str("no id_token in the token response"),
            IdTokenError::Malformed(why) => write!(f, "id_token is malformed ({why})"),
            IdTokenError::IssuerMismatch => f.write_str("id_token iss does not match the issuer"),
            IdTokenError::AudienceMismatch => f.write_str("id_token aud does not contain the issued client_id"),
            IdTokenError::NonceMismatch => f.write_str("id_token nonce does not match the one sent"),
            IdTokenError::Expired => f.write_str("id_token is expired"),
        }
    }
}

pub struct PlanUsageOutcome {
    pub tokens: TokenSet,
    pub issued_client_id: String,
    pub redirect_uri: String,
    /// Whether the token response's granted scopes include [`PLAN_USAGE_SCOPE`].
    pub plan_usage_granted: bool,
    pub id_token: Result<IdTokenInfo, IdTokenError>,
}

/// Persist the issued client id and every token field only after the existing
/// claim checks pass. Signature verification remains unimplemented in this spike.
pub fn run_and_store(
    cfg: &PlanUsageConfig,
    store: &SessionStore<'_>,
    present: impl FnOnce(&url::Url),
) -> Result<PlanUsageOutcome, PlanUsageError> {
    let out = run(cfg, present)?;
    if let Err(e) = &out.id_token {
        return Err(PlanUsageError::IdToken(e.clone()));
    }
    let session = Session::new(Registration {
        token_url: cfg.token_url.clone(), client_id: out.issued_client_id.clone(),
        resource: Some(RESOURCE.into()), encoding: token::RefreshEncoding::Form,
    }, out.tokens.clone());
    store.save(&session).map_err(PlanUsageError::Session)?;
    Ok(out)
}

/// Authorize URL for a first registration. Every value is percent-encoded by `url`.
pub fn build_authorize_url(
    cfg: &PlanUsageConfig,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    nonce: &str,
) -> Result<url::Url, PlanUsageError> {
    let mut u = url::Url::parse(&cfg.authorize_url)
        .map_err(|e| PlanUsageError::Config(format!("authorize URL is not a valid URL: {e}")))?;
    u.query_pairs_mut()
        .append_pair("client_id", REGISTRATION_CLIENT_ID)
        .append_pair("agent_name_hint", &cfg.agent_name)
        .append_pair("ext_agent_host_id", &cfg.host_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("resource", RESOURCE)
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", challenge);
    Ok(u)
}

/// Run a first registration + sign-in. `present` receives the authorize URL once the
/// loopback listener is already up.
pub fn run(cfg: &PlanUsageConfig, present: impl FnOnce(&url::Url)) -> Result<PlanUsageOutcome, PlanUsageError> {
    let pkce = pkce::generate();
    let state = pkce::generate_state();
    let nonce = pkce::generate_state();

    let server = CallbackServer::bind(cfg.callback_port)
        .map_err(PlanUsageError::Callback)?
        .with_path(REDIRECT_PATH);
    let redirect_uri = server.redirect_uri();

    present(&build_authorize_url(cfg, &redirect_uri, &pkce.challenge, &state, &nonce)?);

    let cb = server.wait_callback(&state, cfg.callback_timeout).map_err(PlanUsageError::Callback)?;
    let issued_client_id = match cb.client_id.as_deref() {
        None | Some("") => {
            return Err(PlanUsageError::RegistrationIncomplete("the callback carried no client_id".into()));
        }
        Some(REGISTRATION_CLIENT_ID) => {
            return Err(PlanUsageError::RegistrationIncomplete(
                "the callback echoed the registration entrypoint, not an issued client_id".into(),
            ));
        }
        Some(id) => id.to_string(),
    };

    let client = token::http_client().map_err(PlanUsageError::Token)?;
    let tokens = token::exchange_code_with_params(
        &client,
        &ExchangeRequest {
            token_url: &cfg.token_url,
            client_id: &issued_client_id,
            code: &cb.code,
            code_verifier: &pkce.verifier,
            redirect_uri: &redirect_uri,
        },
        &[("resource", RESOURCE)],
    )
    .map_err(PlanUsageError::Token)?;

    let plan_usage_granted = scope_contains(tokens.scope.as_deref(), PLAN_USAGE_SCOPE);
    let id_token = match tokens.id_token.as_ref() {
        Some(t) => validate_id_token(t.expose(), &cfg.issuer, &issued_client_id, &nonce, SystemTime::now()),
        None => Err(IdTokenError::Missing),
    };
    Ok(PlanUsageOutcome { tokens, issued_client_id, redirect_uri, plan_usage_granted, id_token })
}

pub fn has_plan_usage_scope(tokens: &TokenSet) -> bool {
    scope_contains(tokens.scope.as_deref(), PLAN_USAGE_SCOPE)
}

fn scope_contains(scope: Option<&str>, wanted: &str) -> bool {
    scope.is_some_and(|s| s.split_whitespace().any(|x| x == wanted))
}

/// Check iss, aud, nonce and exp of an ID token. Does NOT check the signature.
pub fn validate_id_token(
    jwt: &str,
    issuer: &str,
    client_id: &str,
    nonce: &str,
    now: SystemTime,
) -> Result<IdTokenInfo, IdTokenError> {
    let mut parts = jwt.split('.');
    let (Some(h), Some(p), Some(_sig), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(IdTokenError::Malformed("not three dot-separated parts"));
    };
    let decode = |seg: &str, what: &'static str| -> Result<serde_json::Value, IdTokenError> {
        let bytes = URL_SAFE_NO_PAD.decode(seg).map_err(|_| IdTokenError::Malformed(what))?;
        serde_json::from_slice(&bytes).map_err(|_| IdTokenError::Malformed(what))
    };
    let header = decode(h, "header is not base64url JSON")?;
    let claims = decode(p, "payload is not base64url JSON")?;

    if claims["iss"].as_str() != Some(issuer) {
        return Err(IdTokenError::IssuerMismatch);
    }
    let aud_ok = match &claims["aud"] {
        serde_json::Value::String(a) => a == client_id,
        serde_json::Value::Array(list) => list.iter().any(|a| a.as_str() == Some(client_id)),
        _ => false,
    };
    if !aud_ok {
        return Err(IdTokenError::AudienceMismatch);
    }
    if claims["nonce"].as_str() != Some(nonce) {
        return Err(IdTokenError::NonceMismatch);
    }
    let exp = claims["exp"].as_u64().ok_or(IdTokenError::Malformed("exp is not a number"))?;
    let now_unix = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if exp <= now_unix {
        return Err(IdTokenError::Expired);
    }
    Ok(IdTokenInfo {
        alg: header["alg"].as_str().unwrap_or("unknown").to_string(),
        kid: header["kid"].as_str().map(str::to_string),
        expires_at_unix: exp,
    })
}

/// `$A_EYE_HOST_ID_FILE`, else `~/.config/a-eye/host-id` (OAUTH.md config location).
pub fn host_id_path() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os(HOST_ID_FILE_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty()).ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/a-eye/host-id"))
}

fn valid_host_id(s: &str) -> bool {
    s.strip_prefix(URN_UUID_PREFIX)
        .is_some_and(|u| u.len() == UUID_TEXT_LEN && u.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}

fn new_host_id() -> String {
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{URN_UUID_PREFIX}{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Reuse the persisted host id, or create and persist one. The id is opaque and not a
/// credential, but it must stay stable per host, so a corrupt file is an error rather than
/// something to overwrite silently. Returns `(id, created)`.
pub fn load_or_create_host_id(path: &Path) -> std::io::Result<(String, bool)> {
    use std::io::Write;

    match std::fs::read_to_string(path) {
        Ok(text) => {
            let id = text.trim().to_string();
            return if valid_host_id(&id) {
                Ok((id, false))
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{} does not hold a urn:uuid host id; fix or remove it", path.display()),
                ))
            };
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let id = new_host_id();
    let mut open_opts = std::fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(0o600);
    }
    let mut f = open_opts.open(path)?;
    #[cfg(windows)]
    {
        // Best-effort lockdown to the current user; Windows has no POSIX mode bit.
        // ACL hardening is a follow-up (tracked alongside the Windows platform pivot),
        // not a correctness requirement for this spike.
    }
    writeln!(f, "{id}")?;
    Ok((id, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::simulate_browser;
    use crate::mock_plan::{PlanMock, PlanMode, issue_jwt};

    const ISS: &str = "https://issuer.test";
    const CID: &str = "oaiapp_SYNTHETIC";

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn jwt(claims: serde_json::Value) -> String {
        issue_jwt(&claims)
    }

    fn good_claims() -> serde_json::Value {
        serde_json::json!({"iss": ISS, "aud": CID, "nonce": "n1", "exp": 1_003_600u64, "sub": "SYNTHETIC-sub"})
    }

    #[test]
    fn authorize_url_matches_the_documented_parameter_set() {
        let cfg = PlanUsageConfig::production("urn:uuid:00000000-0000-4000-8000-000000000000".into());
        let u = build_authorize_url(&cfg, "http://127.0.0.1:1455/auth/callback", "CH", "ST", "NO").unwrap();
        assert_eq!(u.as_str().split('?').next(), Some("https://auth.openai.com/api/accounts/authorize"));
        let q: std::collections::BTreeMap<_, _> = u.query_pairs().into_owned().collect();
        let expect = [
            ("client_id", "dynamic_agent_client"),
            ("agent_name_hint", "A-Eye"),
            ("ext_agent_host_id", "urn:uuid:00000000-0000-4000-8000-000000000000"),
            ("response_type", "code"),
            ("redirect_uri", "http://127.0.0.1:1455/auth/callback"),
            ("scope", "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"),
            ("resource", "https://api.openai.com/v1"),
            ("state", "ST"),
            ("nonce", "NO"),
            ("code_challenge_method", "S256"),
            ("code_challenge", "CH"),
        ];
        assert_eq!(q.len(), expect.len(), "unexpected extra/missing parameters: {q:?}");
        for (k, v) in expect {
            assert_eq!(q.get(k).map(String::as_str), Some(v), "{k}");
        }
        assert!(!q.contains_key("code_verifier"));
    }

    #[test]
    fn id_token_accepts_valid_claims_with_string_or_array_aud() {
        let info = validate_id_token(&jwt(good_claims()), ISS, CID, "n1", now()).unwrap();
        assert_eq!(info.alg, "RS256");
        assert_eq!(info.expires_at_unix, 1_003_600);
        let mut c = good_claims();
        c["aud"] = serde_json::json!(["other", CID]);
        assert!(validate_id_token(&jwt(c), ISS, CID, "n1", now()).is_ok());
    }

    #[test]
    fn id_token_rejects_each_bad_claim() {
        let check = |mutate: fn(&mut serde_json::Value), want: IdTokenError| {
            let mut c = good_claims();
            mutate(&mut c);
            assert_eq!(validate_id_token(&jwt(c), ISS, CID, "n1", now()).unwrap_err(), want);
        };
        check(|c| c["iss"] = "https://evil.test".into(), IdTokenError::IssuerMismatch);
        check(|c| c["aud"] = "someone-else".into(), IdTokenError::AudienceMismatch);
        check(|c| c["aud"] = serde_json::json!(["a", "b"]), IdTokenError::AudienceMismatch);
        check(|c| c["nonce"] = "replayed".into(), IdTokenError::NonceMismatch);
        check(|c| c["exp"] = 999_999u64.into(), IdTokenError::Expired);
        check(|c| c["exp"] = "soon".into(), IdTokenError::Malformed("exp is not a number"));
        for bad in ["", "a.b", "a.b.c.d", "!!.!!.sig"] {
            assert!(matches!(validate_id_token(bad, ISS, CID, "n1", now()), Err(IdTokenError::Malformed(_))), "{bad}");
        }
    }

    #[test]
    fn scope_check_is_exact_token_match() {
        assert!(scope_contains(Some("openid chatgpt.tokens.use.direct"), PLAN_USAGE_SCOPE));
        assert!(!scope_contains(Some("openid chatgpt.tokens.use.directly"), PLAN_USAGE_SCOPE));
        assert!(!scope_contains(Some("openid"), PLAN_USAGE_SCOPE));
        assert!(!scope_contains(None, PLAN_USAGE_SCOPE));
    }

    #[cfg(unix)]
    #[test]
    fn host_id_is_created_once_then_reused_and_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("a-eye-hostid-{}", std::process::id()));
        let path = dir.join("nested/host-id");
        let (first, created) = load_or_create_host_id(&path).unwrap();
        assert!(created && valid_host_id(&first), "{first}");
        let (second, created) = load_or_create_host_id(&path).unwrap();
        assert!(!created);
        assert_eq!(first, second);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::write(&path, "not-a-urn\n").unwrap();
        assert_eq!(load_or_create_host_id(&path).unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn cfg(m: &PlanMock, timeout_ms: u64) -> PlanUsageConfig {
        PlanUsageConfig {
            authorize_url: m.authorize_url(),
            token_url: m.token_url(),
            issuer: m.issuer(),
            agent_name: "A-Eye".into(),
            host_id: "urn:uuid:11111111-1111-4111-8111-111111111111".into(),
            callback_port: 0,
            callback_timeout: Duration::from_millis(timeout_ms),
        }
    }

    type Driven = (Result<PlanUsageOutcome, PlanUsageError>, Option<Result<u16, String>>);

    fn drive(m: &PlanMock, timeout_ms: u64) -> Driven {
        let mut h = None;
        let r = run(&cfg(m, timeout_ms), |u| {
            let u = u.to_string();
            h = Some(std::thread::spawn(move || simulate_browser(&u)));
        });
        (r, h.and_then(|h| h.join().ok()))
    }

    #[test]
    fn full_flow_against_synthetic_plan_usage_provider() {
        let m = PlanMock::start(PlanMode::Approve).unwrap();
        let (r, browser) = drive(&m, 5000);
        assert_eq!(browser, Some(Ok(200)));
        let out = r.unwrap();
        assert!(out.issued_client_id.starts_with("oaiapp_SYNTHETIC"));
        assert!(out.redirect_uri.ends_with("/auth/callback"), "{}", out.redirect_uri);
        assert!(out.plan_usage_granted);
        assert!(out.id_token.is_ok(), "{:?}", out.id_token);
        let events = m.events();
        for want in ["host id seen", "registration entrypoint accepted", "token ok: PKCE S256 verified", "resource sent"] {
            assert!(events.iter().any(|e| e.contains(want)), "missing '{want}' in {events:?}");
        }
        assert!(!events.iter().any(|e| e.contains("rejected")), "{events:?}");
    }

    #[test]
    fn callback_without_issued_client_id_is_registration_incomplete() {
        let m = PlanMock::start(PlanMode::OmitClientId).unwrap();
        let (r, _) = drive(&m, 5000);
        assert!(matches!(r, Err(PlanUsageError::RegistrationIncomplete(_))));
        assert_eq!(m.token_requests(), 0, "no token request without an issued client id");
    }

    #[test]
    fn callback_echoing_the_entrypoint_id_is_registration_incomplete() {
        let m = PlanMock::start(PlanMode::EchoEntrypointId).unwrap();
        let (r, _) = drive(&m, 5000);
        assert!(matches!(r, Err(PlanUsageError::RegistrationIncomplete(_))));
        assert_eq!(m.token_requests(), 0);
    }

    #[test]
    fn missing_plan_scope_keeps_sign_in_but_disables_plan_usage() {
        let m = PlanMock::start(PlanMode::NoPlanScope).unwrap();
        let out = drive(&m, 5000).0.unwrap();
        assert!(!out.plan_usage_granted);
        assert!(out.id_token.is_ok());
    }

    #[test]
    fn replayed_nonce_is_reported_not_trusted() {
        let m = PlanMock::start(PlanMode::WrongNonce).unwrap();
        let out = drive(&m, 5000).0.unwrap();
        assert_eq!(out.id_token.unwrap_err(), IdTokenError::NonceMismatch);
    }

    #[test]
    fn user_denial_never_reaches_the_token_endpoint() {
        let m = PlanMock::start(PlanMode::Deny).unwrap();
        let (r, _) = drive(&m, 5000);
        assert!(matches!(r, Err(PlanUsageError::Callback(CallbackError::Provider { ref error, .. })) if error == "access_denied"));
        assert_eq!(m.token_requests(), 0);
    }

    #[test]
    fn busy_callback_port_is_a_clear_bind_error() {
        let held = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let m = PlanMock::start(PlanMode::Approve).unwrap();
        let mut c = cfg(&m, 300);
        c.callback_port = held.local_addr().unwrap().port();
        let err = run(&c, |_| {}).err().unwrap();
        assert!(matches!(err, PlanUsageError::Callback(CallbackError::Bind(_))), "{err}");
    }
}
