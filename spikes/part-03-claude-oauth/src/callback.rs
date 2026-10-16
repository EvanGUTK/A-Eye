//! One-shot loopback callback server.
//!
//! Binds 127.0.0.1 only (random port). Accepts requests until one of:
//! - a valid `/callback?code=..&state=..` arrives          -> Ok(code)
//! - the callback carries `error=..`                      -> ProviderDenied
//! - the callback has a missing/mismatched `state`        -> StateMismatch (fail closed)
//! - the callback has the right state but no `code`       -> MissingCode
//! - the deadline passes                                  -> Timeout
//!
//! Requests for other paths/methods/Host headers (favicon probes, port scans,
//! DNS-rebinding attempts) get 404/405/400 and do NOT end the flow.

use crate::error::{sanitize_code, FlowError};
use crate::http::{read_request, write_response};
use crate::pkce::ct_eq;
use crate::secret::Secret;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};
use url::Url;

pub const CALLBACK_PATH: &str = "/callback";

pub struct CallbackServer {
    listener: TcpListener,
    port: u16,
}

const PAGE_OK: &str = "<!doctype html><title>A-Eye</title><p>Sign-in complete. You can close this tab.</p>";
const PAGE_ERR: &str = "<!doctype html><title>A-Eye</title><p>Sign-in failed. Return to A-Eye for details.</p>";

impl CallbackServer {
    pub fn bind() -> Result<Self, FlowError> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        Ok(Self { listener, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{}", self.port, CALLBACK_PATH)
    }

    /// Consumes the server: the listener is dropped (port closed) on return.
    pub fn wait(self, expected_state: &str, timeout: Duration) -> Result<Secret, FlowError> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(FlowError::Timeout);
            }
            match self.listener.accept() {
                Ok((stream, _peer)) => {
                    if let Some(result) = self.handle(stream, expected_state, deadline) {
                        return result;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// `None` = keep listening; `Some(result)` = flow finished.
    fn handle(
        &self,
        mut stream: TcpStream,
        expected_state: &str,
        overall_deadline: Instant,
    ) -> Option<Result<Secret, FlowError>> {
        // Per-connection absolute deadline, never later than the overall one.
        let conn_deadline = (Instant::now() + Duration::from_secs(2)).min(overall_deadline);
        let req = match read_request(&mut stream, conn_deadline) {
            Ok(r) => r,
            Err(_) => return None, // preconnect / garbage: ignore
        };
        let host_ok = req.header("host").is_some_and(|h| {
            h == format!("127.0.0.1:{}", self.port) || h == format!("localhost:{}", self.port)
        });
        if !host_ok {
            let _ = write_response(&mut stream, 400, "Bad Request", "text/plain", &[], "bad host");
            return None;
        }
        if req.method != "GET" {
            let _ = write_response(&mut stream, 405, "Method Not Allowed", "text/plain", &[], "");
            return None;
        }
        let url = match Url::parse(&format!("http://127.0.0.1{}", req.target)) {
            Ok(u) => u,
            Err(_) => {
                let _ = write_response(&mut stream, 400, "Bad Request", "text/plain", &[], "");
                return None;
            }
        };
        if url.path() != CALLBACK_PATH {
            let _ = write_response(&mut stream, 404, "Not Found", "text/plain", &[], "");
            return None;
        }
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                "error" => error = Some(v.into_owned()),
                _ => {}
            }
        }
        let fail = |stream: &mut TcpStream, e: FlowError| {
            let _ = write_response(stream, 400, "Bad Request", "text/html", &[], PAGE_ERR);
            Some(Err(e))
        };
        // State is checked before anything else so an attacker-supplied
        // error/code is never trusted without proving knowledge of `state`.
        match state {
            Some(s) if ct_eq(&s, expected_state) => {}
            _ => return fail(&mut stream, FlowError::StateMismatch),
        }
        if let Some(e) = error {
            return fail(&mut stream, FlowError::ProviderDenied(sanitize_code(&e)));
        }
        match code {
            Some(c) if !c.is_empty() => {
                let _ = write_response(&mut stream, 200, "OK", "text/html", &[], PAGE_OK);
                Some(Ok(Secret::new(c)))
            }
            _ => fail(&mut stream, FlowError::MissingCode),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::thread;

    /// Raw TCP "browser": sends `GET <target>` with a correct Host header.
    fn get(port: u16, target: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    fn run(server: CallbackServer, state: &'static str) -> thread::JoinHandle<Result<Secret, FlowError>> {
        thread::spawn(move || server.wait(state, Duration::from_secs(2)))
    }

    #[test]
    fn accepts_valid_callback() {
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let h = run(server, "st4te");
        let resp = get(port, "/callback?code=abc123&state=st4te");
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        let code = h.join().unwrap().unwrap();
        assert_eq!(code.expose(), "abc123");
    }

    #[test]
    fn rejects_mismatched_state() {
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let h = run(server, "st4te");
        let resp = get(port, "/callback?code=abc123&state=EVIL");
        assert!(resp.starts_with("HTTP/1.1 400"), "{resp}");
        assert_eq!(h.join().unwrap().unwrap_err(), FlowError::StateMismatch);
    }

    #[test]
    fn rejects_missing_state() {
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let h = run(server, "st4te");
        get(port, "/callback?code=abc123");
        assert_eq!(h.join().unwrap().unwrap_err(), FlowError::StateMismatch);
    }

    #[test]
    fn rejects_missing_code() {
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let h = run(server, "st4te");
        get(port, "/callback?state=st4te");
        assert_eq!(h.join().unwrap().unwrap_err(), FlowError::MissingCode);
    }

    #[test]
    fn provider_error_is_sanitized_and_requires_state() {
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let h = run(server, "st4te");
        get(port, "/callback?error=access_denied&state=st4te");
        assert_eq!(
            h.join().unwrap().unwrap_err(),
            FlowError::ProviderDenied("access_denied".into())
        );
    }

    #[test]
    fn wrong_path_does_not_end_flow() {
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let h = run(server, "st4te");
        let r1 = get(port, "/favicon.ico");
        assert!(r1.starts_with("HTTP/1.1 404"), "{r1}");
        let r2 = get(port, "/callback/extra?code=x&state=st4te");
        assert!(r2.starts_with("HTTP/1.1 404"), "{r2}");
        // Flow still alive: a correct callback succeeds afterwards.
        get(port, "/callback?code=good&state=st4te");
        assert_eq!(h.join().unwrap().unwrap().expose(), "good");
    }

    #[test]
    fn foreign_host_header_is_rejected_and_ignored() {
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let h = run(server, "st4te");
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"GET /callback?code=x&state=st4te HTTP/1.1\r\nHost: evil.example\r\n\r\n")
            .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 400"), "{out}");
        get(port, "/callback?code=ok&state=st4te");
        assert_eq!(h.join().unwrap().unwrap().expose(), "ok");
    }

    #[test]
    fn times_out_and_releases_port() {
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let started = Instant::now();
        let err = server.wait("st4te", Duration::from_millis(300)).unwrap_err();
        assert_eq!(err, FlowError::Timeout);
        assert!(started.elapsed() < Duration::from_secs(2));
        // Listener dropped: connecting must now fail.
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    /// N1 regression: a client trickling one byte every 200 ms (never finishing
    /// the request head) must not be able to hold the server past the overall
    /// callback deadline.
    #[test]
    fn trickling_client_cannot_extend_the_deadline() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let server = CallbackServer::bind().unwrap();
        let port = server.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let trickler = thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let head = format!("GET /callback?code=x&state=st4te HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: ");
            for b in head.bytes().chain(std::iter::repeat(b'a')).take(60) {
                if stop2.load(Ordering::Relaxed) || s.write_all(&[b]).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(200));
            }
        });
        let started = Instant::now();
        let err = server.wait("st4te", Duration::from_secs(1)).unwrap_err();
        let elapsed = started.elapsed();
        stop.store(true, Ordering::Relaxed);
        let _ = trickler.join();
        assert_eq!(err, FlowError::Timeout);
        assert!(
            elapsed < Duration::from_millis(1600),
            "deadline overrun by trickling client: took {elapsed:?} for a 1s timeout"
        );
    }

    #[test]
    fn binds_loopback_only() {
        let server = CallbackServer::bind().unwrap();
        assert!(server.listener.local_addr().unwrap().ip().is_loopback());
        assert!(server.redirect_uri().starts_with("http://127.0.0.1:"));
    }
}
