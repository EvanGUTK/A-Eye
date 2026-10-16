//! S1 regression: the token exchange must NOT follow redirects. A token
//! endpoint that answers 3xx must produce an error and the redirect target
//! (a different local listener / address) must receive no connection at all.

use spike_03_claude_oauth::error::FlowError;
use spike_03_claude_oauth::http::{read_request, write_response};
use spike_03_claude_oauth::secret::Secret;
use spike_03_claude_oauth::token::exchange_code;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Listener {
    port: u16,
    hits: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Listener {
    /// `redirect_to`: Some(location) => answer every request with `status` + Location.
    /// None => a "target" that serves a poisoned token JSON if anyone ever connects.
    fn start(bind_ip: &str, status: u16, redirect_to: Option<String>) -> Listener {
        let l = TcpListener::bind((bind_ip, 0)).unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (h2, s2) = (hits.clone(), stop.clone());
        let join = thread::spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                match l.accept() {
                    Ok((mut stream, _)) => {
                        h2.fetch_add(1, Ordering::SeqCst);
                        let _ = read_request(&mut stream, Instant::now() + Duration::from_secs(2));
                        match &redirect_to {
                            Some(loc) => {
                                let _ = write_response(&mut stream, status, "Redirect", "text/plain", &[("Location", loc)], "");
                            }
                            None => {
                                let _ = write_response(
                                    &mut stream,
                                    200,
                                    "OK",
                                    "application/json",
                                    &[],
                                    r#"{"access_token":"ATTACKER-TOKEN","expires_in":1}"#,
                                );
                            }
                        }
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Listener { port, hits, stop, join: Some(join) }
    }
    fn hits(&self) -> u32 {
        self.hits.load(Ordering::SeqCst)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn exchange(url: &str) -> Result<spike_03_claude_oauth::token::TokenSet, FlowError> {
    exchange_code(
        url,
        "id",
        &Secret::new("SYNTH-CODE"),
        &Secret::new("SYNTH-VERIFIER"),
        "http://127.0.0.1:1/callback",
        Duration::from_secs(3),
    )
}

fn assert_redirect_refused(status: u16) {
    // Different local address AND port from the token endpoint.
    let target = Listener::start("127.0.0.2", status, None);
    let endpoint = Listener::start(
        "127.0.0.1",
        status,
        Some(format!("http://127.0.0.2:{}/", target.port)),
    );
    let r = exchange(&format!("http://127.0.0.1:{}/token", endpoint.port));
    thread::sleep(Duration::from_millis(250));
    assert_eq!(target.hits(), 0, "{status}: redirect target received a connection (result: {r:?})");
    assert_eq!(endpoint.hits(), 1, "{status}: token endpoint should be hit exactly once");
    match r {
        Err(FlowError::TokenEndpoint(ref m)) => assert_eq!(m, &format!("redirect_{status}_refused")),
        other => panic!("{status}: expected redirect refusal, got {other:?}"),
    }
}

#[test]
fn redirect_301_not_followed() {
    assert_redirect_refused(301);
}
#[test]
fn redirect_302_not_followed() {
    assert_redirect_refused(302);
}
#[test]
fn redirect_303_not_followed() {
    assert_redirect_refused(303);
}
#[test]
fn redirect_307_not_followed() {
    assert_redirect_refused(307);
}
#[test]
fn redirect_308_not_followed() {
    assert_redirect_refused(308);
}
