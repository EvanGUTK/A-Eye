//! Adapted excerpts from part-18/src/win.rs: monitor bounds, window styles and alpha painting.
//! Spikes remain standalone (no shared crate before graduation). Only the overlay primitives
//! are copied, not part-18's capability runner, self-test, control window or hotkey code.
//! Changes: a distinct class, a hidden initial window, show/hide toggles, RAII destruction,
//! checked bitmap allocation and GDI handles, and WM_CLOSE exits the owning message loop.
//! Fold these primitives into one native crate when the spikes graduate.

use std::mem::size_of;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
    MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HGDIOBJ, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IsWindowVisible, PostQuitMessage,
    RegisterClassW, SetWindowPos, ShowWindow, UpdateLayeredWindow, HMENU, HTTRANSPARENT,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA,
    WINDOW_EX_STYLE, WM_CLOSE, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const BORDER_THICKNESS_PX: i32 = 6;
const ORANGE_RGB: (u8, u8, u8) = (255, 140, 0);
const CLASS_NAME: PCWSTR = w!("AEyeSpike07Overlay");

#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

fn primary_monitor_region() -> Result<Region, String> {
    // SAFETY: plain query; DEFAULTTOPRIMARY never returns null.
    let monitor = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: cbSize is set and info outlives the call.
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .ok()
        .map_err(|e| format!("GetMonitorInfoW: {e}"))?;
    let r = info.rcMonitor;
    Ok(Region {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    })
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_CLOSE => {
            // Destruction remains with the owning Overlay guard.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: forward unhandled messages unchanged.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub struct Overlay {
    pub hwnd: HWND,
    pub region: Region,
}

impl Overlay {
    pub fn new() -> Result<Self, String> {
        let region = primary_monitor_region()?;
        register_class(CLASS_NAME)?;
        let style =
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
        let overlay = Self {
            hwnd: create_window(CLASS_NAME, style, region)?,
            region,
        };
        paint_border(overlay.hwnd, region)?;
        Ok(overlay)
    }

    pub fn set_visible(&self, visible: bool) -> Result<(), String> {
        // SAFETY: this thread owns hwnd for the lifetime of self.
        unsafe {
            if visible {
                SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
                .map_err(|error| format!("SetWindowPos: {error}"))?;
            }
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
            if IsWindowVisible(self.hwnd).as_bool() != visible {
                return Err("overlay visibility did not change as requested".into());
            }
        }
        Ok(())
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        // SAFETY: the guard stays on the thread that created the window.
        if let Err(error) = unsafe { DestroyWindow(self.hwnd) } {
            eprintln!("DestroyWindow: {error}");
        }
    }
}

fn register_class(name: PCWSTR) -> Result<(), String> {
    // SAFETY: null module name returns this executable's handle.
    let instance =
        unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: name,
        ..Default::default()
    };
    // SAFETY: class is fully initialised and its pointers outlive the call.
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(format!(
            "RegisterClassW: {}",
            windows::core::Error::from_thread()
        ));
    }
    Ok(())
}

fn create_window(class: PCWSTR, ex_style: WINDOW_EX_STYLE, r: Region) -> Result<HWND, String> {
    // SAFETY: null module name returns this executable's handle.
    let instance =
        unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
    // SAFETY: class is registered; all pointers are valid for the call.
    unsafe {
        CreateWindowExW(
            ex_style,
            class,
            w!("A-Eye spike 07 overlay demonstration"),
            WS_POPUP,
            r.x,
            r.y,
            r.w,
            r.h,
            None,
            None::<HMENU>,
            Some(instance.into()),
            None,
        )
    }
    .map_err(|e| format!("CreateWindowExW: {e}"))
}

/// Paints a per-pixel-alpha bitmap: an opaque orange frame, fully transparent inside.
fn paint_border(hwnd: HWND, r: Region) -> Result<(), String> {
    let (width, height) = (r.w, r.h);
    let pixel_count = usize::try_from(width)
        .ok()
        .zip(usize::try_from(height).ok())
        .and_then(|(w, h)| w.checked_mul(h))
        .filter(|count| *count > 0 && *count <= isize::MAX as usize / size_of::<u32>())
        .ok_or("invalid monitor bitmap dimensions")?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: GDI calls below follow the documented create/select/use/deselect/delete order.
    unsafe {
        let screen = GetDC(None);
        if screen.0.is_null() {
            return Err("GetDC failed".into());
        }
        let mem = CreateCompatibleDC(Some(screen));
        if mem.0.is_null() {
            ReleaseDC(None, screen);
            return Err("CreateCompatibleDC failed".into());
        }
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return Err(format!("CreateDIBSection: {e}"));
            }
        };
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return Err("DIB storage is null".into());
        }
        let old = SelectObject(mem, HGDIOBJ(bitmap.0));
        if old.0.is_null() || old.0 as isize == -1 {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return Err("SelectObject / DIB storage failed".into());
        }

        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);
        // Premultiplied BGRA with alpha 255 is just 0xAARRGGBB.
        let orange = 0xff00_0000u32
            | (u32::from(ORANGE_RGB.0) << 16)
            | (u32::from(ORANGE_RGB.1) << 8)
            | u32::from(ORANGE_RGB.2);
        for (i, px) in pixels.iter_mut().enumerate() {
            let (x, y) = ((i % width as usize) as i32, (i / width as usize) as i32);
            let on_border = x < BORDER_THICKNESS_PX
                || y < BORDER_THICKNESS_PX
                || x >= width - BORDER_THICKNESS_PX
                || y >= height - BORDER_THICKNESS_PX;
            *px = if on_border { orange } else { 0 };
        }

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let dst = POINT { x: r.x, y: r.y };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let src = POINT { x: 0, y: 0 };
        let result = UpdateLayeredWindow(
            hwnd,
            Some(screen),
            Some(&dst),
            Some(&size),
            Some(mem),
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result.map_err(|e| format!("UpdateLayeredWindow: {e}"))
    }
}
