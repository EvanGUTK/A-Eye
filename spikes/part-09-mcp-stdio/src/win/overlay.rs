//! Orange border overlays: layered, click-through, non-activating, topmost windows.
//!
//! Window styles, painting and the message pump are adapted from spike 18, which proved them on the
//! primary monitor. What is new here, and so unproven until the live test says otherwise:
//!
//! - one UI thread per overlay, so the MCP request thread is never blocked by a message pump;
//! - stopping is an atomic flag the overlay's own thread polls. No other thread ever touches the
//!   HWND, so a recycled window handle can never be closed by mistake;
//! - every overlay has a bounded lifetime, and one process-wide Ctrl+Alt+Q hotkey stops them all.
//!   If that hotkey cannot be registered, no overlay is shown: a visible indicator without a local
//!   stop control is not acceptable.

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, sleep, JoinHandle};
use std::time::{Duration, Instant};

use spike_10_scope_leases::{Authorization, Denial, LeaseBroker};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PeekMessageW,
    PostQuitMessage, PostThreadMessageW, RegisterClassW, SetWindowPos, ShowWindow,
    TranslateMessage, UpdateLayeredWindow, HMENU, HTTRANSPARENT, HWND_TOPMOST, MSG, PM_NOREMOVE,
    PM_REMOVE, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE,
    ULW_ALPHA, WM_CLOSE, WM_DESTROY, WM_HOTKEY, WM_NCHITTEST, WM_QUIT, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::monitors::Monitor;
use crate::backend::{OverlayEnd, OverlayStarted, OverlayStopped, Rgb, ToolError};

const BORDER_THICKNESS_PX: i32 = 6;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long to wait for a freshly spawned UI thread to report that its window exists.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// Started and not yet stopped by the agent; finished ones are reaped when this is reached.
const MAX_TRACKED_OVERLAYS: usize = 16;
const HOTKEY_ID: i32 = 1;
const VK_Q: u32 = 0x51;
const HOTKEY_LABEL: &str = "Ctrl+Alt+Q";
const CLASS_NAME: PCWSTR = w!("AEyeMcpOverlay");

const REASON_NONE: u8 = 0;
const REASON_STOPPED: u8 = 1;
const REASON_EXPIRED: u8 = 2;
const REASON_HOTKEY: u8 = 3;
const REASON_EXTERNAL: u8 = 4;
const REASON_SHUTDOWN: u8 = 5;

fn end_from_reason(reason: u8) -> OverlayEnd {
    match reason {
        REASON_EXPIRED => OverlayEnd::Expired,
        REASON_HOTKEY => OverlayEnd::LocalHotkey,
        REASON_EXTERNAL => OverlayEnd::ClosedExternally,
        REASON_SHUTDOWN => OverlayEnd::ServerShutdown,
        _ => OverlayEnd::Stopped,
    }
}

/// Records why an overlay ended. Only the first reason wins, so "already ended" is detectable.
fn set_reason(reason: &AtomicU8, new: u8) -> bool {
    reason
        .compare_exchange(REASON_NONE, new, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

struct Entry {
    monitor_id: String,
    reason: Arc<AtomicU8>,
    thread: Option<JoinHandle<()>>,
}

impl Entry {
    fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

type Entries = Arc<Mutex<HashMap<String, Entry>>>;

fn lock(entries: &Entries) -> MutexGuard<'_, HashMap<String, Entry>> {
    // A panic elsewhere must not take the stop control down with it.
    entries.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct OverlayManager {
    next_id: u64,
    entries: Entries,
    hotkey: Option<HotkeyThread>,
    broker: Arc<LeaseBroker>,
}

impl OverlayManager {
    pub fn new(broker: Arc<LeaseBroker>) -> Self {
        Self {
            next_id: 1,
            entries: Arc::new(Mutex::new(HashMap::new())),
            hotkey: None,
            broker,
        }
    }

    pub fn start(
        &mut self,
        monitor: &Monitor,
        colour: Rgb,
        lifetime: Duration,
        authorization: Authorization,
    ) -> Result<OverlayStarted, ToolError> {
        self.ensure_hotkey()?;
        {
            let mut entries = lock(&self.entries);
            if entries.len() >= MAX_TRACKED_OVERLAYS {
                entries.retain(|_, e| !e.finished());
            }
            if entries.len() >= MAX_TRACKED_OVERLAYS {
                return Err(ToolError::new(format!(
                    "too many overlays ({MAX_TRACKED_OVERLAYS}); stop some with aeye_stop_overlay"
                )));
            }
            let monitor_id = monitor.id();
            if let Some((handle, _)) = entries
                .iter()
                .find(|(_, e)| e.monitor_id == monitor_id && !e.finished())
            {
                return Err(ToolError::new(format!(
                    "monitor {monitor_id} already has overlay {handle}"
                )));
            }
        }

        let region = Region {
            x: monitor.rect.left,
            y: monitor.rect.top,
            w: monitor.width(),
            h: monitor.height(),
        };
        let reason = Arc::new(AtomicU8::new(REASON_NONE));
        let lease = OverlayLease {
            broker: Arc::clone(&self.broker),
            authorization,
        };
        let thread = spawn_overlay(region, colour, lifetime, Arc::clone(&reason), lease)?;
        let handle = format!("overlay-{}", self.next_id);
        self.next_id += 1;
        lock(&self.entries).insert(
            handle.clone(),
            Entry {
                monitor_id: monitor.id(),
                reason,
                thread: Some(thread),
            },
        );
        Ok(OverlayStarted {
            handle,
            monitor_id: monitor.id(),
            expires_in: lifetime,
        })
    }

    pub fn stop(&mut self, handle: &str) -> Result<OverlayStopped, ToolError> {
        let entry = lock(&self.entries).remove(handle).ok_or_else(|| {
            ToolError::new(format!(
                "unknown overlay handle '{handle}' (never issued, or already stopped)"
            ))
        })?;
        set_reason(&entry.reason, REASON_STOPPED);
        if let Some(thread) = entry.thread {
            let _ = thread.join();
        }
        Ok(OverlayStopped {
            handle: handle.to_string(),
            ended_by: end_from_reason(entry.reason.load(Ordering::SeqCst)),
        })
    }

    /// Removes every overlay and the hotkey. Nothing may outlive the MCP session.
    pub fn shutdown(&mut self) {
        let entries: Vec<Entry> = lock(&self.entries).drain().map(|(_, e)| e).collect();
        for entry in entries {
            set_reason(&entry.reason, REASON_SHUTDOWN);
            if let Some(thread) = entry.thread {
                let _ = thread.join();
            }
        }
        if let Some(hotkey) = self.hotkey.take() {
            hotkey.stop();
        }
    }

    pub fn ensure_hotkey(&mut self) -> Result<(), ToolError> {
        if self
            .hotkey
            .as_ref()
            .is_some_and(|h| h.running.load(Ordering::SeqCst))
        {
            return Ok(());
        }
        self.hotkey = Some(start_hotkey(
            Arc::clone(&self.entries),
            Arc::clone(&self.broker),
        )?);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Region {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

struct OverlayLease {
    broker: Arc<LeaseBroker>,
    authorization: Authorization,
}

fn spawn_overlay(
    region: Region,
    colour: Rgb,
    lifetime: Duration,
    reason: Arc<AtomicU8>,
    lease: OverlayLease,
) -> Result<JoinHandle<()>, ToolError> {
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), ToolError>>();
    let reason_on_timeout = Arc::clone(&reason);
    let thread = thread::Builder::new()
        .name("aeye-overlay".into())
        .spawn(move || overlay_thread(region, colour, lifetime, &reason, &ready_tx, &lease))
        .map_err(|e| ToolError::new(format!("could not start the overlay thread: {e}")))?;
    match ready_rx.recv_timeout(START_TIMEOUT) {
        Ok(Ok(())) => Ok(thread),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            // The caller gets no handle for a late window, so nothing else can reach it: make it
            // destroy itself on its first poll instead of lingering for its whole lifetime.
            set_reason(&reason_on_timeout, REASON_SHUTDOWN);
            Err(ToolError::new("overlay thread did not report in time"))
        }
    }
}

/// Owns the window for its whole life: create, pump, expire or stop, destroy.
fn overlay_thread(
    region: Region,
    colour: Rgb,
    lifetime: Duration,
    reason: &AtomicU8,
    ready: &mpsc::Sender<Result<(), ToolError>>,
    lease: &OverlayLease,
) {
    let deadline = Instant::now() + lifetime;
    if let Err(denial) = lease.broker.verify(&lease.authorization) {
        let _ = ready.send(Err(denial.into()));
        return;
    }
    let hwnd = match create_overlay(region, colour) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            let _ = ready.send(Err(ToolError::new(format!(
                "overlay window not created: {e}"
            ))));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let mut destroyed = false;
    while pump_messages() {
        if !destroyed {
            if let Err(denial) = lease.broker.verify(&lease.authorization) {
                // Global revocation also covers windows created before the manager tracked them.
                let reason_code = if denial == Denial::Expired {
                    REASON_EXPIRED
                } else {
                    REASON_HOTKEY
                };
                set_reason(reason, reason_code);
            }
            if Instant::now() >= deadline {
                set_reason(reason, REASON_EXPIRED);
            }
            if reason.load(Ordering::SeqCst) != REASON_NONE {
                // SAFETY: hwnd was created on this thread, which is the one calling.
                let _ = unsafe { DestroyWindow(hwnd) };
                destroyed = true;
            }
        }
        sleep(POLL_INTERVAL);
    }
    // The pump ended without us asking: something outside closed the window.
    set_reason(reason, REASON_EXTERNAL);
}

/// Returns false once WM_QUIT has been seen.
fn pump_messages() -> bool {
    let mut msg = MSG::default();
    // SAFETY: standard message pump on the thread that owns the window.
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

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // Belt and braces with WS_EX_TRANSPARENT: never claim mouse hits.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_CLOSE => {
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

fn register_class() -> Result<(), String> {
    // SAFETY: null module name returns this executable's handle.
    let instance =
        unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let class = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        lpszClassName: CLASS_NAME,
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

fn create_overlay(r: Region, colour: Rgb) -> Result<HWND, String> {
    static CLASS: OnceLock<Result<(), String>> = OnceLock::new();
    CLASS.get_or_init(register_class).clone()?;

    // SAFETY: null module name returns this executable's handle.
    let instance =
        unsafe { GetModuleHandleW(None) }.map_err(|e| format!("GetModuleHandleW: {e}"))?;
    let ex_style =
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
    // SAFETY: class is registered; all pointers are valid for the call.
    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            CLASS_NAME,
            w!("A-Eye overlay"),
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
    .map_err(|e| format!("CreateWindowExW: {e}"))?;

    if let Err(e) = paint_border(hwnd, r, colour) {
        // SAFETY: hwnd was just created on this thread.
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(e);
    }
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
    Ok(hwnd)
}

/// Paints a per-pixel-alpha bitmap: an opaque frame in `colour`, fully transparent inside.
fn paint_border(hwnd: HWND, r: Region, colour: Rgb) -> Result<(), String> {
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
        let opaque = 0xff00_0000u32
            | (u32::from(colour.0) << 16)
            | (u32::from(colour.1) << 8)
            | u32::from(colour.2);
        for (i, px) in pixels.iter_mut().enumerate() {
            let (x, y) = ((i as i32) % width, (i as i32) / width);
            let on_border = x < BORDER_THICKNESS_PX
                || y < BORDER_THICKNESS_PX
                || x >= width - BORDER_THICKNESS_PX
                || y >= height - BORDER_THICKNESS_PX;
            *px = if on_border { opaque } else { 0 };
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

/// One thread owns the global Ctrl+Alt+Q registration for the whole process.
struct HotkeyThread {
    thread_id: u32,
    join: JoinHandle<()>,
    running: Arc<AtomicBool>,
}

impl HotkeyThread {
    fn stop(self) {
        // SAFETY: posting to a thread id is allowed from any thread; failure means it already ended.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        let _ = self.join.join();
    }
}

fn start_hotkey(entries: Entries, broker: Arc<LeaseBroker>) -> Result<HotkeyThread, ToolError> {
    // A rendezvous prevents a late registration from retaining the chord after a startup timeout:
    // sending readiness fails if the caller timed out and dropped its receiver.
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(0);
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = Arc::clone(&running);
    let join = thread::Builder::new()
        .name("aeye-hotkey".into())
        .spawn(move || hotkey_thread(&entries, &broker, &ready_tx, &thread_running))
        .map_err(|e| ToolError::new(format!("could not start the hotkey thread: {e}")))?;
    match ready_rx.recv_timeout(START_TIMEOUT) {
        Ok(Ok(thread_id)) => Ok(HotkeyThread {
            thread_id,
            join,
            running,
        }),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(ToolError::new(format!(
                "local stop hotkey {HOTKEY_LABEL} is unavailable ({e}); not granting a lease \
                 without a local stop control (is another A-Eye process holding it?)"
            )))
        }
        Err(_) => Err(ToolError::new("hotkey thread did not report in time")),
    }
}

fn hotkey_thread(
    entries: &Entries,
    broker: &LeaseBroker,
    ready: &mpsc::SyncSender<Result<u32, String>>,
    running: &AtomicBool,
) {
    // SAFETY: a null window registers the hotkey for this thread's message queue.
    let registered =
        unsafe { RegisterHotKey(None, HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_Q) };
    if let Err(e) = registered {
        let _ = ready.send(Err(e.to_string()));
        return;
    }
    let mut msg = MSG::default();
    // SAFETY: PM_NOREMOVE only makes sure this thread has a queue, so a WM_QUIT posted to its id
    // right after `ready` cannot be lost.
    unsafe {
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
    }
    // SAFETY: plain query.
    if ready.send(Ok(unsafe { GetCurrentThreadId() })).is_err() {
        // SAFETY: registration belongs to this thread; no lease was granted on timeout.
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
        return;
    }

    // SAFETY: standard blocking message loop; 0 is WM_QUIT and -1 an error, both end it.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message == WM_HOTKEY && msg.wParam.0 as i32 == HOTKEY_ID {
            let epoch = broker.revoke_all();
            eprintln!("[aeye-mcp] {HOTKEY_LABEL} pressed: revoked all leases at epoch {epoch}; removing overlays");
            for entry in lock(entries).values() {
                set_reason(&entry.reason, REASON_HOTKEY);
            }
        }
    }
    // Mark unavailable BEFORE revocation: a request observing the new epoch must not mistake an
    // exiting thread for a working stop control and grant a lease after this final revoke.
    running.store(false, Ordering::SeqCst);
    // A lost message loop must not leave previously issued leases live without a local stop.
    broker.revoke_all();
    for entry in lock(entries).values() {
        set_reason(&entry.reason, REASON_SHUTDOWN);
    }
    // SAFETY: matches the RegisterHotKey above.
    let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
}
