//! End-to-end PKCE flow against the LOCAL MOCK (the test plays the browser).

use spike_03_claude_oauth::error::FlowError;
use spike_03_claude_oauth::mock::{run_against_mock, Behavior};
use std::time::Duration;

#[test]
fn full_flow_succeeds_and_mock_verifies_pkce() {
    let (res, obs) = run_against_mock(Behavior::Normal, Duration::from_secs(3), true);
    let t = res.expect("flow should succeed");
    assert!(t.access_token.expose().starts_with("SYNTH-ACCESS-"));
    assert!(t.refresh_token.unwrap().expose().starts_with("SYNTH-REFRESH-"));
    assert_eq!(t.expires_in, Some(3600));
    assert_eq!(t.token_type.as_deref(), Some("Bearer"));
    assert_eq!(obs.authorize_hits, 1);
    assert_eq!(obs.token_hits, 1);
    assert!(obs.saw_s256 && obs.saw_form_encoded && obs.pkce_verified && obs.redirect_uri_matched);
}

#[test]
fn tampered_state_is_rejected() {
    let (res, obs) = run_against_mock(Behavior::WrongState, Duration::from_secs(3), true);
    assert_eq!(res.unwrap_err(), FlowError::StateMismatch);
    assert_eq!(obs.token_hits, 0, "must not exchange a code after a state mismatch");
}

#[test]
fn missing_code_is_rejected() {
    let (res, obs) = run_against_mock(Behavior::MissingCode, Duration::from_secs(3), true);
    assert_eq!(res.unwrap_err(), FlowError::MissingCode);
    assert_eq!(obs.token_hits, 0);
}

#[test]
fn provider_denial_is_surfaced() {
    let (res, obs) = run_against_mock(Behavior::AccessDenied, Duration::from_secs(3), true);
    assert_eq!(res.unwrap_err(), FlowError::ProviderDenied("access_denied".into()));
    assert_eq!(obs.token_hits, 0);
}

#[test]
fn token_endpoint_error_is_surfaced() {
    let (res, _) = run_against_mock(Behavior::TokenInvalidGrant, Duration::from_secs(3), true);
    assert_eq!(res.unwrap_err(), FlowError::TokenEndpoint("invalid_grant".into()));
}

#[test]
fn no_browser_means_timeout() {
    let started = std::time::Instant::now();
    let (res, obs) = run_against_mock(Behavior::Normal, Duration::from_secs(1), false);
    assert_eq!(res.unwrap_err(), FlowError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(obs.token_hits, 0);
}

#[test]
fn mock_rejects_wrong_verifier_and_replay() {
    use spike_03_claude_oauth::flow::{build_authorize_url, FlowConfig};
    use spike_03_claude_oauth::mock::{authorize_no_follow, MockProvider, MOCK_CLIENT_ID, MOCK_SCOPE};
    use spike_03_claude_oauth::pkce::Pkce;
    use spike_03_claude_oauth::secret::Secret;
    use spike_03_claude_oauth::token::exchange_code;

    let mock = MockProvider::start(Behavior::Normal).unwrap();
    let cfg = FlowConfig {
        client_id: MOCK_CLIENT_ID.into(),
        auth_url: mock.auth_url(),
        token_url: mock.token_url(),
        scope: MOCK_SCOPE.into(),
        callback_timeout: Duration::from_secs(1),
        http_timeout: Duration::from_secs(5),
    };
    let good = Pkce::generate().unwrap();
    let redirect = "http://127.0.0.1:9/callback";
    let url = build_authorize_url(&cfg, redirect, &good.challenge, "st").unwrap();
    let loc = authorize_no_follow(url.as_str()).unwrap().expect("redirect");
    let code = url::Url::parse(&loc)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| Secret::new(v.into_owned()))
        .unwrap();
    let wrong = Pkce::generate().unwrap();
    let r = exchange_code(&cfg.token_url, &cfg.client_id, &code, &wrong.verifier, redirect, cfg.http_timeout);
    assert_eq!(r.unwrap_err(), FlowError::TokenEndpoint("invalid_grant".into()));
    // Wrong redirect_uri is rejected too.
    let r = exchange_code(&cfg.token_url, &cfg.client_id, &code, &good.verifier, "http://127.0.0.1:1/callback", cfg.http_timeout);
    assert_eq!(r.unwrap_err(), FlowError::TokenEndpoint("invalid_grant".into()));
    // Correct verifier works once ...
    assert!(exchange_code(&cfg.token_url, &cfg.client_id, &code, &good.verifier, redirect, cfg.http_timeout).is_ok());
    // ... and the code cannot be replayed.
    let r = exchange_code(&cfg.token_url, &cfg.client_id, &code, &good.verifier, redirect, cfg.http_timeout);
    assert_eq!(r.unwrap_err(), FlowError::TokenEndpoint("invalid_grant".into()));
    let o = mock.observations();
    assert_eq!((o.bad_verifier_rejected, o.replay_rejected), (1, 1));
}
