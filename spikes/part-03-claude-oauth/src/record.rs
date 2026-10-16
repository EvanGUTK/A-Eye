//! Capability record: runs every check and prints evidence. No false greens:
//! anything not actually exercised is UNVERIFIED, and a documented policy
//! conflict is reported as FAIL.

use crate::error::FlowError;
use crate::handoff::{run_handoff, StubMode, TOKEN_ENV};
use crate::mock::{self, Behavior};
use crate::pkce::{challenge_for, Pkce};
use crate::secret::Secret;
use crate::store;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Exact pins; `pins_match_cargo_toml` below keeps this list honest.
pub const CRATE_PINS: &[(&str, &str)] = &[
    ("keyring", "4.2.0"),
    ("sha2", "0.11.1"),
    ("base64", "0.23.1"),
    ("getrandom", "0.4.3"),
    ("ureq", "3.4.2"),
    ("url", "2.5.8"),
    ("serde_json", "1.0.154"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Unverified,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Unverified => "UNVERIFIED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A property of this code. A FAIL here makes `cargo run` exit non-zero.
    Local,
    /// A finding about the outside world / design assumptions. Reported, never gates the exit code.
    Finding,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub id: &'static str,
    pub name: &'static str,
    pub status: Status,
    pub kind: Kind,
    pub reason: String,
}

fn check(id: &'static str, name: &'static str, kind: Kind, r: Result<String, String>) -> Check {
    match r {
        Ok(reason) => Check { id, name, status: Status::Pass, kind, reason },
        Err(reason) => Check { id, name, status: Status::Fail, kind, reason },
    }
}

fn unverified(id: &'static str, name: &'static str, reason: &str) -> Check {
    Check { id, name, status: Status::Unverified, kind: Kind::Finding, reason: reason.into() }
}

fn cmd_out(program: &str, args: &[&str]) -> Option<String> {
    let (p, a): (String, Vec<String>) = (program.into(), args.iter().map(|s| s.to_string()).collect());
    store::with_timeout(Duration::from_secs(15), move || {
        Command::new(p).args(a).output().ok().filter(|o| o.status.success()).map(|o| {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        })
    })
    .ok()
    .flatten()
}

fn os_pretty_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

// ---- individual checks ------------------------------------------------------

fn c_rfc_vector() -> Result<String, String> {
    let got = challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    if got == "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM" {
        Ok("RFC 7636 Appendix B verifier -> challenge matches".into())
    } else {
        Err(format!("challenge mismatch: {got}"))
    }
}

fn c_verifier_shape() -> Result<String, String> {
    let p = Pkce::generate()?;
    let v = p.verifier.expose();
    let ok = v.len() == 43 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let distinct = Pkce::generate()?.verifier.expose() != v;
    if ok && distinct {
        Ok("32 random bytes -> 43-char base64url, no padding; values differ per call".into())
    } else {
        Err(format!("bad verifier shape (len={})", v.len()))
    }
}

fn c_mock_e2e() -> Result<String, String> {
    let (res, obs) = mock::run_against_mock(Behavior::Normal, Duration::from_secs(3), true);
    let t = res.map_err(|e| e.to_string())?;
    let redacted = {
        let d = format!("{t:?}");
        !d.contains("SYNTH-ACCESS") && !d.contains("SYNTH-REFRESH")
    };
    let good = t.access_token.expose().starts_with("SYNTH-ACCESS-")
        && t.refresh_token.as_ref().is_some_and(|r| r.expose().starts_with("SYNTH-REFRESH-"))
        && t.expires_in == Some(3600)
        && obs.saw_s256
        && obs.saw_form_encoded
        && obs.pkce_verified
        && obs.redirect_uri_matched
        && redacted;
    if good {
        Ok("authorize(S256+state) -> loopback callback -> form POST /token -> access/refresh/expires_in parsed; mock verified PKCE".into())
    } else {
        Err(format!("flow ran but evidence incomplete: {obs:?}"))
    }
}

fn c_mock_rejects_bad_verifier() -> Result<String, String> {
    let mock = mock::MockProvider::start(Behavior::Normal).map_err(|e| e.to_string())?;
    let p = Pkce::generate()?;
    let state = "s";
    let cfg = crate::flow::FlowConfig {
        client_id: mock::MOCK_CLIENT_ID.into(),
        auth_url: mock.auth_url(),
        token_url: mock.token_url(),
        scope: mock::MOCK_SCOPE.into(),
        callback_timeout: Duration::from_secs(1),
        http_timeout: Duration::from_secs(5),
    };
    let redirect = "http://127.0.0.1:9/callback";
    let url = crate::flow::build_authorize_url(&cfg, redirect, &p.challenge, state).map_err(|e| e.to_string())?;
    let loc = mock::authorize_no_follow(url.as_str())?.ok_or("no redirect from mock")?;
    let code = url::Url::parse(&loc)
        .map_err(|e| e.to_string())?
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| Secret::new(v.into_owned()))
        .ok_or("no code in redirect")?;
    let wrong = Pkce::generate()?;
    let bad = crate::token::exchange_code(&cfg.token_url, &cfg.client_id, &code, &wrong.verifier, redirect, cfg.http_timeout);
    match bad {
        Err(FlowError::TokenEndpoint(e)) if e == "invalid_grant" => {}
        other => return Err(format!("wrong verifier was not rejected: {:?}", other.map(|_| "TOKENS ISSUED"))),
    }
    // The correct verifier still works once, and a replay is rejected.
    crate::token::exchange_code(&cfg.token_url, &cfg.client_id, &code, &p.verifier, redirect, cfg.http_timeout)
        .map_err(|e| format!("correct verifier rejected: {e}"))?;
    let replay = crate::token::exchange_code(&cfg.token_url, &cfg.client_id, &code, &p.verifier, redirect, cfg.http_timeout);
    if replay.is_ok() {
        return Err("code replay was accepted".into());
    }
    let o = mock.observations();
    Ok(format!(
        "mock rejects wrong code_verifier ({}x) and code replay ({}x): e2e PASS is not vacuous",
        o.bad_verifier_rejected, o.replay_rejected
    ))
}

fn c_negative_flows() -> Result<String, String> {
    let cases: [(Behavior, FlowError); 4] = [
        (Behavior::WrongState, FlowError::StateMismatch),
        (Behavior::MissingCode, FlowError::MissingCode),
        (Behavior::AccessDenied, FlowError::ProviderDenied("access_denied".into())),
        (Behavior::TokenInvalidGrant, FlowError::TokenEndpoint("invalid_grant".into())),
    ];
    for (b, want) in cases {
        let (res, _) = mock::run_against_mock(b, Duration::from_secs(3), true);
        match res {
            Err(e) if e == want => {}
            Err(e) => return Err(format!("{b:?}: expected {want:?}, got {e:?}")),
            Ok(_) => return Err(format!("{b:?}: flow wrongly succeeded")),
        }
    }
    let t0 = std::time::Instant::now();
    let (res, _) = mock::run_against_mock(Behavior::Normal, Duration::from_secs(1), false);
    if res.err() != Some(FlowError::Timeout) {
        return Err("no-browser run did not time out".into());
    }
    if t0.elapsed() > Duration::from_secs(4) {
        return Err("timeout took too long".into());
    }
    Ok("tampered state, missing code, access_denied, token invalid_grant and 1s callback timeout all rejected with stable errors".into())
}

fn c_redaction() -> Result<String, String> {
    let p = Pkce::generate()?;
    let rendered = format!("{p:?}");
    if rendered.contains(p.verifier.expose()) {
        return Err("PKCE verifier leaked via Debug".into());
    }
    let s = Secret::new("SYNTH-XYZ");
    if format!("{s:?} {s:#?}").contains("SYNTH-XYZ") {
        return Err("Secret Debug leaked".into());
    }
    Ok("Debug of Secret / Pkce / TokenSet prints [REDACTED]".into())
}

/// Returns the check plus a token loaded back from the keyring when available.
fn c_keyring(tokens: &crate::token::TokenSet) -> (Check, Option<Secret>) {
    const ID: &str = "C07";
    const NAME: &str = "keyring store/load/delete (service a-eye, account claude:<id>)";
    if let Err(e) = store::store_status() {
        return (unverified(ID, NAME, &format!("UNVERIFIED: store skipped ({e})")), None);
    }
    match store::roundtrip(tokens) {
        Ok(r) if r.matched && r.deleted_and_gone => (
            Check {
                id: ID,
                name: NAME,
                status: Status::Pass,
                kind: Kind::Local,
                reason: format!("SYNTHETIC tokens stored, read back equal, entry '{}' deleted and confirmed gone; no pre-existing items touched", store::account_name(&r.account_id)),
            },
            r.loaded,
        ),
        Ok(r) => (
            Check {
                id: ID,
                name: NAME,
                status: Status::Fail,
                kind: Kind::Local,
                reason: format!("matched={} deleted_and_gone={} account={}", r.matched, r.deleted_and_gone, r.account_id),
            },
            None,
        ),
        Err(e) => (unverified(ID, NAME, &format!("UNVERIFIED: store skipped ({e})")), None),
    }
}

fn c_handoff_clean(stub: &Path, token: &Secret, source: &str) -> Result<String, String> {
    let r = run_handoff(stub, StubMode::Clean, token)?;
    if !r.is_clean_handoff() {
        return Err(format!("handoff not clean: {r:?}"));
    }
    Ok(format!(
        "token source={source}; child pid {} environ has {TOKEN_ENV}=<token> (value fingerprint matches); absent from /proc/<pid>/cmdline, stdout, stderr and from child HOME/TMPDIR/cwd scan; env keys={:?}; outranking vars (ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN, ...) not forwarded",
        r.child_pid, r.env_keys
    ))
}

fn c_handoff_detectors(stub: &Path, token: &Secret) -> Result<String, String> {
    let mut caught = vec![];
    for (mode, name) in [
        (StubMode::LeakStdout, "stdout"),
        (StubMode::LeakStderr, "stderr"),
        (StubMode::LeakFile, "disk"),
        (StubMode::LeakArgv, "argv"),
    ] {
        let r = run_handoff(stub, mode, token)?;
        let detected = match mode {
            StubMode::LeakStdout => r.stdout_has_token,
            StubMode::LeakStderr => r.stderr_has_token,
            StubMode::LeakFile => r.disk_has_token,
            StubMode::LeakArgv => r.argv_has_token,
            StubMode::Clean => false,
        };
        if !detected || r.is_clean_handoff() {
            return Err(format!("detector for {name} did not fire on a deliberate leak"));
        }
        caught.push(name);
    }
    Ok(format!("deliberately leaking stubs are caught on {caught:?}: the absence checks are not vacuous"))
}

// ---- record -----------------------------------------------------------------

pub struct Record {
    pub header: Vec<(String, String)>,
    pub checks: Vec<Check>,
}

impl Record {
    pub fn local_failures(&self) -> usize {
        self.checks.iter().filter(|c| c.kind == Kind::Local && c.status == Status::Fail).count()
    }
}

pub fn collect(stub_exe: &Path) -> Record {
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string();
    let rustc = cmd_out("rustc", &["--version"]).unwrap_or_else(|| "unavailable".into());
    let node = cmd_out("node", &["--version"]);
    let sdk = cmd_out("npm", &["view", "@anthropic-ai/claude-agent-sdk", "version"]);
    let cli = cmd_out("npm", &["view", "@anthropic-ai/claude-code", "version"]);
    let session = format!(
        "XDG_SESSION_TYPE={} WAYLAND_DISPLAY={} XDG_CURRENT_DESKTOP={}",
        std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unset".into()),
        std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "unset".into()),
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unset".into()),
    );
    let header = vec![
        ("OS".into(), format!("{} / kernel {}", os_pretty_name(), kernel)),
        ("Session".into(), session),
        ("rustc".into(), rustc),
        ("node".into(), node.unwrap_or_else(|| "not present".into())),
        (
            "npm latest (read-only `npm view`, nothing installed)".into(),
            format!(
                "@anthropic-ai/claude-agent-sdk={} ; @anthropic-ai/claude-code={}",
                sdk.unwrap_or_else(|| "unreachable".into()),
                cli.unwrap_or_else(|| "unreachable".into())
            ),
        ),
        (
            "crates (exact pins)".into(),
            CRATE_PINS.iter().map(|(n, v)| format!("{n} {v}")).collect::<Vec<_>>().join(", "),
        ),
        ("Provider".into(), "LOCAL MOCK ONLY - no real provider call was made; tokens are SYNTHETIC".into()),
    ];

    let mut checks = vec![
        check("C01", "PKCE S256 RFC 7636 Appendix B vector", Kind::Local, c_rfc_vector()),
        check("C02", "code_verifier / state generation", Kind::Local, c_verifier_shape()),
        check("C03", "PKCE flow end-to-end vs LOCAL MOCK (loopback callback, form token exchange)", Kind::Local, c_mock_e2e()),
        check("C04", "mock enforces PKCE + one-shot codes (non-vacuous)", Kind::Local, c_mock_rejects_bad_verifier()),
        check("C05", "negative paths + timeout (state/code/error/path)", Kind::Local, c_negative_flows()),
        check("C06", "secret redaction in Debug output", Kind::Local, c_redaction()),
    ];

    // Synthetic tokens for the store + handoff checks (taken from a mock run).
    let (res, _) = mock::run_against_mock(Behavior::Normal, Duration::from_secs(3), true);
    match res {
        Ok(tokens) => {
            let (kc, loaded) = c_keyring(&tokens);
            checks.push(kc);
            let (tok, src) = match loaded {
                Some(t) => (t, "keyring (loaded back from Secret Service)"),
                None => (tokens.access_token.clone(), "in-memory synthetic (keyring unavailable)"),
            };
            checks.push(check("C08", "env-only token handoff to SDK-runner stub (env yes; argv/stdout/stderr/disk no)", Kind::Local, c_handoff_clean(stub_exe, &tok, src)));
            checks.push(check("C09", "handoff leak detectors fire on deliberate leaks", Kind::Local, c_handoff_detectors(stub_exe, &tok)));
        }
        Err(e) => {
            checks.push(Check { id: "C07", name: "keyring store/load/delete", status: Status::Fail, kind: Kind::Local, reason: format!("could not obtain mock tokens: {e}") });
        }
    }

    checks.push(Check {
        id: "F01",
        name: "OAUTH.md assumption: third-party app runs its own claude.ai OAuth and stores the tokens",
        status: Status::Fail,
        kind: Kind::Finding,
        reason: "CONTRADICTED by Anthropic docs (code.claude.com/docs/en/legal-and-compliance): third parties may not offer claude.ai login, route via Free/Pro/Max credentials, or 'collect, store, or intermediate Claude.ai credentials or session tokens'".into(),
    });
    checks.push(unverified("F02", "live PKCE against claude.ai (endpoints, client_id, scopes)", "not run: needs interactive consent; no client_id is hardcoded/extracted; --live reads A_EYE_OAUTH_* env and is opt-in"));
    checks.push(unverified("F03", "real access-token lifetime", "docs: `claude setup-token` token is one year; /login access-token TTL and refresh behavior are not documented; mock uses synthetic 3600s"));
    checks.push(unverified("F04", "SDK accepts a subscription OAuth token via env", "docs name CLAUDE_CODE_OAUTH_TOKEN for Claude Code (CLI); SDK runs the same binary, but no SDK-specific statement and nothing was run"));
    checks.push(unverified("F05", "real SDK session turn + session-binding (2nd turn references 1st)", "real SDK session: UNVERIFIED (requires interactive consent + user subscription); SDK session_id is a UUID per docs"));
    Record { header, checks }
}

pub fn render(rec: &Record) -> String {
    let mut s = String::new();
    s.push_str("==== A-Eye spike part 03: Claude OAuth PKCE + SDK token handoff - CAPABILITY RECORD ====\n\n");
    for (k, v) in &rec.header {
        s.push_str(&format!("{k:<52}: {v}\n"));
    }
    s.push_str("\n---- Checks (L = property of this code; F = finding about the outside world) ----\n");
    for c in &rec.checks {
        let tag = if c.kind == Kind::Local { "L" } else { "F" };
        s.push_str(&format!("[{:<10}] {} {} {}\n             {}\n", c.status.label(), c.id, tag, c.name, c.reason));
    }
    let proved: Vec<String> = rec
        .checks
        .iter()
        .filter(|c| c.status == Status::Pass)
        .map(|c| format!("  - {}: {}", c.id, c.name))
        .collect();
    s.push_str("\n---- What was proved (against the LOCAL MOCK / local processes only) ----\n");
    s.push_str(&proved.join("\n"));
    s.push_str("\n\n---- What failed or is unknown ----\n");
    for c in rec.checks.iter().filter(|c| c.status != Status::Pass) {
        s.push_str(&format!("  - {} [{}] {}\n", c.id, c.status.label(), c.name));
    }
    s.push_str("\n---- Primary sources (read 2026-10-15) ----\n");
    for l in [
        "https://code.claude.com/docs/en/legal-and-compliance  (Authentication and credential use)",
        "https://code.claude.com/docs/en/authentication  (CLAUDE_CODE_OAUTH_TOKEN, `claude setup-token`, precedence)",
        "https://code.claude.com/docs/en/agent-sdk/overview  (note on claude.ai login / rate limits; package names)",
        "https://code.claude.com/docs/en/agent-sdk/sessions  (session_id, resume)",
    ] {
        s.push_str(&format!("  {l}\n"));
    }
    s.push_str("\nNotes: zeroize-on-drop is not implemented (no extra dependency in the spike). Mock server is HTTP on loopback; real endpoints are HTTPS.\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_match_cargo_toml() {
        let toml = include_str!("../Cargo.toml");
        for (name, ver) in CRATE_PINS {
            assert!(
                toml.contains(&format!("{name} = \"={ver}\"")) || toml.contains(&format!("{name} = {{ version = \"={ver}\"")),
                "{name} {ver} not pinned exactly in Cargo.toml"
            );
        }
    }
}
