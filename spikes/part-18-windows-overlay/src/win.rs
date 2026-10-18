//! Windows-only implementation of spike 18.

use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject,
    GetDC, GetMonitorInfoW, GetPixel, MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CAPTUREBLT, DIB_RGB_COLORS,
    HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, SRCCOPY,
};
use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, SendInput, UnregisterHotKey, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VIRTUAL_KEY,
    VK_CONTROL, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClassNameW,
    GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindow,
    IsWindowVisible, PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow,
    WindowFromPoint, GWL_EXSTYLE, HMENU, HTTRANSPARENT, HWND_TOPMOST, MSG, PM_REMOVE,
    SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOW, SW_SHOWNOACTIVATE,
    ULW_ALPHA, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_HOTKEY, WM_NCHITTEST, WM_QUIT, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const BORDER_THICKNESS_PX: i32 = 6;
/// Opaque orange, as (r, g, b). Alpha is 255, so a correct composite shows it unblended.
const ORANGE_RGB: (u8, u8, u8) = (255, 140, 0);
/// Per-channel tolerance when comparing the composited screen pixel with ORANGE_RGB.
const COLOUR_TOLERANCE: i32 = 3;
const DEFAULT_HOLD: Duration = Duration::from_secs(3);
/// Time given to the window manager to settle before the focus and pixel checks.
const SETTLE: Duration = Duration::from_millis(500);
/// When the self-test injects the stop hotkey, relative to the window being shown.
const SELFTEST_HOTKEY_AT: Duration = Duration::from_millis(1200);
/// How long past the hold the self-test waits for the hotkey before giving up and closing.
const SELFTEST_GRACE: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const HOTKEY_ID: i32 = 1;
const VK_Q: u16 = 0x51;
const STOP_HOTKEY_EVENTS: u32 = 6;
const CLASS_NAME: PCWSTR = w!("AEyeSpike18Overlay");
const CONTROL_CLASS_NAME: PCWSTR = w!("AEyeSpike18Control");
const CONTROL_SIZE_PX: i32 = 200;
const CONTROL_OFFSET_PX: i32 = 50;
const CLASS_NAME_BUF: usize = 128;
const CLR_INVALID: u32 = 0xffff_ffff;

const STOP_NONE: u8 = 0;
const STOP_TIMEOUT: u8 = 1;
const STOP_HOTKEY: u8 = 2;
const STOP_CTRL_C: u8 = 3;
const STOP_WM_CLOSE: u8 = 4;
const STOP_SELFTEST_FALLBACK: u8 = 5;

static STOP_REASON: AtomicU8 = AtomicU8::new(STOP_NONE);
/// The overlay HWND for the console handler thread; 0 when no overlay exists.
static OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);

fn stop_reason_label(code: u8) -> &'static str {
    match code {
        STOP_TIMEOUT => "hold timeout elapsed",
        STOP_HOTKEY => "Ctrl+Alt+Q global hotkey",
        STOP_CTRL_C => "Ctrl+C / Ctrl+Break in the console",
        STOP_WM_CLOSE => "WM_CLOSE",
        STOP_SELFTEST_FALLBACK => "self-test fallback (hotkey did not fire)",
        _ => "none",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Fail,
    Unverified,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Unverified => "UNVERIFIED",
        }
    }
}

struct Check {
    id: &'static str,
    name: &'static str,
    status: Status,
    detail: String,
}

fn check(id: &'static str, name: &'static str, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        id,
        name,
        status: if ok { Status::Pass } else { Status::Fail },
        detail: detail.into(),
    }
}

fn unverified(id: &'static str, name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        id,
        name,
        status: Status::Unverified,
        detail: detail.into(),
    }
}

/// Identity of a top-level window. Titles are deliberately not recorded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct WindowId {
    hwnd: isize,
    pid: u32,
}

fn describe(hwnd: HWND) -> (WindowId, String) {
    let mut class = [0u16; CLASS_NAME_BUF];
    // SAFETY: buffer is valid for CLASS_NAME_BUF u16s.
    let len = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
    let mut pid = 0u32;
    // SAFETY: valid out-pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (
        WindowId {
            hwnd: hwnd.0 as isize,
            pid,
        },
        String::from_utf16_lossy(&class[..len]),
    )
}

fn foreground_json(hwnd: HWND) -> Value {
    if hwnd.0.is_null() {
        return Value::Null;
    }
    let (id, class) = describe(hwnd);
    json!({ "hwnd": format!("0x{:x}", id.hwnd), "class": class, "pid": id.pid })
}

#[derive(Clone, Copy, Debug)]
struct Region {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
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

fn parse_region(s: &str) -> Option<Region> {
    let v: Vec<i32> = s
        .split(',')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    match v.as_slice() {
        [x, y, w, h] if *w > 2 * BORDER_THICKNESS_PX && *h > 2 * BORDER_THICKNESS_PX => {
            Some(Region {
                x: *x,
                y: *y,
                w: *w,
                h: *h,
            })
        }
        _ => None,
    }
}

struct Args {
    region: Option<Region>,
    hold: Duration,
    hold_forever: bool,
    control: bool,
    selftest_hotkey: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        region: None,
        hold: DEFAULT_HOLD,
        hold_forever: false,
        control: false,
        selftest_hotkey: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--region" => {
                let v = it.next().unwrap_or_default();
                args.region = Some(
                    parse_region(&v).ok_or_else(|| format!("bad --region '{v}' (want x,y,w,h)"))?,
                );
            }
            "--hold-secs" => {
                let v = it.next().unwrap_or_default();
                let secs: u64 = v.parse().map_err(|_| format!("bad --hold-secs '{v}'"))?;
                args.hold_forever = secs == 0;
                args.hold = Duration::from_secs(secs);
            }
            "--control" => args.control = true,
            "--selftest-hotkey" => args.selftest_hotkey = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // Belt and braces with WS_EX_TRANSPARENT: never claim mouse hits.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_ID => {
            STOP_REASON.store(STOP_HOTKEY, Ordering::SeqCst);
            // SAFETY: hwnd is this window, on its own thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = STOP_REASON.compare_exchange(
                STOP_NONE,
                STOP_WM_CLOSE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            // SAFETY: hwnd is this window, on its own thread.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: always valid on a UI thread.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: forwarding the unhandled message unchanged.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> BOOL {
    if ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT {
        STOP_REASON.store(STOP_CTRL_C, Ordering::SeqCst);
        let hwnd = OVERLAY_HWND.load(Ordering::SeqCst);
        if hwnd != 0 {
            // SAFETY: PostMessageW is safe to call from any thread.
            let _ =
                unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        return BOOL(1);
    }
    BOOL(0)
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
            w!("A-Eye spike 18"),
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
        let mem = CreateCompatibleDC(Some(screen));
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(mem);
                ReleaseDC(None, screen);
                return Err(format!("CreateDIBSection: {e}"));
            }
        };
        let old = SelectObject(mem, HGDIOBJ(bitmap.0));

        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (width * height) as usize);
        // Premultiplied BGRA with alpha 255 is just 0xAARRGGBB.
        let orange = 0xff00_0000u32
            | (u32::from(ORANGE_RGB.0) << 16)
            | (u32::from(ORANGE_RGB.1) << 8)
            | u32::from(ORANGE_RGB.2);
        for (i, px) in pixels.iter_mut().enumerate() {
            let (x, y) = ((i as i32) % width, (i as i32) / width);
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

/// Reads one composited screen pixel (includes layered windows via CAPTUREBLT) as (r, g, b).
fn screen_pixel(x: i32, y: i32) -> Option<(u8, u8, u8)> {
    // SAFETY: GDI calls follow the documented create/use/delete order.
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, 1, 1);
        let old = SelectObject(mem, HGDIOBJ(bitmap.0));
        let copied = BitBlt(mem, 0, 0, 1, 1, Some(screen), x, y, SRCCOPY | CAPTUREBLT).is_ok();
        let colour = GetPixel(mem, 0, 0).0;
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        // COLORREF is 0x00BBGGRR.
        (copied && colour != CLR_INVALID).then_some((
            (colour & 0xff) as u8,
            ((colour >> 8) & 0xff) as u8,
            ((colour >> 16) & 0xff) as u8,
        ))
    }
}

fn is_orange(p: (u8, u8, u8)) -> bool {
    let near = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs() <= COLOUR_TOLERANCE;
    near(p.0, ORANGE_RGB.0) && near(p.1, ORANGE_RGB.1) && near(p.2, ORANGE_RGB.2)
}

fn key_event(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    }
}

/// Injects Ctrl+Alt+Q. Returns how many of the events the system accepted.
fn inject_stop_hotkey() -> u32 {
    let events = [
        key_event(VK_CONTROL.0, false),
        key_event(VK_MENU.0, false),
        key_event(VK_Q, false),
        key_event(VK_Q, true),
        key_event(VK_MENU.0, true),
        key_event(VK_CONTROL.0, true),
    ];
    // SAFETY: events is a valid slice of INPUT.
    unsafe { SendInput(&events, size_of::<INPUT>() as i32) }
}

/// Returns false once WM_QUIT has been seen.
fn pump_messages() -> bool {
    let mut msg = MSG::default();
    // SAFETY: standard message pump on the thread that owns the windows.
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

/// Shows a window like the overlay but WITHOUT WS_EX_NOACTIVATE, using SW_SHOW, to see whether
/// this process can change the foreground window at all. If it cannot, "foreground unchanged"
/// on the real overlay proves nothing about NOACTIVATE.
fn run_control(region: Region) -> Check {
    const NAME: &str = "focus control: an activatable window changes the foreground";
    // SAFETY: plain query.
    let before = unsafe { GetForegroundWindow() };
    if before.0.is_null() {
        return unverified(
            "C1",
            NAME,
            "no foreground window before the control; cannot compare",
        );
    }
    let small = Region {
        x: region.x + CONTROL_OFFSET_PX,
        y: region.y + CONTROL_OFFSET_PX,
        w: CONTROL_SIZE_PX,
        h: CONTROL_SIZE_PX,
    };
    let hwnd = match create_window(CONTROL_CLASS_NAME, WS_EX_TOPMOST | WS_EX_TOOLWINDOW, small) {
        Ok(h) => h,
        Err(e) => return unverified("C1", NAME, format!("control window not created: {e}")),
    };
    // SAFETY: hwnd was just created on this thread.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };
    let shown = Instant::now();
    while shown.elapsed() < SETTLE {
        pump_messages();
        sleep(POLL_INTERVAL);
    }
    // SAFETY: plain query.
    let after = unsafe { GetForegroundWindow() };
    let changed = after != before;
    let (after_id, _) = describe(after);
    // SAFETY: hwnd is ours; afterwards the user's window is put back in front.
    unsafe {
        let _ = DestroyWindow(hwnd);
        pump_messages();
        let _ = SetForegroundWindow(before);
    }
    // SAFETY: plain query.
    let restored = unsafe { GetForegroundWindow() } == before;
    let detail = format!(
        "foreground {} after showing an ordinary activatable window (now {after_id:?}); original restored: {restored}",
        if changed { "CHANGED" } else { "did NOT change" },
    );
    if changed {
        Check {
            id: "C1",
            name: NAME,
            status: Status::Pass,
            detail,
        }
    } else {
        unverified(
            "C1",
            NAME,
            format!("{detail}; the focus check on the overlay is not discriminating"),
        )
    }
}

pub fn run() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let mut checks: Vec<Check> = Vec::new();

    // SAFETY: called once at startup before any window exists.
    let dpi = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    checks.push(check(
        "O0",
        "per-monitor DPI awareness v2 set",
        dpi.is_ok(),
        format!("{dpi:?}"),
    ));

    let region = match args.region.map(Ok).unwrap_or_else(primary_monitor_region) {
        Ok(r) => r,
        Err(e) => {
            checks.push(check("O1", "target region resolved", false, e));
            return finish(&checks, None, Value::Null, Value::Null, STOP_NONE);
        }
    };
    println!("target region (physical px): {region:?}");

    if let Err(e) = register_class(CLASS_NAME).and_then(|()| register_class(CONTROL_CLASS_NAME)) {
        checks.push(check("O1", "window classes registered", false, e));
        return finish(&checks, Some(region), Value::Null, Value::Null, STOP_NONE);
    }
    // SAFETY: handler is a valid extern "system" fn for the process lifetime.
    let handler_ok = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), true) }.is_ok();

    if args.control {
        checks.push(run_control(region));
    }

    // SAFETY: plain query.
    let fg_before = unsafe { GetForegroundWindow() };
    let fg_before_json = foreground_json(fg_before);
    let ex_style =
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
    let created = create_window(CLASS_NAME, ex_style, region)
        .and_then(|h| paint_border(h, region).map(|()| h));
    let hwnd = match created {
        Ok(h) => h,
        Err(e) => {
            checks.push(check("O1", "overlay window created and painted", false, e));
            return finish(
                &checks,
                Some(region),
                fg_before_json,
                Value::Null,
                STOP_NONE,
            );
        }
    };
    OVERLAY_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

    // SAFETY: hwnd is ours and lives on this thread.
    let hotkey_ok = unsafe {
        RegisterHotKey(
            Some(hwnd),
            HOTKEY_ID,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            u32::from(VK_Q),
        )
    }
    .is_ok();

    // SAFETY: hwnd is ours. SW_SHOWNOACTIVATE + SWP_NOACTIVATE keep the previous window active.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SET_WINDOW_POS_FLAGS(SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0),
        );
    }
    println!(
        "overlay shown. Stop with Ctrl+Alt+Q (hotkey registered: {hotkey_ok}) or Ctrl+C (handler: {handler_ok}); \
         auto-closes after {}.",
        if args.hold_forever { "never (--hold-secs 0)".to_string() } else { format!("{} s", args.hold.as_secs()) }
    );

    let mut fg_after_json = Value::Null;
    let mut checked = false;
    let mut hotkey_injected: Option<u32> = None;
    let started = Instant::now();
    while pump_messages() {
        let t = started.elapsed();
        // A Ctrl+C that arrived before the overlay existed only latched the reason; honour it.
        // SAFETY: IsWindow accepts any handle value; the window is ours, on its own thread.
        if STOP_REASON.load(Ordering::SeqCst) == STOP_CTRL_C
            && unsafe { IsWindow(Some(hwnd)) }.as_bool()
        {
            unsafe {
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
                let _ = DestroyWindow(hwnd);
            }
            continue;
        }
        if !checked && t >= SETTLE {
            checked = true;
            // SAFETY: plain query.
            let fg_after = unsafe { GetForegroundWindow() };
            fg_after_json = foreground_json(fg_after);
            checks.extend(live_checks(hwnd, region, fg_before, fg_after));
        }
        // Never inject the chord unless this process owns it: otherwise it would reach another app.
        if args.selftest_hotkey && hotkey_ok && hotkey_injected.is_none() && t >= SELFTEST_HOTKEY_AT
        {
            hotkey_injected = Some(inject_stop_hotkey());
        }
        // In the self-test the hotkey is the intended stop path; the timeout is only a fallback.
        let deadline = if args.selftest_hotkey {
            Some(args.hold + SELFTEST_GRACE)
        } else {
            (!args.hold_forever).then_some(args.hold)
        };
        if deadline.is_some_and(|d| t >= d) {
            let reason = if args.selftest_hotkey {
                STOP_SELFTEST_FALLBACK
            } else {
                STOP_TIMEOUT
            };
            let _ =
                STOP_REASON.compare_exchange(STOP_NONE, reason, Ordering::SeqCst, Ordering::SeqCst);
            // SAFETY: hwnd is ours, on its own thread.
            unsafe {
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_ID);
                let _ = DestroyWindow(hwnd);
            }
        }
        sleep(POLL_INTERVAL);
    }
    OVERLAY_HWND.store(0, Ordering::SeqCst);

    let reason = STOP_REASON.load(Ordering::SeqCst);
    // SAFETY: IsWindow accepts any handle value.
    let gone = !unsafe { IsWindow(Some(hwnd)) }.as_bool();
    checks.push(check(
        "O6",
        "overlay closed locally",
        gone && reason != STOP_NONE,
        format!(
            "window destroyed: {gone}; stop path: {}",
            stop_reason_label(reason)
        ),
    ));
    const O7: &str = "Ctrl+Alt+Q global hotkey closes the overlay without focus";
    checks.push(if args.selftest_hotkey {
        check(
            "O7",
            O7,
            reason == STOP_HOTKEY,
            format!(
                "injected {} of {STOP_HOTKEY_EVENTS} key events (hotkey registered: {hotkey_ok}); stop path: {}",
                hotkey_injected.unwrap_or(0),
                stop_reason_label(reason)
            ),
        )
    } else {
        unverified("O7", O7, "not run; use --selftest-hotkey")
    });
    finish(&checks, Some(region), fg_before_json, fg_after_json, reason);
}

fn live_checks(hwnd: HWND, region: Region, fg_before: HWND, fg_after: HWND) -> Vec<Check> {
    let mut out = Vec::new();

    // SAFETY: plain queries on our own live window.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let wanted = [
        ("WS_EX_LAYERED", WS_EX_LAYERED.0),
        ("WS_EX_TRANSPARENT", WS_EX_TRANSPARENT.0),
        ("WS_EX_NOACTIVATE", WS_EX_NOACTIVATE.0),
        ("WS_EX_TOPMOST", WS_EX_TOPMOST.0),
        ("WS_EX_TOOLWINDOW", WS_EX_TOOLWINDOW.0),
    ];
    let missing: Vec<&str> = wanted
        .iter()
        .filter(|(_, bit)| style & bit == 0)
        .map(|(n, _)| *n)
        .collect();
    out.push(check(
        "O1",
        "overlay has layered, click-through, non-activating, topmost, toolwindow styles",
        missing.is_empty(),
        format!("ex-style=0x{style:08x}; missing: {missing:?}"),
    ));

    let mut rect = RECT::default();
    // SAFETY: valid out-pointer.
    let rect_ok = unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok();
    // SAFETY: plain query.
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    out.push(check(
        "O2",
        "overlay visible and exactly covers the target region",
        rect_ok
            && visible
            && (
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
            ) == (region.x, region.y, region.w, region.h),
        format!(
            "visible={visible} rect=({},{})..({},{}) target={region:?}",
            rect.left, rect.top, rect.right, rect.bottom
        ),
    ));

    out.push(if fg_before.0.is_null() {
        unverified(
            "O3",
            "foreground window unchanged after showing the overlay",
            "no foreground window before show",
        )
    } else {
        let (before, _) = describe(fg_before);
        let (after, class) = describe(fg_after);
        check(
            "O3",
            "foreground window unchanged after showing the overlay",
            fg_after == fg_before && fg_after != hwnd,
            format!(
                "before={before:?} after={after:?} (class {class}); overlay hwnd=0x{:x}",
                hwnd.0 as isize
            ),
        )
    });

    let edge = (region.x + BORDER_THICKNESS_PX / 2, region.y + region.h / 2);
    let centre = (region.x + region.w / 2, region.y + region.h / 2);
    out.push(match screen_pixel(edge.0, edge.1) {
        Some(p) => check(
            "O4",
            "composited screen pixel on the border is orange",
            is_orange(p),
            format!("pixel at {edge:?} = rgb{p:?}, want rgb{ORANGE_RGB:?} +/-{COLOUR_TOLERANCE}"),
        ),
        None => unverified(
            "O4",
            "composited screen pixel on the border is orange",
            "could not read the screen pixel",
        ),
    });
    out.push(match screen_pixel(centre.0, centre.1) {
        Some(p) => check(
            "O4b",
            "interior is not covered by the overlay (centre pixel is not orange)",
            !is_orange(p),
            format!("pixel at {centre:?} = rgb{p:?}"),
        ),
        None => unverified(
            "O4b",
            "interior is not covered by the overlay (centre pixel is not orange)",
            "could not read the screen pixel",
        ),
    });

    // SAFETY: plain hit-test query.
    let hit = unsafe {
        WindowFromPoint(POINT {
            x: edge.0,
            y: edge.1,
        })
    };
    out.push(check(
        "O5",
        "mouse hit-test at a border point does not land on the overlay (click-through)",
        hit != hwnd,
        format!(
            "WindowFromPoint({edge:?}) = 0x{:x}; overlay = 0x{:x}",
            hit.0 as isize, hwnd.0 as isize
        ),
    ));
    out
}

fn finish(checks: &[Check], region: Option<Region>, fg_before: Value, fg_after: Value, stop: u8) {
    println!("\n== checks ==");
    for c in checks {
        println!("  [{}] {} {}: {}", c.status.label(), c.id, c.name, c.detail);
    }

    const REQUIRED: [&str; 6] = ["O1", "O2", "O3", "O4", "O5", "O6"];
    let status_of = |id: &str| checks.iter().find(|c| c.id == id).map(|c| c.status);
    let blocking = checks
        .iter()
        .find(|c| c.status == Status::Fail)
        .or_else(|| {
            REQUIRED.iter().find_map(|id| {
                checks
                    .iter()
                    .find(|c| c.id == *id && c.status != Status::Pass)
            })
        });
    let line = if let Some(c) = blocking {
        format!(
            "SPIKE STATUS: overlay NOT proven (reason: {} {} is {}: {})",
            c.id,
            c.name,
            c.status.label(),
            c.detail
        )
    } else if REQUIRED.iter().any(|id| status_of(id).is_none()) {
        "SPIKE STATUS: overlay NOT proven (a required check was not recorded)".to_string()
    } else {
        let caveats: Vec<&str> = [
            (status_of("C1") != Some(Status::Pass))
                .then_some("focus check not validated by a control (C1 not PASS)"),
            (status_of("O7") != Some(Status::Pass)).then_some("global-hotkey stop (O7) UNVERIFIED"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if caveats.is_empty() {
            "SPIKE STATUS: overlay proven (styles, region, focus, composited orange pixel, click-through, local close)"
                .to_string()
        } else {
            format!("SPIKE STATUS: overlay proven EXCEPT {}", caveats.join("; "))
        }
    };
    println!("\n{line}");

    let record = json!({
        "spike": 18,
        "region": region.map(|r| json!([r.x, r.y, r.w, r.h])),
        "foreground": { "before": fg_before, "after": fg_after },
        "stop_reason": stop_reason_label(stop),
        "checks": checks.iter().map(|c| json!({
            "id": c.id, "name": c.name, "status": c.status.label(), "detail": c.detail
        })).collect::<Vec<_>>(),
        "status_line": line,
    });
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out");
    let bytes = serde_json::to_vec_pretty(&record).unwrap_or_default();
    if let Err(e) = std::fs::create_dir_all(&out_dir)
        .and_then(|()| std::fs::write(out_dir.join("record.json"), bytes))
    {
        eprintln!("could not write out/record.json: {e}");
    }
}
