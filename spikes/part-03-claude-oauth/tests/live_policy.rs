//! `--live` endpoint policy, exercised through the real binary. Nothing here
//! reaches the network: bad endpoints are refused before any flow starts, and
//! the https-accepted case stops at the policy acknowledgement gate.

use spike_03_claude_oauth::error::FlowError;
use spike_03_claude_oauth::secret::Secret;
use spike_03_claude_oauth::token::exchange_code;
use std::process::Command;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_spike-03-claude-oauth");

fn live(auth: &str, token: &str, ack: bool) -> (Option<i32>, String) {
    let mut c = Command::new(BIN);
    c.arg("--live")
        .env_clear()
        .env("A_EYE_OAUTH_CLIENT_ID", "test-client")
        .env("A_EYE_OAUTH_AUTH_URL", auth)
        .env("A_EYE_OAUTH_TOKEN_URL", token);
    if ack {
        c.env("A_EYE_LIVE_ACK_POLICY", "1");
    }
    let o = c.output().unwrap();
    (o.status.code(), String::from_utf8_lossy(&o.stderr).to_string())
}

const OK_AUTH: &str = "https://auth.example.com/authorize";
const OK_TOKEN: &str = "https://auth.example.com/token";

#[test]
fn live_rejects_non_https_and_lookalike_token_urls() {
    for bad in [
        "http://127.0.0.1.evil.com/token",
        "http://127.0.0.1:1@evil.com/token",
        "http://localhost.evil.example/token",
        "http://example.com/token",
        "http://127.0.0.1:9/token",
    ] {
        let (code, err) = live(OK_AUTH, bad, true);
        assert_eq!(code, Some(2), "{bad}: {err}");
        assert!(err.contains("--live refused: A_EYE_OAUTH_TOKEN_URL"), "{bad}: {err}");
    }
}

#[test]
fn live_rejects_non_https_auth_urls() {
    for bad in ["http://127.0.0.1.evil.com/authorize", "http://localhost.evil.example/a", "http://example.com/a"] {
        let (code, err) = live(bad, OK_TOKEN, true);
        assert_eq!(code, Some(2), "{bad}: {err}");
        assert!(err.contains("--live refused: A_EYE_OAUTH_AUTH_URL"), "{bad}: {err}");
    }
}

#[test]
fn live_accepts_https_then_stops_at_ack_gate() {
    let (code, err) = live(OK_AUTH, OK_TOKEN, false);
    assert_eq!(code, Some(2));
    assert!(!err.contains("--live refused"), "https endpoints must pass validation: {err}");
    assert!(err.contains("A_EYE_LIVE_ACK_POLICY=1"), "{err}");
}

#[test]
fn token_exchange_refuses_cleartext_to_lookalike_hosts() {
    let (code, verifier) = (Secret::new("c"), Secret::new("v"));
    for bad in [
        "http://127.0.0.1.evil.com/token",
        "http://127.0.0.1:1@evil.com/token",
        "http://localhost.evil.example/token",
        "http://example.com/token",
    ] {
        let r = exchange_code(bad, "id", &code, &verifier, "http://127.0.0.1:1/callback", Duration::from_secs(1));
        // Refused by the URL policy (no connection attempted: "token request failed" would mean it tried).
        assert!(
            matches!(r, Err(FlowError::Io(ref m)) if m.contains("refusing") || m.contains("userinfo")),
            "{bad}: {r:?}"
        );
    }
}

#[test]
fn exchange_uses_normalized_url_for_alternative_loopback_spellings() {
    use spike_03_claude_oauth::mock::{Behavior, MockProvider};
    // Mock answers invalid_grant to any well-formed form POST: reaching it proves
    // the request went to the normalized loopback address.
    let mock = MockProvider::start(Behavior::TokenInvalidGrant).unwrap();
    let port = mock.base.rsplit(':').next().unwrap().to_string();
    for host in ["127.1", "0x7f.1", "2130706433", "127.0.0.1."] {
        let url = format!("http://{host}:{port}/token");
        let r = exchange_code(&url, "mock-client-id", &Secret::new("c"), &Secret::new("v"), "http://127.0.0.1:1/callback", Duration::from_secs(3));
        assert_eq!(r.unwrap_err(), FlowError::TokenEndpoint("invalid_grant".into()), "{url}");
    }
}
