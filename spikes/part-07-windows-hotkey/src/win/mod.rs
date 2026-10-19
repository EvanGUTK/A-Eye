mod overlay;

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::core::{BOOL, HRESULT};
use windows::Win32::Foundation::ERROR_HOTKEY_ALREADY_REGISTERED;
use windows::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    WM_HOTKEY, WM_QUIT,
};

use crate::config::{Config, Hotkey, STOP_HOTKEY};
use overlay::Overlay;

const TOGGLE_ID: i32 = 1;
const EXIT_ID: i32 = 2;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn ctrl_handler(kind: u32) -> BOOL {
    if kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT {
        INTERRUPTED.store(true, Ordering::SeqCst);
        return BOOL(1);
    }
    BOOL(0)
}

struct ConsoleHandler;

impl Drop for ConsoleHandler {
    fn drop(&mut self) {
        // SAFETY: removes the same static callback registered by this process.
        if let Err(error) = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), false) } {
            eprintln!("remove console handler: {error}");
        }
    }
}

#[derive(Default)]
struct Hotkeys(Vec<i32>);

impl Hotkeys {
    fn register(&mut self, id: i32, key: &Hotkey) -> Result<(), (i32, String)> {
        let mut modifiers = MOD_NOREPEAT;
        if key.ctrl {
            modifiers |= MOD_CONTROL;
        }
        if key.alt {
            modifiers |= MOD_ALT;
        }
        if key.shift {
            modifiers |= MOD_SHIFT;
        }
        // SAFETY: NULL binds to this thread, independent of any overlay HWND or focus.
        if let Err(error) = unsafe { RegisterHotKey(None, id, modifiers, u32::from(key.key)) } {
            if error.code() == HRESULT::from_win32(ERROR_HOTKEY_ALREADY_REGISTERED.0) {
                let remedy = if id == EXIT_ID {
                    "Stop the process holding the fixed exit chord Ctrl+Alt+Q."
                } else {
                    "Stop that process or choose --hotkey with a different chord."
                };
                return Err((3, format!("hotkey {} unavailable: ERROR_HOTKEY_ALREADY_REGISTERED (1409); another process owns it. {remedy} No overlay was shown.", key.label())));
            }
            return Err((
                1,
                format!(
                    "RegisterHotKey({}): {error}; no overlay was shown",
                    key.label()
                ),
            ));
        }
        self.0.push(id);
        Ok(())
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        for id in self.0.iter().rev() {
            // SAFETY: registrations belong to this same thread.
            if let Err(error) = unsafe { UnregisterHotKey(None, *id) } {
                eprintln!("UnregisterHotKey({id}): {error}");
            }
        }
    }
}

fn emit(value: Value) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{value}")
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("write evidence: {error}"))
}

pub fn run(config: Config) -> Result<(), (i32, String)> {
    let fatal = |error: String| (1, error);
    // SAFETY: set before creating any windows; physical pixels match part-18.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
        .map_err(|error| fatal(format!("DPI awareness: {error}")))?;
    let mut hotkeys = Hotkeys::default();
    hotkeys.register(TOGGLE_ID, &config.hotkey)?;
    hotkeys.register(EXIT_ID, &Hotkey::parse(STOP_HOTKEY).map_err(fatal)?)?;
    // SAFETY: static callback only latches a flag; all window work stays on this thread.
    unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), true) }
        .map_err(|error| fatal(format!("console handler: {error}")))?;
    let console = ConsoleHandler;
    let overlay = Overlay::new().map_err(fatal)?;
    emit(
        json!({"event": "ready", "pid": std::process::id(), "hotkey": config.hotkey.label(),
        "exit_hotkey": STOP_HOTKEY, "overlay": "off", "hwnd": overlay.hwnd.0 as usize,
        "region": [overlay.region.x, overlay.region.y, overlay.region.w, overlay.region.h],
        "hold_secs": config.hold.as_secs(), "capture": false}),
    )
    .map_err(fatal)?;

    let started = Instant::now();
    let mut visible = false;
    let mut toggles = 0u64;
    let reason = 'running: loop {
        if INTERRUPTED.load(Ordering::SeqCst) {
            break "ctrl_c";
        }
        if started.elapsed() >= config.hold {
            break "timeout";
        }
        let mut message = MSG::default();
        // SAFETY: polling this thread's own queue, including HWND-independent WM_HOTKEY.
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_QUIT {
                break 'running "wm_close";
            }
            if message.message == WM_HOTKEY {
                if message.wParam.0 == EXIT_ID as usize {
                    break 'running "exit_hotkey";
                }
                if message.wParam.0 == TOGGLE_ID as usize {
                    // SAFETY: read only; do not request or restore foreground focus.
                    let before = unsafe { GetForegroundWindow() };
                    overlay.set_visible(!visible).map_err(fatal)?;
                    visible = !visible;
                    toggles += 1;
                    let after = unsafe { GetForegroundWindow() };
                    emit(json!({"event": "toggle", "count": toggles,
                        "overlay": if visible { "on" } else { "off" },
                        "foreground_before": before.0 as usize,
                        "foreground_after": after.0 as usize}))
                    .map_err(fatal)?;
                }
                continue;
            }
            // SAFETY: normal message dispatch for this thread's overlay.
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        sleep(POLL_INTERVAL);
    };
    drop(overlay);
    drop(console);
    drop(hotkeys);
    emit(json!({"event": "stopped", "reason": reason, "toggles": toggles, "overlay": "off"}))
        .map_err(fatal)
}
