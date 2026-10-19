//! Deliberate native desktop test. Launches a synthetic PowerShell/WinForms window, injects
//! keyboard input only after this process's child owns the chord, probes three pixels,
//! and checks foreground identity throughout each transition. No screenshot is saved.
#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::mem::size_of;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetPixel,
    ReleaseDC, SelectObject, CAPTUREBLT, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    PostMessageW, SetForegroundWindow, WM_CLOSE,
};

const BINARY: &str = env!("CARGO_BIN_EXE_spike-07-windows-hotkey");
const TIMEOUT: Duration = Duration::from_secs(10);
const SETTLE: Duration = Duration::from_millis(350);
const POLL: Duration = Duration::from_millis(10);
const ORANGE: u32 = 0x0000_8cff; // COLORREF: 0x00BBGGRR (255, 140, 0).
const KEY_A: u16 = b'A' as u16;
const KEY_Q: u16 = b'Q' as u16;

struct Instance {
    child: Child,
    events: Receiver<Value>,
}

impl Instance {
    fn start(chord: &str, hold: &str) -> Self {
        let mut child = Command::new(BINARY)
            .args(["--hotkey", chord, "--hold-secs", hold])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn spike binary");
        let stdout = child.stdout.take().unwrap();
        let (sender, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value =
                    serde_json::from_str(&line.expect("read stdout")).expect("JSON evidence line");
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        Self { child, events }
    }

    fn event(&self, expected: &str) -> Value {
        let value = self.events.recv_timeout(TIMEOUT).expect("event in time");
        assert_eq!(value["event"], expected, "{value}");
        println!("{value}");
        value
    }

    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("query child") {
                return status;
            }
            assert!(Instant::now() < deadline, "spike failed to exit");
            sleep(POLL);
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.child
                .kill()
                .expect("terminate test-owned spike after test failure");
        }
        self.child.wait().expect("reap test-owned spike");
    }
}

struct Fixture {
    child: Child,
    hwnd: HWND,
}

impl Fixture {
    fn start() -> Self {
        // A separate GUI process avoids reusing the user's documents or a WSL pseudoconsole.
        let title = format!("AEyeSpike07ForegroundFixture-{}", std::process::id());
        let command = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
            $form = New-Object System.Windows.Forms.Form; \
            $form.Text = '{title}'; $form.Width = 540; $form.Height = 180; \
            $label = New-Object System.Windows.Forms.Label; $label.AutoSize = $true; \
            $label.Text = 'Synthetic foreground application for A-Eye hotkey test'; \
            $label.Left = 20; $label.Top = 30; $form.Controls.Add($label); \
            [System.Windows.Forms.Application]::Run($form)"
        );
        let child = Command::new("powershell.exe")
            .args(["-NoProfile", "-STA", "-Command", &command])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("launch separate PowerShell GUI fixture");
        let mut fixture = Self {
            child,
            hwnd: HWND::default(),
        };
        let wide: Vec<_> = title.encode_utf16().chain(Some(0)).collect();
        let started = Instant::now();
        while fixture.hwnd.0.is_null() {
            // SAFETY: null-terminated title remains valid throughout the call.
            fixture.hwnd = unsafe { FindWindowW(None, PCWSTR(wide.as_ptr())) }.unwrap_or_default();
            assert!(
                started.elapsed() < TIMEOUT,
                "synthetic PowerShell window not found"
            );
            sleep(POLL);
        }
        let mut pid = 0;
        // SAFETY: query our fixture window and request foreground once, before any hotkey.
        unsafe {
            GetWindowThreadProcessId(fixture.hwnd, Some(&mut pid));
            assert_eq!(pid, fixture.child.id());
            assert!(
                SetForegroundWindow(fixture.hwnd).as_bool(),
                "Windows refused fixture activation"
            );
        }
        assert_eq!(unsafe { GetForegroundWindow() }, fixture.hwnd);
        println!(
            "PASS: separate PowerShell/WinForms foreground hwnd={} pid={pid}",
            fixture.hwnd.0 as usize
        );
        fixture
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.hwnd.0.is_null() {
            // SAFETY: this unique window belongs to the fixture launched by this test.
            if let Err(error) =
                unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            {
                eprintln!("close test fixture: {error}");
            }
        }
        let started = Instant::now();
        while self.child.try_wait().expect("query fixture").is_none() {
            if started.elapsed() > TIMEOUT {
                self.child.kill().expect("terminate test-owned fixture");
                break;
            }
            sleep(POLL);
        }
        self.child.wait().expect("reap fixture");
    }
}

fn key_event(key: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(key),
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

fn inject(key: u16) {
    // Never interfere with a key the human is physically holding.
    for vk in [
        VK_CONTROL.0,
        VK_MENU.0,
        VK_SHIFT.0,
        VK_LWIN.0,
        VK_RWIN.0,
        key,
    ] {
        assert!(
            unsafe { GetAsyncKeyState(i32::from(vk)) } >= 0,
            "release keyboard before this test"
        );
    }
    let events = [
        key_event(VK_CONTROL.0, false),
        key_event(VK_MENU.0, false),
        key_event(key, false),
        key_event(key, true),
        key_event(VK_MENU.0, true),
        key_event(VK_CONTROL.0, true),
    ];
    // SAFETY: all six initialized events form a balanced Ctrl+Alt+key press/release.
    assert_eq!(
        unsafe { SendInput(&events, size_of::<INPUT>() as i32) },
        events.len() as u32
    );
}

fn pixel(x: i32, y: i32) -> u32 {
    // SAFETY: exactly part-18's composited one-pixel probe; no image or title is retained.
    unsafe {
        let screen = GetDC(None);
        assert!(!screen.0.is_null());
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, 1, 1);
        let old = SelectObject(memory, HGDIOBJ(bitmap.0));
        let copied = BitBlt(memory, 0, 0, 1, 1, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        let value = GetPixel(memory, 0, 0).0;
        SelectObject(memory, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        copied.expect("composited pixel read");
        assert_ne!(value, u32::MAX);
        value
    }
}

fn settle(foreground: HWND) {
    let started = Instant::now();
    while started.elapsed() < SETTLE {
        assert_eq!(
            unsafe { GetForegroundWindow() },
            foreground,
            "foreground changed during transition"
        );
        sleep(POLL);
    }
}

fn toggle(
    instance: &Instance,
    key: u16,
    foreground: HWND,
    hwnd: HWND,
    points: &[(i32, i32)],
    on: bool,
    count: u64,
) {
    assert_eq!(unsafe { GetForegroundWindow() }, foreground);
    inject(key);
    let event = instance.event("toggle");
    assert_eq!(event["count"], count);
    assert_eq!(event["overlay"], if on { "on" } else { "off" });
    assert_eq!(event["foreground_before"], foreground.0 as usize);
    assert_eq!(event["foreground_after"], foreground.0 as usize);
    settle(foreground);
    assert_eq!(unsafe { IsWindowVisible(hwnd) }.as_bool(), on);
    for &(x, y) in points {
        let colour = pixel(x, y);
        assert_eq!(
            colour == ORANGE,
            on,
            "border probe ({x},{y}) = 0x{colour:06x}"
        );
    }
    println!(
        "PASS: toggle {count}, border {}, 3 composited pixels and foreground unchanged",
        if on { "ON" } else { "OFF" }
    );
}

fn reject_instance(chord: &str, expected_chord: &str) {
    let mut second = Command::new(BINARY)
        .args(["--hotkey", chord, "--hold-secs", "1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while second.try_wait().unwrap().is_none() {
        if started.elapsed() >= TIMEOUT {
            second.kill().expect("terminate hung competing instance");
            second.wait().unwrap();
            panic!("competing instance did not fail promptly");
        }
        sleep(POLL);
    }
    let output = second.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty(), "competing instance reached ready");
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("ERROR_HOTKEY_ALREADY_REGISTERED (1409)"),
        "{error}"
    );
    assert!(
        error.contains(expected_chord),
        "wrong hotkey error: {error}"
    );
    println!("PASS: competing instance exited 3: {}", error.trim());
}

#[test]
#[ignore = "requires an unlocked Windows desktop; launches a separate GUI and injects hotkeys"]
fn global_toggle_conflict_and_release() {
    // SAFETY: set before any DPI-dependent probe in this standalone test process.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.unwrap();
    let fixture = Fixture::start();
    let mut first = Instance::start("Ctrl+Alt+A", "60");
    let ready = first.event("ready");
    let hwnd = HWND(ready["hwnd"].as_u64().unwrap() as *mut _);
    let region: Vec<_> = ready["region"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap() as i32)
        .collect();
    let [x, y, w, h] = region.as_slice() else {
        panic!("region shape");
    };
    let points = [
        (*x + 2, *y + h / 2),
        (*x + w / 2, *y + 2),
        (*x + w - 3, *y + h / 2),
    ];
    assert_ne!(
        ready["pid"].as_u64().unwrap(),
        u64::from(std::process::id())
    );
    assert_eq!(ready["overlay"], "off");
    assert!(!unsafe { IsWindowVisible(hwnd) }.as_bool());
    settle(fixture.hwnd);
    for &(px, py) in &points {
        assert_ne!(pixel(px, py), ORANGE, "baseline already orange");
    }
    toggle(&first, KEY_A, fixture.hwnd, hwnd, &points, true, 1);

    // A genuinely different process must receive Win32 error 1409, not just a failed boolean.
    reject_instance("Ctrl+Alt+A", "hotkey Ctrl+Alt+A unavailable");
    // Also test failure after acquiring the toggle chord but before acquiring the exit chord.
    reject_instance("Ctrl+Alt+7", "hotkey Ctrl+Alt+Q unavailable");
    // SAFETY: a failed startup must have released its partially acquired toggle registration.
    unsafe {
        RegisterHotKey(
            None,
            98,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            u32::from(b'7'),
        )
        .unwrap();
        UnregisterHotKey(None, 98).unwrap();
    }
    println!("PASS: fixed exit-chord conflict rolls back the acquired custom toggle chord");
    settle(fixture.hwnd);
    toggle(&first, KEY_A, fixture.hwnd, hwnd, &points, false, 2);
    toggle(&first, KEY_A, fixture.hwnd, hwnd, &points, true, 3);
    inject(KEY_Q);
    let stopped = first.event("stopped");
    assert_eq!(stopped["reason"], "exit_hotkey");
    assert!(first.wait().success());
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
    settle(fixture.hwnd);
    for &(px, py) in &points {
        assert_ne!(pixel(px, py), ORANGE);
    }
    println!("PASS: exit hotkey removes active overlay and preserves fixture focus");

    // A different chord proves configuration reaches RegisterHotKey, not only the parser.
    let mut custom = Instance::start("Ctrl+Alt+7", "60");
    let ready = custom.event("ready");
    assert_eq!(ready["hotkey"], "Ctrl+Alt+7");
    let custom_hwnd = HWND(ready["hwnd"].as_u64().unwrap() as *mut _);
    toggle(
        &custom,
        b'7' as u16,
        fixture.hwnd,
        custom_hwnd,
        &points,
        true,
        1,
    );
    toggle(
        &custom,
        b'7' as u16,
        fixture.hwnd,
        custom_hwnd,
        &points,
        false,
        2,
    );
    inject(KEY_Q);
    custom.event("stopped");
    assert!(custom.wait().success());

    // Re-register the original chord, then prove timeout also cleans up while the border is ON.
    let mut timed = Instance::start("Ctrl+Alt+A", "3");
    let ready = timed.event("ready");
    let timed_hwnd = HWND(ready["hwnd"].as_u64().unwrap() as *mut _);
    toggle(&timed, KEY_A, fixture.hwnd, timed_hwnd, &points, true, 1);
    assert_eq!(timed.event("stopped")["reason"], "timeout");
    assert!(timed.wait().success());
    assert!(!unsafe { IsWindow(Some(timed_hwnd)) }.as_bool());
    settle(fixture.hwnd);
    for &(px, py) in &points {
        assert_ne!(pixel(px, py), ORANGE);
    }
    // SAFETY: direct probes in the test's thread after every child has exited.
    unsafe {
        RegisterHotKey(
            None,
            99,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            u32::from(KEY_A),
        )
        .unwrap();
        UnregisterHotKey(None, 99).unwrap();
        RegisterHotKey(
            None,
            100,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            u32::from(KEY_Q),
        )
        .unwrap();
        UnregisterHotKey(None, 100).unwrap();
    }
    println!(
        "PASS: timeout removes active overlay; toggle and exit chords can be registered again"
    );
}
