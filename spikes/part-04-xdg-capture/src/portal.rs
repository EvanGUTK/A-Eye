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

/// Hard upper bound for the consent step. The run can never hang past this.
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
        let timed = tokio::time::timeout_at(deadline.into(), async {
            let request = self.proxy.select_sources(&self.session, opts).await?;
            request.response()
        })
        .await;
        let r = close_on_error(&self.session, resolve("select_sources", timed)).await;
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
        let timed = tokio::time::timeout_at(deadline.into(), async {
            let request = proxy.start(&session, None, Default::default()).await?;
            request.response()
        })
        .await;
        let outcome =
            resolve("start (consent)", timed).and_then(|streams| match streams.streams().first() {
                None => Err(PortalError::Failed {
                    stage: "start (consent)",
                    message: "portal returned no streams".to_string(),
                }),
                Some(s) => Ok((StreamInfo::from_stream(s), streams.streams().len())),
            });
        match close_on_error(&session, outcome).await {
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
