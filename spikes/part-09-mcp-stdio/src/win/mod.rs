//! The Windows implementation of `Backend`: monitors, Windows.Graphics.Capture and overlays.

mod capture;
mod monitors;
mod overlay;

use std::time::Duration;

use windows::Graphics::Capture::GraphicsCaptureSession;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

use crate::backend::{
    Backend, Cursor, Frame, MonitorInfo, OverlayStarted, OverlayStopped, Rgb, ToolError,
};

pub struct WindowsBackend {
    /// Created on the first capture; a D3D11 device is not needed for the other tools.
    gpu: Option<capture::Gpu>,
    overlays: overlay::OverlayManager,
}

impl WindowsBackend {
    pub fn new() -> Result<Self, String> {
        // Per-monitor v2 makes every rect, size and cursor position physical pixels. It must be
        // set before any window exists, and a wrong awareness level would silently skew every
        // coordinate, so failure is fatal.
        // SAFETY: called once at startup before any window exists.
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
            .map_err(|e| format!("could not set per-monitor DPI awareness: {e}"))?;
        // SAFETY: first WinRT init on this thread.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|e| format!("RoInitialize failed: {e}"))?;
        Ok(Self {
            gpu: None,
            overlays: overlay::OverlayManager::new(),
        })
    }

    fn gpu(&mut self) -> Result<&capture::Gpu, ToolError> {
        if self.gpu.is_none() {
            if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
                return Err(ToolError::new(
                    "Windows.Graphics.Capture is not supported on this system",
                ));
            }
            self.gpu = Some(capture::Gpu::new()?);
        }
        Ok(self.gpu.as_ref().expect("initialised just above"))
    }
}

impl Backend for WindowsBackend {
    fn list_monitors(&mut self) -> Result<Vec<MonitorInfo>, ToolError> {
        Ok(monitors::enumerate()?
            .iter()
            .map(monitors::Monitor::info)
            .collect())
    }

    fn capture_monitor(&mut self, monitor_id: &str) -> Result<Frame, ToolError> {
        let monitor = monitors::find(monitor_id)?;
        capture::capture(self.gpu()?, &monitor)
    }

    fn cursor(&mut self) -> Result<Cursor, ToolError> {
        monitors::cursor()
    }

    fn start_overlay(
        &mut self,
        monitor_id: &str,
        color: Rgb,
        lifetime: Duration,
    ) -> Result<OverlayStarted, ToolError> {
        let monitor = monitors::find(monitor_id)?;
        self.overlays.start(&monitor, color, lifetime)
    }

    fn stop_overlay(&mut self, handle: &str) -> Result<OverlayStopped, ToolError> {
        self.overlays.stop(handle)
    }

    fn shutdown(&mut self) {
        self.overlays.shutdown();
    }
}
