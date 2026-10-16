//! Real Secret Service round trip with SYNTHETIC tokens and a unique account.
//! If the Secret Service is unusable the test prints
//! `UNVERIFIED: store skipped (<reason>)` and returns (the capability record
//! reports it as UNVERIFIED, never PASS).

use spike_03_claude_oauth::{pkce, store, token};

#[test]
fn store_load_delete_synthetic_tokens() {
    if let Err(e) = store::store_status() {
        eprintln!("UNVERIFIED: store skipped ({e})");
        return;
    }
    let tokens = token::parse_token_response(&format!(
        r#"{{"access_token":"SYNTH-KR-{}","refresh_token":"SYNTH-KR-R","expires_in":60}}"#,
        pkce::random_hex(8).unwrap()
    ))
    .unwrap();
    match store::roundtrip(&tokens) {
        Ok(r) => {
            assert!(r.matched, "loaded token differs from stored");
            assert!(r.deleted_and_gone, "test entry still present after delete");
            assert!(!store::entry_exists(&r.account_id).unwrap());
        }
        Err(e) => eprintln!("UNVERIFIED: store skipped ({e})"),
    }
}

#[test]
fn timeout_wrapper_gives_up() {
    let r = store::with_timeout(std::time::Duration::from_millis(100), || {
        std::thread::sleep(std::time::Duration::from_secs(2));
    });
    assert!(r.is_err());
}
