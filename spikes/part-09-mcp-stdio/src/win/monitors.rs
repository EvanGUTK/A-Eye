//! Monitor enumeration and cursor position.
//!
//! Adapted from spike 17 (`enumerate_monitors`, cursor lookup). Spike 17 is a bin-only crate with
//! private, print-as-you-go code, and "no shared crates until a spike graduates" applies, so this
//! is a deliberate copy with the output plumbing removed. When the spikes graduate, fold both
//! copies into one capture crate.

use std::mem::size_of;

use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, MONITOR_DEFAULTTONULL,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, MONITORINFOF_PRIMARY};

use crate::backend::{Cursor, CursorOnMonitor, MonitorInfo, ToolError};

const BASE_DPI: u32 = 96;

#[derive(Clone)]
pub struct Monitor {
    pub handle: HMONITOR,
    pub device: String,
    pub rect: RECT,
    pub primary: bool,
    pub effective_dpi: u32,
}

impl Monitor {
    pub fn id(&self) -> String {
        monitor_id(self.handle)
    }

    pub fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }

    pub fn height(&self) -> i32 {
        self.rect.bottom - self.rect.top
    }

    pub fn info(&self) -> MonitorInfo {
        MonitorInfo {
            id: self.id(),
            device: self.device.clone(),
            x: self.rect.left,
            y: self.rect.top,
            width: self.width(),
            height: self.height(),
            effective_dpi: self.effective_dpi,
            scale_percent: self.effective_dpi * 100 / BASE_DPI,
            primary: self.primary,
        }
    }
}

/// Instance-scoped: an HMONITOR is only meaningful to this process while the display layout holds.
fn monitor_id(handle: HMONITOR) -> String {
    format!("hm-{:x}", handle.0 as usize)
}

unsafe extern "system" fn collect_monitor(
    handle: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: lparam is the &mut Vec<HMONITOR> passed by `enumerate` for this call only.
    let handles = unsafe { &mut *(lparam.0 as *mut Vec<HMONITOR>) };
    handles.push(handle);
    BOOL(1)
}

fn os_error(what: &str, e: windows::core::Error) -> ToolError {
    ToolError::new(format!("{what} failed: {e}"))
}

pub fn enumerate() -> Result<Vec<Monitor>, ToolError> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only writes through the pointer while this call runs.
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM(&mut handles as *mut Vec<HMONITOR> as isize),
        )
        .ok()
        .map_err(|e| os_error("EnumDisplayMonitors", e))?;
    }
    let mut monitors = Vec::with_capacity(handles.len());
    for handle in handles {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize says it is the extended form.
        unsafe { GetMonitorInfoW(handle, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }
            .ok()
            .map_err(|e| os_error("GetMonitorInfoW", e))?;
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        // SAFETY: valid monitor handle and out-pointers.
        unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
            .map_err(|e| os_error("GetDpiForMonitor", e))?;
        let name_len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        monitors.push(Monitor {
            handle,
            device: String::from_utf16_lossy(&info.szDevice[..name_len]),
            rect: info.monitorInfo.rcMonitor,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            effective_dpi: dpi_x,
        });
    }
    Ok(monitors)
}

/// Looks the id up in a fresh enumeration, so an id from before a display change is rejected
/// instead of silently pointing at something else.
pub fn find(id: &str) -> Result<Monitor, ToolError> {
    enumerate()?
        .into_iter()
        .find(|m| m.id() == id)
        .ok_or_else(|| {
            ToolError::new(format!(
                "unknown monitor_id '{id}': call aeye_list_monitors for current ids"
            ))
        })
}

pub fn cursor() -> Result<Cursor, ToolError> {
    let mut point = POINT::default();
    // SAFETY: valid out-pointer.
    unsafe { GetCursorPos(&mut point) }.map_err(|e| os_error("GetCursorPos", e))?;
    // SAFETY: plain query.
    let at = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONULL) };
    let on_monitor = enumerate()?
        .into_iter()
        .find(|m| m.handle == at)
        .map(|m| CursorOnMonitor {
            monitor_id: m.id(),
            x: point.x - m.rect.left,
            y: point.y - m.rect.top,
        });
    Ok(Cursor {
        x: point.x,
        y: point.y,
        on_monitor,
    })
}
