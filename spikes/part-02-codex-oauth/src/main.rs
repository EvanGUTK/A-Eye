//! A-Eye spike part 02: Codex-style OAuth 2.0 + PKCE flow.
//!
//! Default run: complete the flow end-to-end against a LOCAL MOCK provider,
//! store synthetic tokens in the OS keyring, and print a capability record.
//! `--live` (opt-in, never run by CI/agents): print the authorization URL for a
//! human to open, using endpoints/client id from the environment.
//! `--live-plan-usage` (opt-in, never run by CI/agents): the same for OpenAI's documented
//! "Sign in with ChatGPT" route for open-source apps; no client id to supply.
//!
//! No real provider is contacted in the default run (AGENTS.md rule 12).
//!
//! Exit status of the default run: 0 when the record was produced and no row is
//! FAIL (UNVERIFIED rows do not change it); 2 when at least one row is FAIL.

mod callback;
mod flow;
mod mock;
#[cfg(test)]
mod mock_plan;
mod pkce;
mod persistence;
mod record;
mod secret;
mod siwc;
mod store;
mod token;

use std::net::{Ipv4Addr, TcpStream};
use std::process::ExitCode;
use std::time::Duration;

use callback::CallbackError;
use flow::{FlowConfig, FlowError};
use mock::{MockProvider, Mode, simulate_browser};
use record::{Check, Status};

const MOCK_CLIENT_ID: &str = "mock-client-id";
const SCOPE: &str = "openid profile email offline_access";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_mock(),
        Some("--live") => run_live(),
        Some("--live-plan-usage") => run_live_plan_usage(),
        Some("--help") | Some("-h") => {
            println!(
                "usage: a-eye-spike-02-codex-oauth [--live]\n\n\
                 (no args)  run the full flow against a local mock provider and print the capability record\n\
                 --live     opt-in: print an authorization URL for a human to open. Requires\n\
                 \x20          A_EYE_OAUTH_CLIENT_ID, A_EYE_OAUTH_AUTH_URL, A_EYE_OAUTH_TOKEN_URL;\n\
                 \x20          optional A_EYE_OAUTH_SCOPE, A_EYE_OAUTH_CALLBACK_PORT, A_EYE_OAUTH_CALLBACK_PATH.\n\
                 \x20          Tokens persist via part 01 Secret Service; startup loads/refreshes them.\n\
                 --live-plan-usage  opt-in: first registration + sign-in through OpenAI's documented\n\
                 \x20          \"Sign in with ChatGPT\" route for open-source apps. Needs no environment\n\
                 \x20          variables; listens on 127.0.0.1:1455 and prints a URL for a human to open.\n\
                 \x20          Tokens persist under a separate account alias; only fingerprints are printed.\n\
                 \x20          A_EYE_OAUTH_ACCOUNT_ID selects a stable alias (defaults: default / plan-usage)."
            );
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument: {other} (try --help)");
            ExitCode::from(2)
        }
    }
}

fn mock_cfg(m: &MockProvider, timeout: Duration) -> FlowConfig {
    FlowConfig {
        authorize_url: m.authorize_url(),
        token_url: m.token_url(),
        client_id: m.client_id().to_string(),
        scope: SCOPE.into(),
        callback_port: 0,
        callback_path: "/callback".into(),
        callback_timeout: timeout,
    }
}

fn port_closed(port: u16) -> bool {
    TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, port).into(), Duration::from_millis(300)).is_err()
}

fn run_mock() -> ExitCode {
    let mut checks: Vec<Check> = Vec::new();

    // 1. PKCE primitives (runtime evidence, not just unit tests).
    let rfc = pkce::challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    checks.push(Check::from_bool(
        "pkce.rfc7636_vector",
        rfc == "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        "RFC 7636 Appendix B verifier -> expected S256 challenge".into(),
        format!("challenge mismatch: {rfc}"),
    ));
    let p = pkce::generate();
    let v = p.verifier.expose();
    checks.push(Check::from_bool(
        "pkce.generated_verifier",
        v.len() == 43 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        format!("32 random bytes -> {} base64url chars, unreserved only, no padding", v.len()),
        format!("unexpected verifier shape (len {})", v.len()),
    ));

    let mock = match MockProvider::start(MOCK_CLIENT_ID) {
        Ok(m) => m,
        Err(e) => {
            checks.push(Check::fail("mock.start", format!("could not start local mock provider: {e}")));
            return finish(checks, None);
        }
    };

    // 2. Happy path.
    let mut browser = None;
    let happy = flow::run(&mock_cfg(&mock, Duration::from_secs(10)), |u| {
        let u = u.to_string();
        browser = Some(std::thread::spawn(move || simulate_browser(&u)));
    });
    let browser_status = browser.map(|h| h.join().ok().and_then(|r| r.ok()));
    let mut tokens_for_store = None;
    match happy {
        Ok(out) => {
            let t = &out.tokens;
            let events = mock.events();
            let pkce_verified = events.iter().any(|e| e.contains("PKCE S256 verified"));
            let ok = browser_status == Some(Some(200))
                && pkce_verified
                && t.refresh_token.is_some()
                && t.expires_in.is_some()
                && out.redirect_uri.starts_with("http://127.0.0.1:");
            checks.push(Check::from_bool(
                "flow.mock_end_to_end",
                ok,
                format!(
                    "authorize(S256+state) -> loopback callback {} -> token exchange. Mock verified PKCE; \
                     access_token len={} fp={}; refresh_token {}; id_token {}; expires_in={}s (expires_at {}); token_type={}; scope=[{}]",
                    out.redirect_uri,
                    t.access_token.expose().len(),
                    t.access_token.fingerprint(),
                    if t.refresh_token.is_some() { "present" } else { "absent" },
                    if t.id_token.is_some() { "present" } else { "absent" },
                    t.expires_in.unwrap_or(0),
                    t.expires_at().and_then(store::iso8601).unwrap_or_else(|| "n/a".into()),
                    t.token_type.as_deref().unwrap_or("n/a"),
                    t.scope.as_deref().unwrap_or("n/a"),
                ),
                format!("flow completed but evidence incomplete: browser={browser_status:?} pkce_verified={pkce_verified}"),
            ));
            checks.push(Check::from_bool(
                "callback.port_freed_after_use",
                port_closed(out.callback_port),
                format!("nothing listening on 127.0.0.1:{} after the callback", out.callback_port),
                format!("port {} still accepting connections", out.callback_port),
            ));
            let dbg = format!("{t:?} {t:#?}");
            let leaked = [Some(&t.access_token), t.refresh_token.as_ref(), t.id_token.as_ref()]
                .into_iter()
                .flatten()
                .any(|s| dbg.contains(s.expose()));
            checks.push(Check::from_bool(
                "secrets.debug_redaction",
                !leaked,
                "Debug/pretty-Debug of TokenSet contains no token value".into(),
                "a token value appeared in Debug output".into(),
            ));
            tokens_for_store = Some(out.tokens);
        }
        Err(e) => checks.push(Check::fail("flow.mock_end_to_end", format!("{e}"))),
    }

    // 3. Negative: tampered state must never reach the token endpoint.
    mock.set_mode(Mode::TamperState);
    let before = mock.token_requests();
    let mut h = None;
    let r = flow::run(&mock_cfg(&mock, Duration::from_millis(1200)), |u| {
        let u = u.to_string();
        h = Some(std::thread::spawn(move || simulate_browser(&u)));
    });
    let cb_status = h.and_then(|h| h.join().ok()).and_then(|r| r.ok());
    checks.push(Check::from_bool(
        "callback.rejects_state_mismatch",
        matches!(r, Err(FlowError::Callback(CallbackError::Timeout(_))))
            && cb_status == Some(400)
            && mock.token_requests() == before,
        "forged state answered HTTP 400, no token request made, wait ended in timeout (not accepted)".into(),
        format!("unexpected: result_ok={} callback_http={cb_status:?}", r.is_ok()),
    ));

    // 4. Negative: user denial.
    mock.set_mode(Mode::Deny);
    let mut h = None;
    let r = flow::run(&mock_cfg(&mock, Duration::from_secs(5)), |u| {
        let u = u.to_string();
        h = Some(std::thread::spawn(move || simulate_browser(&u)));
    });
    let _ = h.map(|h| h.join());
    checks.push(Check::from_bool(
        "flow.provider_denial_surfaces_error",
        matches!(&r, Err(FlowError::Callback(CallbackError::Provider { error, .. })) if error == "access_denied"),
        "provider error=access_denied (with valid state) returned as a clear Err".into(),
        "denial was not reported as a provider error".into(),
    ));

    // 5. Negative: nobody completes the flow -> timeout Err, no panic.
    mock.set_mode(Mode::Approve);
    let t0 = std::time::Instant::now();
    let r = flow::run(&mock_cfg(&mock, Duration::from_secs(1)), |_| {});
    checks.push(Check::from_bool(
        "callback.timeout_returns_err",
        matches!(r, Err(FlowError::Callback(CallbackError::Timeout(_)))),
        format!("no callback -> Err(Timeout) after {:.1}s (configurable; default 300s)", t0.elapsed().as_secs_f64()),
        "timeout path did not return Err(Timeout)".into(),
    ));

    // 6. Credential store (synthetic tokens only).
    match tokens_for_store {
        Some(t) => match store::roundtrip(&t, store::KEYRING_TIMEOUT) {
            store::StoreReport::Verified { account, steps } => checks.push(Check::pass(
                "store.keyring_roundtrip",
                format!("service={} account={} (temporary; deleted). {}", store::SERVICE, account, steps.join("; ")),
            )),
            store::StoreReport::Skipped { reason } => {
                checks.push(Check::unverified("store.keyring_roundtrip", format!("UNVERIFIED: store skipped ({reason})")))
            }
            store::StoreReport::Failed { reason, steps } => checks.push(Check::fail(
                "store.keyring_roundtrip",
                format!("{reason}. Completed: {}", steps.join("; ")),
            )),
        },
        None => checks.push(Check::unverified("store.keyring_roundtrip", "UNVERIFIED: no tokens because the flow failed")),
    }

    finish(checks, Some(&mock))
}

/// Static, honest statements about what this spike did NOT verify.
fn unverified_checks() -> Vec<Check> {
    vec![
        Check::unverified(
            "live.codex_flow",
            "not run: needs interactive browser consent on a real ChatGPT account; --live-plan-usage (documented route) and --live exist but were not executed by an agent. \
             Furthest verified without a human: the real binary printed the authorize URL, its loopback listener rejected bad state/path and accepted a fake code, and the real token endpoint answered it with invalid_grant",
        ),
        Check::unverified(
            "live.real_endpoints_and_client_id",
            "the default run sends nothing to auth.openai.com (a separate manual run on 2026-10-18 sent one token POST with a fake code -> invalid_grant, and no authorize request). Endpoints/redirect/scope below are from openai/codex source \
             (see 'what failed or is unknown'); the public client_id is deliberately not hardcoded here",
        ),
        Check::unverified(
            "live.token_shape_and_lifetime",
            "real access/refresh/id token format and lifetime unknown; mock values are synthetic",
        ),
        Check::unverified(
            "codex.app_server_turn",
            "out of scope for this spike (AE-001-L step 'start app-server and send a turn' is a later part)",
        ),
        Check::unverified(
            "store.encrypted_file_fallback",
            "not implemented here; spike writes nothing to disk when Secret Service is unusable",
        ),
    ]
}

fn finish(mut checks: Vec<Check>, mock: Option<&MockProvider>) -> ExitCode {
    checks.extend(unverified_checks());

    let codex_cli = record::command_first_line("codex", &["--version"], Duration::from_secs(5))
        .unwrap_or_else(|| "not found on PATH (or did not answer within 5s)".into());
    let rustc = record::command_first_line("rustc", &["--version"], Duration::from_secs(5))
        .unwrap_or_else(|| "unknown".into());

    println!("=== A-Eye spike part 02: Codex OAuth 2.0 + PKCE capability record ===");
    println!("mode            : MOCK provider only (no real provider call was made)");
    println!("exit status     : 0 = record produced and no FAIL row (UNVERIFIED does not change it); 2 = at least one FAIL row");
    println!("os              : {} (kernel {})", record::os_pretty_name(), record::kernel());
    println!("session         : {}", record::session_summary());
    println!("rustc           : {rustc}");
    println!("codex cli       : {codex_cli} (version string only; the CLI was not used for auth)");
    println!(
        "crates (locked) : {}",
        record::locked_versions(&["base64", "keyring", "keyring-core", "zbus-secret-service-keyring-store", "rand", "reqwest", "serde", "serde_json", "sha2", "time", "url"]).join(", ")
    );
    println!("storage backend : Secret Service (org.freedesktop.secrets) via keyring v1 feature");
    println!("token lifetime  : mock only (expires_in=3600); real lifetime UNVERIFIED");
    println!();
    println!("checks:");
    record::print_table(&checks);
    println!();
    if let Some(m) = mock {
        println!("mock provider event log (non-secret):");
        for e in m.events() {
            println!("  - {e}");
        }
        println!();
    }
    println!("what was proved:");
    println!("  - PKCE S256 generation matches RFC 7636 Appendix B; verifier is 43 unreserved chars");
    println!("  - Full authorization-code + PKCE flow runs end-to-end against a local mock (authorize ->");
    println!("    loopback callback with state validation -> form-encoded token exchange -> parse)");
    println!("  - Forged state is rejected and never reaches the token endpoint; denial and timeout return Err");
    println!("  - Tokens are held in a redacting type; printed output contains only lengths and SHA-256 fingerprints");
    println!("  - Keyring write/readback/delete: see store.keyring_roundtrip row (only that row is evidence)");
    println!();
    println!("what failed or is unknown:");
    println!("  - Live Codex/ChatGPT OAuth flow: UNVERIFIED (not run; requires interactive consent)");
    println!("  - Real auth.openai.com behavior: UNVERIFIED (default run sends nothing; one manual fake-code token POST on 2026-10-18 -> invalid_grant). Documented vs. unverified:");
    println!("    DOCUMENTED (https://developers.openai.com/codex/auth, now learn.chatgpt.com/docs/auth): browser sign-in with");
    println!("      a localhost callback, default port 1455; credentials cached in ~/.codex/auth.json or the OS store");
    println!("      (cli_auth_credentials_store = file|keyring|auto|ephemeral); device-code login exists; tokens auto-refresh.");
    println!("    FROM SOURCE, not documented (github.com/openai/codex codex-rs/login/src/{{server.rs,oauth/*.rs}}, main@6b99197, read 2026-10-15):");
    println!("      authorize = https://auth.openai.com/oauth/authorize ; token = https://auth.openai.com/oauth/token");
    println!("      redirect  = http://127.0.0.1:1455/auth/callback, fallback port 1457; source comment says the ports are kept");
    println!("                  'in sync with the Codex CLI Hydra redirect URI allow-list' => registered URIs; a random port or");
    println!("                  /callback path would likely be rejected for the Codex public client (inferred, UNVERIFIED)");
    println!("      scope     = openid profile email offline_access api.connectors.read api.connectors.invoke");
    println!("      extra authorize params: id_token_add_organizations=true, codex_cli_simplified_flow=true, originator");
    println!("      auth-code exchange = form-encoded; ChatGPT refresh = JSON; Codex PKCE verifier = 64 random bytes (86 chars)");
    println!("      Codex tolerates a state suffix appended by ChatGPT (life-sciences onboarding); this spike requires exact state");
    println!("    docs/architecture/OAUTH.md was corrected from these findings (PR #21); re-checked unchanged at openai/codex 70b3bb6 on 2026-10-18.");
    println!("  - Third-party use of the Codex public client_id by another app: no OpenAI statement found; UNVERIFIED");
    println!("    OpenAI DOES document a route for open-source apps (developers.openai.com/siwc/token-sharing-open-source, read 2026-10-18):");
    println!("      authorize = https://auth.openai.com/api/accounts/authorize with client_id=dynamic_agent_client; the issued client_id comes back on the");
    println!("      callback; token = https://auth.openai.com/api/accounts/oauth/token (+ resource). Endpoints match the provider's OIDC discovery document.");
    println!("      Implemented as --live-plan-usage and tested against a synthetic mock only; see docs/planning/LIVE-AUTH-RUNBOOK.md");
    println!("  - Codex app-server turn (text + image): out of scope, UNVERIFIED");
    println!("  - Encrypted-file fallback when Secret Service is absent: not implemented in this spike");
    println!();
    let (p, f, u) = (
        record::count(&checks, Status::Pass),
        record::count(&checks, Status::Fail),
        record::count(&checks, Status::Unverified),
    );
    println!("summary: {p} PASS, {f} FAIL, {u} UNVERIFIED");
    if f > 0 {
        println!("WARNING: {f} check(s) FAILED - the record above is honest, do not treat this spike as green; exit status 2");
    }
    ExitCode::from(exit_status(f))
}

/// 0 when no check FAILed (UNVERIFIED is not a failure), 2 otherwise.
fn exit_status(failed: usize) -> u8 {
    if failed == 0 { 0 } else { 2 }
}

fn run_live() -> ExitCode {
    let need = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(client_id), Some(auth_url), Some(token_url)) =
        (need("A_EYE_OAUTH_CLIENT_ID"), need("A_EYE_OAUTH_AUTH_URL"), need("A_EYE_OAUTH_TOKEN_URL"))
    else {
        eprintln!("--live needs A_EYE_OAUTH_CLIENT_ID, A_EYE_OAUTH_AUTH_URL and A_EYE_OAUTH_TOKEN_URL");
        return ExitCode::from(2);
    };
    for (name, u) in [("A_EYE_OAUTH_AUTH_URL", &auth_url), ("A_EYE_OAUTH_TOKEN_URL", &token_url)] {
        if let Err(e) = flow::validate_endpoint(u) {
            eprintln!("{name} rejected: {e}");
            return ExitCode::from(2);
        }
    }
    let port: u16 = match need("A_EYE_OAUTH_CALLBACK_PORT") {
        Some(p) => match p.parse() {
            Ok(p) => p,
            Err(_) => {
                eprintln!("A_EYE_OAUTH_CALLBACK_PORT must be a port number");
                return ExitCode::from(2);
            }
        },
        None => 0,
    };
    let cfg = FlowConfig {
        authorize_url: auth_url,
        token_url,
        client_id,
        scope: need("A_EYE_OAUTH_SCOPE").unwrap_or_else(|| SCOPE.into()),
        callback_port: port,
        callback_path: need("A_EYE_OAUTH_CALLBACK_PATH").unwrap_or_else(|| callback::DEFAULT_PATH.into()),
        callback_timeout: callback::DEFAULT_TIMEOUT,
    };
    let _lock = match oauth_lock() { Ok(lock) => lock, Err(code) => return code };
    let backend = spike_01_credential_store::backend::SecretServiceBackend::new(store::KEYRING_TIMEOUT);
    let store = match session_store(&backend, "default") { Ok(store) => store, Err(code) => return code };
    let res = flow::run_persisted(&cfg, &store, |u| {
        println!("Open this URL yourself (waiting up to 5 minutes):\n{u}\n");
    });
    match res {
        Ok(out) => {
            let t = out.tokens;
            println!("OAuth session ready; tokens persisted and read back through part 01 Secret Service");
            println!("  access_token  len={} fp={}", t.access_token.expose().len(), t.access_token.fingerprint());
            println!("  refresh_token {}", t.refresh_token.map_or("absent".into(), |s| format!("present fp={}", s.fingerprint())));
            println!("  id_token      {}", t.id_token.map_or("absent".into(), |s| format!("present fp={}", s.fingerprint())));
            println!("  expires_in    {:?}s", t.expires_in);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("live flow failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn oauth_lock() -> Result<std::fs::File, ExitCode> {
    persistence::lock_path().and_then(|path| persistence::lock(&path)).map_err(|e| {
        eprintln!("{e}");
        ExitCode::FAILURE
    })
}

fn session_store<'a>(
    backend: &'a dyn spike_01_credential_store::backend::CredentialBackend,
    default_alias: &str,
) -> Result<persistence::SessionStore<'a>, ExitCode> {
    let alias = std::env::var("A_EYE_OAUTH_ACCOUNT_ID").unwrap_or_else(|_| default_alias.into());
    persistence::SessionStore::new(backend, &alias).map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(2)
    })
}

/// Resolve the persisted host id, then run the plan-usage sign-in and print a PASS/FAIL table.
fn run_live_plan_usage() -> ExitCode {
    let _lock = match oauth_lock() { Ok(lock) => lock, Err(code) => return code };
    let backend = spike_01_credential_store::backend::SecretServiceBackend::new(store::KEYRING_TIMEOUT);
    let store = match session_store(&backend, "plan-usage") { Ok(store) => store, Err(code) => return code };
    match store.resume(siwc::TOKEN_URL, None, Some(siwc::RESOURCE), token::RefreshEncoding::Form,
        std::time::SystemTime::now()) {
        Ok(Some(session)) => {
            println!("Stored plan-usage session loaded/refreshed; no browser sign-in needed.");
            let plan_usage_granted = siwc::has_plan_usage_scope(&session.tokens);
            println!("access_token fp={}; expires_in={:?}s; plan_usage_scope={}",
                session.tokens.access_token.fingerprint(), session.tokens.expires_in, plan_usage_granted);
            println!("ID token signature remains UNVERIFIED; this run did not recheck sign-in claims.");
            return ExitCode::from(exit_status(usize::from(!plan_usage_granted)));
        }
        Ok(None) => {}
        Err(e) => { eprintln!("{e}"); return ExitCode::FAILURE; }
    }
    let path = match siwc::host_id_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot locate the host id file: {e} (set {})", siwc::HOST_ID_FILE_ENV);
            return ExitCode::from(2);
        }
    };
    let (host_id, created) = match siwc::load_or_create_host_id(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("host id file {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    println!("host id         : {host_id} ({} {})", if created { "created" } else { "reused from" }, path.display());
    println!("registering as  : {} (agent_name_hint), client_id=dynamic_agent_client", siwc::AGENT_NAME);
    println!("A-Eye will NOT open a browser. Open the URL below yourself; waiting up to 5 minutes on 127.0.0.1:{}.\n", siwc::REDIRECT_PORT);

    let cfg = siwc::PlanUsageConfig::production(host_id);
    match siwc::run_and_store(&cfg, &store, |u| println!("{u}\n")) {
        Ok(out) => print_plan_usage_result(&out),
        Err(e) => {
            eprintln!("live plan-usage flow failed: {e}");
            if matches!(e, siwc::PlanUsageError::Callback(callback::CallbackError::Bind(_))) {
                eprintln!(
                    "hint: port {} is busy (a running `codex login` also uses it). The first registration needs exactly this port.",
                    siwc::REDIRECT_PORT
                );
            }
            ExitCode::FAILURE
        }
    }
}

fn print_plan_usage_result(out: &siwc::PlanUsageOutcome) -> ExitCode {
    let t = &out.tokens;
    let mut checks = vec![
        Check::pass(
            "registration.issued_client_id",
            format!(
                "callback carried an issued client_id (prefix {}, len {})",
                out.issued_client_id.chars().take(7).collect::<String>(),
                out.issued_client_id.len()
            ),
        ),
        Check::pass(
            "token.exchange",
            format!(
                "access_token len={} fp={}; refresh_token {}; expires_in={:?}s; token_type={}; scope=[{}]",
                t.access_token.expose().len(),
                t.access_token.fingerprint(),
                t.refresh_token.as_ref().map_or("absent".into(), |s| format!("present fp={}", s.fingerprint())),
                t.expires_in,
                t.token_type.as_deref().unwrap_or("n/a"),
                t.scope.as_deref().unwrap_or("n/a"),
            ),
        ),
        Check::from_bool(
            "token.plan_usage_scope",
            out.plan_usage_granted,
            format!("{} is in the granted scopes", siwc::PLAN_USAGE_SCOPE),
            format!("{} is NOT in the granted scopes: sign-in works but plan usage is disabled", siwc::PLAN_USAGE_SCOPE),
        ),
    ];
    checks.push(match &out.id_token {
        Ok(info) => Check::pass(
            "id_token.claims",
            format!("iss, aud (= issued client_id), nonce and exp valid; alg={} kid={}; expires_at_unix={}", info.alg, info.kid.as_deref().unwrap_or("n/a"), info.expires_at_unix),
        ),
        Err(e) => Check::fail("id_token.claims", e.to_string()),
    });
    checks.push(Check::unverified(
        "id_token.signature",
        format!("not checked: needs the provider JWKS ({}/.well-known/jwks.json)", siwc::ISSUER),
    ));

    println!("redirect used   : {}", out.redirect_uri);
    println!("\nchecks:");
    record::print_table(&checks);
    println!("\ntokens persisted and read back through part 01 Secret Service; nothing above contains a token value.");
    let failed = record::count(&checks, Status::Fail);
    println!("summary: {} PASS, {failed} FAIL, {} UNVERIFIED", record::count(&checks, Status::Pass), record::count(&checks, Status::Unverified));
    ExitCode::from(exit_status(failed))
}

#[cfg(test)]
mod tests {
    use super::exit_status;

    #[test]
    fn exit_status_is_nonzero_only_for_fail_rows() {
        assert_eq!(exit_status(0), 0);
        assert_eq!(exit_status(1), 2);
        assert_eq!(exit_status(7), 2);
    }
}
