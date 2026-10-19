//! Platform-neutral contract between the MCP layer and the native capture/overlay code.
//! Nothing here touches the OS, so the protocol and tool logic can be tested without a desktop.

use std::fmt;
use std::time::Duration;

/// A failure the agent can read and act on; it becomes an `isError` tool result.
#[derive(Debug, PartialEq, Eq)]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Instance-scoped: valid for this server process only, never assumed stable.
    pub id: String,
    pub device: String,
    /// Virtual-desktop rectangle in physical pixels.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub effective_dpi: u32,
    pub scale_percent: u32,
    pub primary: bool,
}

/// Tightly packed RGBA8, top-down.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorOnMonitor {
    pub monitor_id: String,
    /// Position relative to that monitor's top-left corner.
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Virtual-desktop physical pixels.
    pub x: i32,
    pub y: i32,
    pub on_monitor: Option<CursorOnMonitor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub struct OverlayStarted {
    pub handle: String,
    pub monitor_id: String,
    pub expires_in: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayEnd {
    Stopped,
    Expired,
    LocalHotkey,
    ClosedExternally,
    ServerShutdown,
}

impl OverlayEnd {
    pub fn label(self) -> &'static str {
        match self {
            OverlayEnd::Stopped => "stopped",
            OverlayEnd::Expired => "expired",
            OverlayEnd::LocalHotkey => "local_hotkey",
            OverlayEnd::ClosedExternally => "closed_externally",
            OverlayEnd::ServerShutdown => "server_shutdown",
        }
    }
}

pub struct OverlayStopped {
    pub handle: String,
    /// What ended the overlay. `Stopped` means this call ended it; anything else means it had
    /// already ended on its own before this call.
    pub ended_by: OverlayEnd,
}

pub trait Backend {
    fn list_monitors(&mut self) -> Result<Vec<MonitorInfo>, ToolError>;
    fn capture_monitor(&mut self, monitor_id: &str) -> Result<Frame, ToolError>;
    fn cursor(&mut self) -> Result<Cursor, ToolError>;
    fn start_overlay(
        &mut self,
        monitor_id: &str,
        color: Rgb,
        lifetime: Duration,
    ) -> Result<OverlayStarted, ToolError>;
    fn stop_overlay(&mut self, handle: &str) -> Result<OverlayStopped, ToolError>;
    /// Called once when the client closes stdin: nothing may outlive the session.
    fn shutdown(&mut self);
}
