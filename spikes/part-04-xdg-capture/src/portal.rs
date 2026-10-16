//! XDG ScreenCast portal session via `ashpd` (issue #13).
//!
//! The session is a typestate chain. Each step consumes the previous state:
//!
//! ```text
//! PortalCreated --select_monitor()--> PortalSelected --start()--> StartedSession
//!                                                                     |
//!                                              open_pipe_wire_remote() v
//!                                                                CaptureTarget
//! ```
//!
//! `CaptureTarget` (the only thing the PipeWire consumer accepts) has private
//! fields and can only be produced by `StartedSession::open_pipe_wire_remote`,
//! and `StartedSession` can only be produced by a successful `start()`, i.e.
//! after the compositor consent dialog returned a selected stream. So no frame
//! acquisition can begin before the session is confirmed started and the
//! user's selected stream is known. This is enforced by construction, not by
//! a runtime flag.

use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream,
};
use ashpd::desktop::{PersistMode, ResponseError, Session};
use ashpd::enumflags2::BitFlags;

/// Consent budget shared by SelectSources + Start (the interactive calls).
/// Non-interactive steps and the error-path session close each have their own
/// [`STEP_TIMEOUT`] (10s), so the worst case for the whole portal stage is
/// 90s + 10s, never unbounded.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(90);
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum PortalError {
    /// ScreenCast interface is not offered by the portal frontend/backends.
    Unavailable(String),
    /// A portal step failed for a non-user reason.
    Failed {
        stage: &'static str,
        message: String,
    },
    /// The user (or compositor) cancelled/denied the consent dialog.
    Denied,
    /// No consent decision within [`CONSENT_TIMEOUT`].
    TimedOut,
}

impl std::fmt::Display for PortalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PortalError::Unavailable(m) => write!(f, "ScreenCast portal unavailable: {m}"),
            PortalError::Failed { stage, message } => write!(f, "{stage} failed: {message}"),
            PortalError::Denied => write!(f, "user denied / cancelled the consent dialog"),
            PortalError::TimedOut => {
                write!(f, "no consent within {}s", CONSENT_TIMEOUT.as_secs())
            }
        }
    }
}

impl std::error::Error for PortalError {}

fn classify(stage: &'static str, e: ashpd::Error) -> PortalError {
    match e {
        ashpd::Error::PortalNotFound(i) => {
            PortalError::Unavailable(format!("no portal frontend/backend implements {i}"))
        }
        ashpd::Error::Response(ResponseError::Cancelled) => PortalError::Denied,
        other => PortalError::Failed {
            stage,
            message: other.to_string(),
        },
    }
}

/// Map the outcome of an interactive (consent) step. A timeout is `TimedOut`;
/// a portal response error (including `Cancelled`) goes through [`classify`].
fn resolve<T>(
    stage: &'static str,
    timed: Result<Result<T, ashpd::Error>, tokio::time::error::Elapsed>,
) -> Result<T, PortalError> {
    match timed {
        Err(_) => Err(PortalError::TimedOut),
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(classify(stage, e)),
    }
}

/// Something that can end a portal session (seam so the "error => session
/// closed" rule is unit-testable without a portal).
trait Closable {
    async fn close_it(&self) -> Result<(), ashpd::Error>;
}

impl Closable for Session<Screencast> {
    async fn close_it(&self) -> Result<(), ashpd::Error> {
        self.close().await
    }
}

/// On `Err`, close the session (dismissing any open dialog) and pass the
/// error through; on `Ok` leave the session open.
async fn close_on_error<T, C: Closable>(
    closer: &C,
    r: Result<T, PortalError>,
) -> Result<T, PortalError> {
    if r.is_err() {
        let _ = tokio::time::timeout(STEP_TIMEOUT, closer.close_it()).await;
    }
    r
}

/// Facts about the portal interface, recorded as soon as they are known.
#[derive(Debug, Clone, Default)]
pub struct PortalFacts {
    pub interface_version: Option<u32>,
    pub available_source_types: Option<String>,
    pub available_cursor_modes: Option<String>,
    pub chosen_cursor_mode: Option<String>,
}

/// What the user actually selected (from the portal's Start response).
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub node_id: u32,
    /// (width, height) in the compositor coordinate space (logical), not necessarily pixels.
    pub logical_size: Option<(i32, i32)>,
    /// (x, y) in the compositor coordinate space.
    pub position: Option<(i32, i32)>,
    pub source_type: Option<String>,
    pub stream_id: Option<String>,
}

impl StreamInfo {
    fn from_stream(s: &Stream) -> Self {
        Self {
            node_id: s.pipe_wire_node_id(),
            logical_size: s.size(),
            position: s.position(),
            source_type: s.source_type().map(|t| format!("{t:?}")),
            stream_id: s.id().map(|s| s.to_string()),
        }
    }
}

/// Step 1: interface bound, session created.
pub struct PortalCreated {
    proxy: Screencast,
    session: Session<Screencast>,
    pub facts: PortalFacts,
}

/// Step 2: sources selected (monitor) and the portal's SelectSources
/// response was OK (not cancelled). Carries the consent deadline that
/// started before SelectSources: on some backends (xdph) the interactive
/// picker is shown during SelectSources, on others during Start.
pub struct PortalSelected {
    proxy: Screencast,
    session: Session<Screencast>,
    pub facts: PortalFacts,
    deadline: Instant,
}

/// Step 3: the user consented; `stream` is what they selected.
/// Only constructible by [`PortalSelected::start`].
pub struct StartedSession {
    proxy: Screencast,
    session: Session<Screencast>,
    pub facts: PortalFacts,
    pub stream: StreamInfo,
    /// Instant the portal confirmed the session (latency baseline for #14).
    pub confirmed_at: Instant,
    pub stream_count: usize,
}

/// PipeWire remote fd + node id of the stream the user selected.
/// Only constructible by [`StartedSession::open_pipe_wire_remote`].
pub struct CaptureTarget {
    fd: OwnedFd,
    node_id: u32,
    confirmed_at: Instant,
}

impl CaptureTarget {
    pub fn node_id(&self) -> u32 {
        self.node_id
    }
    pub fn confirmed_at(&self) -> Instant {
        self.confirmed_at
    }
    pub fn into_fd(self) -> OwnedFd {
        self.fd
    }
}

async fn with_timeout<T>(
    stage: &'static str,
    fut: impl std::future::Future<Output = Result<T, ashpd::Error>>,
) -> Result<T, PortalError> {
    match tokio::time::timeout(STEP_TIMEOUT, fut).await {
        Ok(r) => r.map_err(|e| classify(stage, e)),
        Err(_) => Err(PortalError::Failed {
            stage,
            message: format!("timed out after {}s", STEP_TIMEOUT.as_secs()),
        }),
    }
}

/// Bind the ScreenCast interface and create a session.
pub async fn create() -> Result<PortalCreated, PortalError> {
    let proxy = with_timeout("bind ScreenCast interface", Screencast::new()).await?;
    let mut facts = PortalFacts {
        interface_version: Some(proxy.version()),
        ..Default::default()
    };
    facts.available_source_types =
        tokio::time::timeout(STEP_TIMEOUT, proxy.available_source_types())
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|t| format!("{t:?}"));
    facts.available_cursor_modes =
        tokio::time::timeout(STEP_TIMEOUT, proxy.available_cursor_modes())
            .await
            .ok()
            .and_then(|r| r.ok())
            .map(|m| format!("{m:?}"));
    let session = with_timeout("create_session", proxy.create_session(Default::default())).await?;
    Ok(PortalCreated {
        proxy,
        session,
        facts,
    })
}

/// Pending portal reply to SelectSources. ashpd hands this back as
/// `Ok(Request)` even when the portal answered Cancelled; the verdict is only
/// in `response()`.
trait SelectReply {
    fn response(&self) -> Result<(), ashpd::Error>;
}

/// Pending portal reply to Start.
trait StartReply {
    fn streams(&self) -> Result<Vec<StreamInfo>, ashpd::Error>;
}

impl SelectReply for ashpd::desktop::Request<()> {
    fn response(&self) -> Result<(), ashpd::Error> {
        ashpd::desktop::Request::response(self)
    }
}

impl StartReply for ashpd::desktop::Request<ashpd::desktop::screencast::Streams> {
    fn streams(&self) -> Result<Vec<StreamInfo>, ashpd::Error> {
        let r = self.response()?;
        Ok(r.streams().iter().map(StreamInfo::from_stream).collect())
    }
}

/// The two interactive portal calls, as a seam so the call-site wiring
/// (await response, map errors, close the session, share one deadline) is
/// unit-testable with a fake portal.
trait ScreencastApi {
    type Session: Closable;
    type Select: SelectReply;
    type Start: StartReply;
    async fn select_sources(
        &self,
        session: &Self::Session,
        opts: SelectSourcesOptions,
    ) -> Result<Self::Select, ashpd::Error>;
    async fn start(&self, session: &Self::Session) -> Result<Self::Start, ashpd::Error>;
}

struct RealApi<'a>(&'a Screencast);

impl ScreencastApi for RealApi<'_> {
    type Session = Session<Screencast>;
    type Select = ashpd::desktop::Request<()>;
    type Start = ashpd::desktop::Request<ashpd::desktop::screencast::Streams>;
    async fn select_sources(
        &self,
        session: &Self::Session,
        opts: SelectSourcesOptions,
    ) -> Result<Self::Select, ashpd::Error> {
        self.0.select_sources(session, opts).await
    }
    async fn start(&self, session: &Self::Session) -> Result<Self::Start, ashpd::Error> {
        self.0.start(session, None, Default::default()).await
    }
}

/// SelectSources under the shared consent `deadline`: awaits the reply's
/// verdict, maps Cancelled to `Denied` and a missed deadline to `TimedOut`,
/// and closes the session on any error.
async fn do_select<A: ScreencastApi>(
    api: &A,
    session: &A::Session,
    opts: SelectSourcesOptions,
    deadline: Instant,
) -> Result<(), PortalError> {
    let timed = tokio::time::timeout_at(deadline.into(), async {
        let request = api.select_sources(session, opts).await?;
        request.response()
    })
    .await;
    close_on_error(session, resolve("select_sources", timed)).await
}

/// Start under the same `deadline`; yields the first selected stream and the
/// stream count, closing the session on any error (including "no streams").
async fn do_start<A: ScreencastApi>(
    api: &A,
    session: &A::Session,
    deadline: Instant,
) -> Result<(StreamInfo, usize), PortalError> {
    let timed = tokio::time::timeout_at(deadline.into(), async {
        let request = api.start(session).await?;
        request.streams()
    })
    .await;
    let outcome = resolve("start (consent)", timed).and_then(|mut streams| {
        let count = streams.len();
        if streams.is_empty() {
            Err(PortalError::Failed {
                stage: "start (consent)",
                message: "portal returned no streams".to_string(),
            })
        } else {
            Ok((streams.remove(0), count))
        }
    });
    close_on_error(session, outcome).await
}

impl PortalCreated {
    /// Request monitor capture (not window). Cursor: hidden when offered
    /// (pointer position is tracked separately by A-Eye; see CAPTURE.md),
    /// else embedded, else the portal default.
    ///
    /// The interactive picker may appear during SelectSources (xdph) or
    /// during Start (others), so the whole consent budget (`budget`, capped at
    /// [`CONSENT_TIMEOUT`]) starts here and spans both calls. The SelectSources
    /// *response* is awaited: ashpd returns `Ok(Request)` even when the portal
    /// answered Cancelled, which only surfaces through `response()`.
    pub async fn select_monitor(mut self, budget: Duration) -> Result<PortalSelected, PortalError> {
        let deadline = Instant::now() + budget.min(CONSENT_TIMEOUT);
        let cursor = {
            let offered = self
                .facts
                .available_cursor_modes
                .clone()
                .unwrap_or_default();
            if offered.contains("Hidden") {
                Some(CursorMode::Hidden)
            } else if offered.contains("Embedded") {
                Some(CursorMode::Embedded)
            } else {
                None
            }
        };
        self.facts.chosen_cursor_mode = Some(
            cursor
                .map(|c| format!("{c:?}"))
                .unwrap_or_else(|| "portal default".to_string()),
        );
        let opts = SelectSourcesOptions::default()
            .set_sources(BitFlags::from(SourceType::Monitor))
            .set_multiple(false)
            .set_cursor_mode(cursor)
            .set_persist_mode(PersistMode::DoNot)
            .set_restore_token(None);
        let r = do_select(&RealApi(&self.proxy), &self.session, opts, deadline).await;
        r.map(|()| PortalSelected {
            proxy: self.proxy,
            session: self.session,
            facts: self.facts,
            deadline,
        })
    }
}

impl PortalSelected {
    /// Start the session and wait for the remaining consent budget. Always
    /// closes the session on any non-success path.
    pub async fn start(self) -> Result<StartedSession, PortalError> {
        let PortalSelected {
            proxy,
            session,
            facts,
            deadline,
        } = self;
        match do_start(&RealApi(&proxy), &session, deadline).await {
            Ok((stream, stream_count)) => Ok(StartedSession {
                proxy,
                session,
                facts,
                stream,
                confirmed_at: Instant::now(),
                stream_count,
            }),
            Err(e) => Err(e),
        }
    }
}

impl StartedSession {
    /// Open the PipeWire remote for this session. Needs a started session by type.
    pub async fn open_pipe_wire_remote(&self) -> Result<CaptureTarget, PortalError> {
        let fd = with_timeout(
            "open_pipe_wire_remote",
            self.proxy
                .open_pipe_wire_remote(&self.session, Default::default()),
        )
        .await?;
        Ok(CaptureTarget {
            fd,
            node_id: self.stream.node_id,
            confirmed_at: self.confirmed_at,
        })
    }

    /// Close the portal session (also revokes the stream).
    pub async fn close(self) -> Result<(), PortalError> {
        with_timeout("close session", self.session.close()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_response_is_denied_not_failure() {
        let e = classify("start", ashpd::Error::Response(ResponseError::Cancelled));
        assert!(matches!(e, PortalError::Denied));
    }

    #[test]
    fn missing_interface_is_unavailable() {
        let name =
            ashpd::zbus::names::OwnedInterfaceName::try_from("org.freedesktop.portal.ScreenCast")
                .unwrap();
        let e = classify("bind", ashpd::Error::PortalNotFound(name));
        match e {
            PortalError::Unavailable(m) => assert!(m.contains("ScreenCast")),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn other_errors_keep_their_stage() {
        let e = classify("create_session", ashpd::Error::NoResponse);
        match e {
            PortalError::Failed { stage, message } => {
                assert_eq!(stage, "create_session");
                assert!(!message.is_empty());
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    struct FakeSession(std::cell::Cell<u32>);
    impl Closable for FakeSession {
        async fn close_it(&self) -> Result<(), ashpd::Error> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }

    use std::cell::RefCell;

    type SelectOutcome = Result<(), ashpd::Error>;
    type StartOutcome = Result<Vec<StreamInfo>, ashpd::Error>;

    struct FakeSelectReply(RefCell<Option<SelectOutcome>>);
    impl SelectReply for FakeSelectReply {
        fn response(&self) -> SelectOutcome {
            self.0.borrow_mut().take().expect("response read once")
        }
    }
    struct FakeStartReply(RefCell<Option<StartOutcome>>);
    impl StartReply for FakeStartReply {
        fn streams(&self) -> StartOutcome {
            self.0.borrow_mut().take().expect("streams read once")
        }
    }

    /// Fake portal. `select_*`: the call itself returns Ok(reply) after
    /// `select_delay`, with the verdict hidden inside the reply (as ashpd does).
    struct FakeApi {
        select_delay: Duration,
        select_verdict: RefCell<Option<SelectOutcome>>,
        start_hangs: bool,
        start_verdict: RefCell<Option<StartOutcome>>,
    }
    impl FakeApi {
        fn new(select: SelectOutcome) -> Self {
            Self {
                select_delay: Duration::ZERO,
                select_verdict: RefCell::new(Some(select)),
                start_hangs: false,
                start_verdict: RefCell::new(None),
            }
        }
    }
    impl ScreencastApi for FakeApi {
        type Session = FakeSession;
        type Select = FakeSelectReply;
        type Start = FakeStartReply;
        async fn select_sources(
            &self,
            _: &FakeSession,
            _: SelectSourcesOptions,
        ) -> Result<FakeSelectReply, ashpd::Error> {
            tokio::time::sleep(self.select_delay).await;
            Ok(FakeSelectReply(RefCell::new(self.select_verdict.take())))
        }
        async fn start(&self, _: &FakeSession) -> Result<FakeStartReply, ashpd::Error> {
            if self.start_hangs {
                std::future::pending::<()>().await;
            }
            Ok(FakeStartReply(RefCell::new(self.start_verdict.take())))
        }
    }

    fn cancelled() -> ashpd::Error {
        ashpd::Error::Response(ResponseError::Cancelled)
    }

    fn stream(node: u32) -> StreamInfo {
        StreamInfo {
            node_id: node,
            logical_size: Some((1600, 900)),
            position: Some((0, 0)),
            source_type: Some("Monitor".into()),
            stream_id: None,
        }
    }

    fn soon(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn b1_select_ok_request_with_cancelled_response_is_denied_and_closes() {
        let api = FakeApi::new(Err(cancelled()));
        let session = FakeSession(Default::default());
        let r = do_select(&api, &session, Default::default(), soon(1000)).await;
        assert!(matches!(r, Err(PortalError::Denied)), "got {r:?}");
        assert_eq!(session.0.get(), 1, "session must be closed after denial");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn b1_select_ok_keeps_session_open() {
        let api = FakeApi::new(Ok(()));
        let session = FakeSession(Default::default());
        assert!(do_select(&api, &session, Default::default(), soon(1000))
            .await
            .is_ok());
        assert_eq!(session.0.get(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn start_denied_no_streams_and_success_paths() {
        let session = FakeSession(Default::default());
        let mut api = FakeApi::new(Ok(()));
        *api.start_verdict.get_mut() = Some(Err(cancelled()));
        let r = do_start(&api, &session, soon(1000)).await;
        assert!(matches!(r, Err(PortalError::Denied)), "got {r:?}");
        assert_eq!(session.0.get(), 1);

        let session = FakeSession(Default::default());
        let mut api = FakeApi::new(Ok(()));
        *api.start_verdict.get_mut() = Some(Ok(vec![]));
        let r = do_start(&api, &session, soon(1000)).await;
        assert!(matches!(r, Err(PortalError::Failed { .. })), "got {r:?}");
        assert_eq!(session.0.get(), 1, "no streams must close the session");

        let session = FakeSession(Default::default());
        let mut api = FakeApi::new(Ok(()));
        *api.start_verdict.get_mut() = Some(Ok(vec![stream(42), stream(43)]));
        let (first, count) = do_start(&api, &session, soon(1000)).await.unwrap();
        assert_eq!((first.node_id, count), (42, 2));
        assert_eq!(session.0.get(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn b2_one_deadline_spans_select_and_start() {
        // SelectSources takes 60ms of a 120ms budget and succeeds; Start never
        // answers. Start must time out at the *shared* deadline, not get a
        // fresh budget, and the session must be closed.
        let mut api = FakeApi::new(Ok(()));
        api.select_delay = Duration::from_millis(60);
        api.start_hangs = true;
        let session = FakeSession(Default::default());
        let t0 = Instant::now();
        let deadline = t0 + Duration::from_millis(120);
        do_select(&api, &session, Default::default(), deadline)
            .await
            .expect("select within budget");
        assert_eq!(session.0.get(), 0);
        let r = do_start(&api, &session, deadline).await;
        let elapsed = t0.elapsed();
        assert!(matches!(r, Err(PortalError::TimedOut)), "got {r:?}");
        assert!(
            elapsed >= Duration::from_millis(110) && elapsed < Duration::from_millis(400),
            "expected ~120ms total, got {elapsed:?}"
        );
        assert_eq!(session.0.get(), 1, "timeout must close the session");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn b2_select_slower_than_budget_times_out_and_closes() {
        let mut api = FakeApi::new(Ok(()));
        api.select_delay = Duration::from_secs(30);
        let session = FakeSession(Default::default());
        let r = do_select(&api, &session, Default::default(), soon(50)).await;
        assert!(matches!(r, Err(PortalError::TimedOut)), "got {r:?}");
        assert_eq!(session.0.get(), 1);
    }

    #[test]
    fn resolve_maps_portal_cancel_to_denied() {
        // B1: ashpd returns Ok(Request) and the error only appears in response().
        let r: Result<(), PortalError> = resolve(
            "select_sources",
            Ok(Err(ashpd::Error::Response(ResponseError::Cancelled))),
        );
        assert!(matches!(r, Err(PortalError::Denied)));
        let other: Result<(), PortalError> = resolve(
            "select_sources",
            Ok(Err(ashpd::Error::Response(ResponseError::Other))),
        );
        assert!(matches!(other, Err(PortalError::Failed { .. })));
        assert!(resolve("select_sources", Ok(Ok(7))).is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolve_maps_elapsed_to_timed_out() {
        let timed = tokio::time::timeout(
            Duration::from_millis(1),
            std::future::pending::<Result<(), ashpd::Error>>(),
        )
        .await;
        assert!(matches!(
            resolve("select_sources", timed),
            Err(PortalError::TimedOut)
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn response_error_closes_session_and_yields_no_success() {
        let denied = FakeSession(Default::default());
        let r: Result<(), PortalError> = close_on_error(
            &denied,
            resolve(
                "select_sources",
                Ok(Err(ashpd::Error::Response(ResponseError::Cancelled))),
            ),
        )
        .await;
        assert!(matches!(r, Err(PortalError::Denied)));
        assert_eq!(denied.0.get(), 1, "session must be closed on denial");

        let ok = FakeSession(Default::default());
        let r = close_on_error(&ok, resolve("start", Ok(Ok(1)))).await;
        assert_eq!(r.unwrap(), 1);
        assert_eq!(ok.0.get(), 0, "session must stay open on success");
    }

    #[test]
    fn consent_timeout_is_capped_at_90s() {
        assert_eq!(CONSENT_TIMEOUT, Duration::from_secs(90));
        assert_eq!(PortalError::TimedOut.to_string(), "no consent within 90s");
    }
}
