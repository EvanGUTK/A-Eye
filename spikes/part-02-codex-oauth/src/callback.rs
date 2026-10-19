//! One-shot local HTTP server that receives the OAuth redirect.
//!
//! * Bound to 127.0.0.1 only, on a random port unless one is configured.
//! * Accepts `GET <path>?code=..&state=..`. Everything else is answered with an
//!   error status and does NOT end the wait (a stray request must not be able to
//!   cancel the login); only a valid callback, a provider `error` carrying the
//!   correct state, or the timeout ends it.
//! * `wait` consumes the server: when it returns, the listener is dropped and the
//!   port is freed.
//! * The `Host` header must be a loopback authority for this port (DNS-rebinding
//!   guard). Response bodies never echo request data.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::secret::Secret;

pub const DEFAULT_PATH: &str = "/callback";
/// docs/architecture/OAUTH.md: listen for at most 5 minutes.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_REQUEST_BYTES: usize = 8 * 1024;
/// Total time one connection may take to deliver its request head.
const PER_CONNECTION_READ: Duration = Duration::from_secs(2);
/// Socket timeouts must be non-zero; this is the floor.
const MIN_SOCKET_TIMEOUT: Duration = Duration::from_millis(10);

#[derive(Debug)]
pub enum CallbackError {
    Bind(io::Error),
    Io(io::Error),
    /// No valid callback arrived before the deadline.
    Timeout(Duration),
    /// The provider redirected back with `error=...` (and our state).
    Provider { error: String, description: Option<String> },
}

impl std::fmt::Display for CallbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallbackError::Bind(e) => write!(f, "could not bind loopback callback port: {e}"),
            CallbackError::Io(e) => write!(f, "callback server I/O error: {e}"),
            CallbackError::Timeout(d) => {
                write!(f, "no valid OAuth callback within {:.1}s", d.as_secs_f64())
            }
            CallbackError::Provider { error, description } => match description {
                Some(d) => write!(f, "provider returned error '{error}': {d}"),
                None => write!(f, "provider returned error '{error}'"),
            },
        }
    }
}

impl std::error::Error for CallbackError {}

/// What a valid callback carries. `client_id` is present only when the provider echoes it
/// (the ChatGPT plan-usage registration returns the issued client_id here).
pub struct Callback {
    pub code: Secret,
    pub client_id: Option<String>,
}

pub struct CallbackServer {
    listener: TcpListener,
    addr: SocketAddr,
    path: String,
}

impl CallbackServer {
    /// `port` 0 = let the OS pick a free port.
    pub fn bind(port: u16) -> Result<Self, CallbackError> {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(CallbackError::Bind)?;
        listener.set_nonblocking(true).map_err(CallbackError::Bind)?;
        let addr = listener.local_addr().map_err(CallbackError::Bind)?;
        Ok(CallbackServer { listener, addr, path: DEFAULT_PATH.to_string() })
    }

    /// Override the callback path (default `/callback`). A provider that only
    /// allows a registered redirect URI (Codex CLI uses `/auth/callback`) needs this.
    pub fn with_path(mut self, path: &str) -> Self {
        self.path = path.to_string();
        self
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{}", self.addr.port(), self.path)
    }

    /// Block until one valid callback arrives or `timeout` elapses. Consumes the
    /// server, so the port is released on return.
    pub fn wait(self, expected_state: &str, timeout: Duration) -> Result<Secret, CallbackError> {
        self.wait_callback(expected_state, timeout).map(|cb| cb.code)
    }

    /// Like [`wait`](Self::wait) but also returns the `client_id` query parameter.
    pub fn wait_callback(self, expected_state: &str, timeout: Duration) -> Result<Callback, CallbackError> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(CallbackError::Timeout(timeout));
            }
            match self.listener.accept() {
                Ok((stream, _peer)) => {
                    if let Some(outcome) = self.handle(stream, expected_state, deadline) {
                        return outcome;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(CallbackError::Io(e)),
            }
        }
    }

    /// `None` = request rejected, keep listening. `Some` = terminal outcome.
    fn handle(
        &self,
        mut stream: TcpStream,
        expected_state: &str,
        deadline: Instant,
    ) -> Option<Result<Callback, CallbackError>> {
        // Absolute per-connection deadline: a trickling client gets PER_CONNECTION
        // total time (never more than the overall deadline), not PER_CONNECTION per read.
        let conn_deadline = (Instant::now() + PER_CONNECTION_READ).min(deadline);
        let write_budget = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(1))
            .max(MIN_SOCKET_TIMEOUT);
        if stream.set_nonblocking(false).is_err() || stream.set_write_timeout(Some(write_budget)).is_err() {
            return None;
        }
        let Some(head) = read_head(&mut stream, conn_deadline) else {
            // Out of time: just drop the connection and go back to listening.
            if Instant::now() < conn_deadline {
                respond(&mut stream, 400, "Bad Request");
            }
            return None;
        };
        let Some(req) = parse_head(&head) else {
            respond(&mut stream, 400, "Bad Request");
            return None;
        };

        let port = self.addr.port();
        let host_ok = req.host.as_deref().is_some_and(|h| {
            h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}")
        });
        if !host_ok {
            respond(&mut stream, 403, "Forbidden");
            return None;
        }
        if req.method != "GET" {
            respond(&mut stream, 405, "Method Not Allowed");
            return None;
        }
        if req.path != self.path {
            respond(&mut stream, 404, "Not Found");
            return None;
        }

        let mut code = None;
        let mut state = None;
        let mut error = None;
        let mut error_description = None;
        let mut client_id = None;
        for (k, v) in url::form_urlencoded::parse(req.query.as_bytes()) {
            match k.as_ref() {
                "code" if code.is_none() => code = Some(v.into_owned()),
                "state" if state.is_none() => state = Some(v.into_owned()),
                "error" if error.is_none() => error = Some(v.into_owned()),
                "error_description" if error_description.is_none() => {
                    error_description = Some(v.into_owned())
                }
                "client_id" if client_id.is_none() => client_id = Some(v.into_owned()),
                _ => {}
            }
        }

        // State is checked before the code or a provider error is trusted.
        if !state.as_deref().is_some_and(|s| ct_eq(s.as_bytes(), expected_state.as_bytes())) {
            respond(&mut stream, 400, "State mismatch");
            return None;
        }
        if let Some(error) = error {
            respond(&mut stream, 400, "Sign-in was not completed. You can close this tab.");
            return Some(Err(CallbackError::Provider { error, description: error_description }));
        }
        match code {
            Some(c) if !c.is_empty() => {
                respond(&mut stream, 200, "Sign-in complete. You can close this tab.");
                Some(Ok(Callback { code: Secret::new(c), client_id }))
            }
            _ => {
                respond(&mut stream, 400, "Missing authorization code");
                None
            }
        }
    }
}

struct RequestHead {
    method: String,
    path: String,
    query: String,
    host: Option<String>,
}

/// Read until the blank line ending the headers (bounded in size AND time).
/// The socket read timeout is reset to the time remaining before every read, so
/// the whole head must arrive by `deadline`. `None` on timeout, EOF before a
/// full head, or oversize.
fn read_head(stream: &mut TcpStream, deadline: Instant) -> Option<String> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        stream.set_read_timeout(Some(remaining.max(MIN_SOCKET_TIMEOUT))).ok()?;
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if buf.len() > MAX_REQUEST_BYTES {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    String::from_utf8(buf).ok()
}

fn parse_head(head: &str) -> Option<RequestHead> {
    let mut lines = head.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?;
    let version = parts.next()?;
    if !version.starts_with("HTTP/1.") || parts.next().is_some() || !target.starts_with('/') {
        return None;
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };
    let host = lines.find_map(|l| {
        let (name, value) = l.split_once(':')?;
        name.eq_ignore_ascii_case("host").then(|| value.trim().to_string())
    });
    Some(RequestHead { method, path, query, host })
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let msg = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(msg.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// Constant-time equality for equal-length inputs (length is not secret here).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// Send a raw request and return the full response text.
    fn raw(addr: SocketAddr, request: &str) -> String {
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        s.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    fn get(addr: SocketAddr, target: &str) -> String {
        raw(addr, &format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", addr.port()))
    }

    fn spawn_wait(
        server: CallbackServer,
        state: &'static str,
        timeout: Duration,
    ) -> thread::JoinHandle<Result<Secret, CallbackError>> {
        thread::spawn(move || server.wait(state, timeout))
    }

    #[test]
    fn binds_loopback_only_on_random_port() {
        let s = CallbackServer::bind(0).unwrap();
        assert_eq!(s.local_addr().ip(), std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(s.local_addr().port(), 0);
        assert_eq!(s.redirect_uri(), format!("http://127.0.0.1:{}/callback", s.local_addr().port()));
    }

    #[test]
    fn valid_callback_delivers_code_and_frees_port() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let h = spawn_wait(server, "st4te", Duration::from_secs(5));
        let resp = get(addr, "/callback?code=abc%2F123&state=st4te");
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        let code = h.join().unwrap().unwrap();
        assert_eq!(code.expose(), "abc/123"); // percent-decoded
        // Port is free again: nothing listens, and we can rebind it.
        assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err());
        CallbackServer::bind(addr.port()).expect("port should be rebindable");
    }

    #[test]
    fn state_mismatch_is_rejected_and_server_keeps_waiting() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let h = spawn_wait(server, "good", Duration::from_secs(5));
        let bad = get(addr, "/callback?code=evil&state=wrong");
        assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
        let none = get(addr, "/callback?code=evil");
        assert!(none.starts_with("HTTP/1.1 400"), "{none}");
        let ok = get(addr, "/callback?code=real&state=good");
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert_eq!(h.join().unwrap().unwrap().expose(), "real");
    }

    #[test]
    fn missing_or_empty_code_rejected() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let h = spawn_wait(server, "s", Duration::from_secs(5));
        assert!(get(addr, "/callback?state=s").starts_with("HTTP/1.1 400"));
        assert!(get(addr, "/callback?code=&state=s").starts_with("HTTP/1.1 400"));
        assert!(get(addr, "/callback?code=c&state=s").starts_with("HTTP/1.1 200"));
        assert_eq!(h.join().unwrap().unwrap().expose(), "c");
    }

    #[test]
    fn wrong_method_path_and_host_rejected() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let h = spawn_wait(server, "s", Duration::from_secs(5));
        let port = addr.port();
        let post = raw(
            addr,
            &format!("POST /callback?code=c&state=s HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        );
        assert!(post.starts_with("HTTP/1.1 405"), "{post}");
        assert!(get(addr, "/other?code=c&state=s").starts_with("HTTP/1.1 404"));
        assert!(get(addr, "/callbackx?code=c&state=s").starts_with("HTTP/1.1 404"));
        let rebind = raw(
            addr,
            "GET /callback?code=c&state=s HTTP/1.1\r\nHost: evil.example.com\r\n\r\n",
        );
        assert!(rebind.starts_with("HTTP/1.1 403"), "{rebind}");
        let nohost = raw(addr, "GET /callback?code=c&state=s HTTP/1.1\r\n\r\n");
        assert!(nohost.starts_with("HTTP/1.1 403"), "{nohost}");
        // none of the above ended the wait; a valid one does
        assert!(get(addr, "/callback?code=c&state=s").starts_with("HTTP/1.1 200"));
        assert!(h.join().unwrap().is_ok());
    }

    #[test]
    fn garbage_and_oversize_requests_do_not_panic_or_end_wait() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let h = spawn_wait(server, "s", Duration::from_secs(5));
        let _ = raw(addr, "\x00\x01\x02 not http\r\n\r\n");
        let big = format!("GET /callback?x={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n", "a".repeat(20_000), addr.port());
        let _ = raw(addr, &big); // no blank line, oversize
        assert!(get(addr, "/callback?code=c&state=s").starts_with("HTTP/1.1 200"));
        assert!(h.join().unwrap().is_ok());
    }

    #[test]
    fn provider_error_with_valid_state_is_err() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let h = spawn_wait(server, "s", Duration::from_secs(5));
        let _ = get(addr, "/callback?error=access_denied&error_description=nope&state=s");
        match h.join().unwrap() {
            Err(CallbackError::Provider { error, description }) => {
                assert_eq!(error, "access_denied");
                assert_eq!(description.as_deref(), Some("nope"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn provider_error_with_wrong_state_is_ignored() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let h = spawn_wait(server, "s", Duration::from_secs(5));
        let r = get(addr, "/callback?error=access_denied&state=forged");
        assert!(r.starts_with("HTTP/1.1 400"));
        assert!(get(addr, "/callback?code=c&state=s").starts_with("HTTP/1.1 200"));
        assert!(h.join().unwrap().is_ok());
    }

    #[test]
    fn timeout_returns_err_not_panic_and_frees_port() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let started = Instant::now();
        let r = server.wait("s", Duration::from_millis(400));
        assert!(matches!(r, Err(CallbackError::Timeout(_))), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err());
    }

    #[test]
    fn idle_client_cannot_block_past_deadline() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        // Connect and send nothing; the wait must still honour its deadline.
        let _idle = TcpStream::connect(addr).unwrap();
        let started = Instant::now();
        let r = server.wait("s", Duration::from_millis(500));
        assert!(matches!(r, Err(CallbackError::Timeout(_))));
        assert!(started.elapsed() < Duration::from_secs(4), "took {:?}", started.elapsed());
    }

    /// Trickle `payload` one byte per `gap`, for at most `max`, from a thread.
    fn trickle(addr: SocketAddr, payload: String, gap: Duration, max: Duration) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut s = TcpStream::connect(addr).unwrap();
            let end = Instant::now() + max;
            for b in payload.bytes().cycle() {
                if Instant::now() >= end || s.write_all(&[b]).is_err() {
                    break;
                }
                thread::sleep(gap);
            }
        })
    }

    /// Regression (validator B1): the read timeout used to be per read(), so a
    /// client sending one byte every few hundred ms kept `wait` open far past
    /// its deadline.
    #[test]
    fn trickling_client_cannot_block_past_deadline() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let t = trickle(
            addr,
            format!("GET /callback?x=y HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Pad: ", addr.port()),
            Duration::from_millis(150),
            Duration::from_secs(8),
        );
        let deadline = Duration::from_secs(1);
        let started = Instant::now();
        let r = server.wait("s", deadline);
        let took = started.elapsed();
        assert!(matches!(r, Err(CallbackError::Timeout(_))), "{r:?}");
        assert!(took < deadline + Duration::from_millis(700), "wait overran its deadline: {took:?}");
        t.join().unwrap();
    }

    /// A trickler that connected first must not starve the real callback:
    /// it is dropped at the per-connection limit and the callback is served.
    #[test]
    fn trickler_does_not_starve_real_callback() {
        let server = CallbackServer::bind(0).unwrap();
        let addr = server.local_addr();
        let t = trickle(
            addr,
            format!("GET /callback?x=y HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Pad: ", addr.port()),
            Duration::from_millis(150),
            Duration::from_secs(8),
        );
        let h = spawn_wait(server, "s", Duration::from_secs(6));
        thread::sleep(Duration::from_millis(300)); // trickler is accepted first
        let ok = get(addr, "/callback?code=real&state=s");
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert_eq!(h.join().unwrap().unwrap().expose(), "real");
        t.join().unwrap();
    }

    #[test]
    fn constant_time_eq_basics() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
