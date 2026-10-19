//! Live test: drives the real binary over stdio on an interactive Windows desktop.
//!
//! Ignored by default because it captures the primary monitor, flashes borders on it and injects
//! the Ctrl+Alt+Q chord. Run it deliberately, one test at a time:
//!
//!   cargo test --test live_stdio -- --ignored --nocapture --test-threads=1
//!
//! The border is detected by capturing the monitor through the server itself, so a pass means the
//! whole path (MCP request -> overlay window -> compositor -> Windows.Graphics.Capture -> PNG over
//! MCP) works, not just that a window was created.
#![cfg(windows)]

use std::io::{BufRead, BufReader, Cursor, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_MENU,
};

const SERVER: &str = env!("CARGO_BIN_EXE_spike-09-mcp-stdio");
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Time for the compositor to show or drop a window before the next capture.
const SETTLE: Duration = Duration::from_millis(500);
const ORANGE: (u8, u8, u8) = (255, 140, 0);
const BLUE: (u8, u8, u8) = (30, 144, 255);
const COLOUR_TOLERANCE: i32 = 6;
const FULL_RES: u32 = 8192;
const BORDER_PROBE_X: u32 = 2;
const VK_Q: u16 = 0x51;
/// Distinct sampled colours a real desktop frame is expected to exceed.
const MIN_DISTINCT_COLOURS: usize = 8;

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Result<Value, String>>,
    next_id: u64,
}

impl Client {
    fn spawn(args: &[&str]) -> Self {
        let mut child = Command::new(SERVER)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn the server");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                // Anything on stdout that is not one JSON message breaks the transport.
                let parsed = line.map_err(|e| e.to_string()).and_then(|l| {
                    serde_json::from_str::<Value>(&l)
                        .map_err(|e| format!("stdout line is not JSON ({e}): {l}"))
                });
                let bad = parsed.is_err();
                if tx.send(parsed).is_err() || bad {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 1,
        }
    }

    fn send(&mut self, msg: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        writeln!(stdin, "{msg}").expect("write to server");
        stdin.flush().expect("flush to server");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let msg = self
                .lines
                .recv_timeout(REPLY_TIMEOUT)
                .expect("a reply in time")
                .expect("clean stdout");
            if msg["id"] == id {
                return msg;
            }
        }
    }

    fn handshake(&mut self) -> Value {
        let reply = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "live-test", "version": "0" }
            }),
        );
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        reply
    }

    fn call(&mut self, name: &str, args: Value) -> Value {
        let reply = self.request("tools/call", json!({ "name": name, "arguments": args }));
        assert!(reply.get("error").is_none(), "protocol error: {reply}");
        reply["result"].clone()
    }

    fn capture(&mut self, monitor_id: &str, max_dimension: u32) -> Shot {
        let result = self.call(
            "aeye_capture_monitor",
            json!({ "monitor_id": monitor_id, "max_dimension": max_dimension }),
        );
        assert_eq!(result["isError"], false, "capture failed: {result}");
        Shot::decode(&result)
    }

    /// Closes stdin like a host would and waits for the process to leave on its own.
    fn close_and_wait(mut self) -> Option<ExitStatus> {
        drop(self.stdin.take());
        let started = Instant::now();
        while started.elapsed() < EXIT_TIMEOUT {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return Some(status);
            }
            sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

struct Shot {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    meta: Value,
}

impl Shot {
    fn decode(result: &Value) -> Self {
        let image = result["content"]
            .as_array()
            .expect("content array")
            .iter()
            .find(|c| c["type"] == "image")
            .expect("an image content item");
        assert_eq!(image["mimeType"], "image/png");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(image["data"].as_str().expect("base64 data"))
            .expect("valid base64");
        let mut reader = png::Decoder::new(Cursor::new(bytes))
            .read_info()
            .expect("a valid PNG");
        let mut buf = vec![0; reader.output_buffer_size().expect("buffer size")];
        let info = reader.next_frame(&mut buf).expect("PNG frame");
        assert_eq!(info.color_type, png::ColorType::Rgb);
        buf.truncate(info.buffer_size());
        Self {
            width: info.width,
            height: info.height,
            rgb: buf,
            meta: result["structuredContent"].clone(),
        }
    }

    fn px(&self, x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * self.width + x) * 3) as usize;
        (self.rgb[i], self.rgb[i + 1], self.rgb[i + 2])
    }

    fn distinct_colours(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        for i in (0..self.rgb.len() / 3).step_by(997) {
            seen.insert((self.rgb[i * 3], self.rgb[i * 3 + 1], self.rgb[i * 3 + 2]));
        }
        seen.len()
    }
}

fn near(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
    let close = |x: u8, y: u8| (i32::from(x) - i32::from(y)).abs() <= COLOUR_TOLERANCE;
    close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2)
}

#[derive(Default)]
struct Report(Vec<(String, bool)>);

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        println!(
            "[{}] {name}: {}",
            if ok { "PASS" } else { "FAIL" },
            detail.as_ref()
        );
        self.0.push((name.to_string(), ok));
    }

    fn finish(self) {
        let failed: Vec<&str> = self
            .0
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(n, _)| n.as_str())
            .collect();
        println!("\n{} checks, {} failed", self.0.len(), failed.len());
        assert!(failed.is_empty(), "failed checks: {failed:?}");
    }
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

/// Injects Ctrl+Alt+Q; returns how many of the six events the system accepted.
fn inject_stop_chord() -> u32 {
    let events = [
        key_event(VK_CONTROL.0, false),
        key_event(VK_MENU.0, false),
        key_event(VK_Q, false),
        key_event(VK_Q, true),
        key_event(VK_MENU.0, true),
        key_event(VK_CONTROL.0, true),
    ];
    // SAFETY: events is a valid slice of INPUT.
    unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) }
}

/// Listening TCP sockets plus bound UDP endpoints owned by `pid`, via the Windows networking
/// cmdlets. Returns None if PowerShell could not be queried.
fn network_endpoints(pid: u32) -> Option<u32> {
    let script = format!(
        "(@(Get-NetTCPConnection -OwningProcess {pid} -ErrorAction SilentlyContinue).Count + \
         @(Get-NetUDPEndpoint -OwningProcess {pid} -ErrorAction SilentlyContinue).Count)"
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

struct Primary {
    id: String,
    width: u32,
    height: u32,
}

fn primary_monitor(client: &mut Client) -> (Primary, usize) {
    let result = client.call("aeye_list_monitors", json!({}));
    let monitors = result["structuredContent"]["monitors"]
        .as_array()
        .expect("monitors array");
    let primary = monitors
        .iter()
        .find(|m| m["primary"] == true)
        .expect("a primary monitor");
    (
        Primary {
            id: primary["id"].as_str().expect("id").to_string(),
            width: primary["width"].as_u64().expect("width") as u32,
            height: primary["height"].as_u64().expect("height") as u32,
        },
        monitors.len(),
    )
}

fn border_probe(shot: &Shot) -> (u8, u8, u8) {
    shot.px(BORDER_PROBE_X, shot.height / 2)
}

#[test]
#[ignore = "needs an interactive Windows desktop; see the module docs"]
fn live_session() {
    let mut r = Report::default();
    let mut client = Client::spawn(&["--allow-capture"]);
    let pid = client.child.id();

    let init = client.handshake();
    r.check(
        "initialize",
        init["result"]["protocolVersion"] == "2025-11-25"
            && init["result"]["capabilities"]["tools"].is_object(),
        init["result"]["serverInfo"].to_string(),
    );

    let listed = client.request("tools/list", json!({}));
    let mut names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("name"))
        .collect();
    names.sort_unstable();
    r.check(
        "tools/list",
        names
            == [
                "aeye_capture_monitor",
                "aeye_get_cursor_position",
                "aeye_list_monitors",
                "aeye_start_overlay",
                "aeye_stop_overlay",
            ],
        format!("{names:?}"),
    );

    let (primary, monitor_count) = primary_monitor(&mut client);
    r.check(
        "list_monitors",
        primary.width > 0 && primary.height > 0,
        format!(
            "{monitor_count} monitor(s); primary {} is {}x{}",
            primary.id, primary.width, primary.height
        ),
    );

    let cursor = client.call("aeye_get_cursor_position", json!({}));
    let c = &cursor["structuredContent"];
    r.check(
        "get_cursor_position",
        c["x"].is_i64() && c["y"].is_i64(),
        c.to_string(),
    );

    let endpoints = network_endpoints(pid);
    r.check(
        "no network listener",
        endpoints == Some(0),
        format!("TCP+UDP endpoints owned by pid {pid}: {endpoints:?} (one snapshot, mid-session)"),
    );

    // Baseline capture at full resolution.
    let before = client.capture(&primary.id, FULL_RES);
    r.check(
        "capture full size",
        (before.width, before.height) == (primary.width, primary.height)
            && before.meta["original_size"] == json!([primary.width, primary.height])
            && before.meta["scale"] == 1.0,
        format!(
            "{}x{} (monitor {}x{}), {} png bytes",
            before.width, before.height, primary.width, primary.height, before.meta["png_bytes"]
        ),
    );
    r.check(
        "capture is not blank",
        before.distinct_colours() >= MIN_DISTINCT_COLOURS,
        format!("{} distinct sampled colours", before.distinct_colours()),
    );
    let precondition = !near(border_probe(&before), ORANGE) && !near(border_probe(&before), BLUE);
    r.check(
        "precondition: no border colour at the probe pixel yet",
        precondition,
        format!("{:?}", border_probe(&before)),
    );

    let small = client.capture(&primary.id, 800);
    let longest = small.width.max(small.height);
    r.check(
        "capture downscale",
        longest == 800 && small.meta["original_size"] == json!([primary.width, primary.height]),
        format!(
            "{}x{}, scale {}",
            small.width, small.height, small.meta["scale"]
        ),
    );

    // Overlay appears in a real capture of the monitor.
    let started = client.call(
        "aeye_start_overlay",
        json!({ "monitor_id": primary.id, "duration_secs": 30 }),
    );
    let handle = started["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    r.check(
        "start_overlay",
        started["isError"] == false && !handle.is_empty(),
        started["structuredContent"].to_string(),
    );
    sleep(SETTLE);
    let with = client.capture(&primary.id, FULL_RES);
    r.check(
        "overlay border is orange in a capture",
        near(border_probe(&with), ORANGE) && near(with.px(0, 0), ORANGE),
        format!(
            "probe {:?}, corner {:?}, want {ORANGE:?} +/-{COLOUR_TOLERANCE}",
            border_probe(&with),
            with.px(0, 0)
        ),
    );
    let centre = with.px(with.width / 2, with.height / 2);
    r.check(
        "overlay interior is untouched",
        centre == before.px(before.width / 2, before.height / 2) || !near(centre, ORANGE),
        format!("centre {centre:?}"),
    );

    let second = client.call("aeye_start_overlay", json!({ "monitor_id": primary.id }));
    r.check(
        "one overlay per monitor",
        second["isError"] == true,
        second["content"][0]["text"].to_string(),
    );

    let stopped = client.call("aeye_stop_overlay", json!({ "handle": handle }));
    r.check(
        "stop_overlay",
        stopped["structuredContent"]["ended_by"] == "stopped",
        stopped["structuredContent"].to_string(),
    );
    sleep(SETTLE);
    let after = client.capture(&primary.id, FULL_RES);
    r.check(
        "border gone after stop",
        !near(border_probe(&after), ORANGE),
        format!("probe {:?}", border_probe(&after)),
    );
    let again = client.call("aeye_stop_overlay", json!({ "handle": handle }));
    r.check(
        "stopping twice is an error",
        again["isError"] == true,
        again["content"][0]["text"].to_string(),
    );

    // Colour parameter.
    let blue = client.call(
        "aeye_start_overlay",
        json!({ "monitor_id": primary.id, "color": "#1E90FF", "duration_secs": 30 }),
    );
    let blue_handle = blue["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    sleep(SETTLE);
    let blue_shot = client.capture(&primary.id, FULL_RES);
    r.check(
        "custom colour",
        near(border_probe(&blue_shot), BLUE),
        format!("probe {:?}, want {BLUE:?}", border_probe(&blue_shot)),
    );
    client.call("aeye_stop_overlay", json!({ "handle": blue_handle }));

    // Every monitor, including non-primary and portrait ones: spike 18 only tried the primary.
    let all = client.call("aeye_list_monitors", json!({}));
    for m in all["structuredContent"]["monitors"]
        .as_array()
        .expect("monitors array")
    {
        let id = m["id"].as_str().expect("id").to_string();
        let (w, h) = (
            m["width"].as_u64().expect("width") as u32,
            m["height"].as_u64().expect("height") as u32,
        );
        let started = client.call(
            "aeye_start_overlay",
            json!({ "monitor_id": id, "duration_secs": 30 }),
        );
        sleep(SETTLE);
        let shot = client.capture(&id, FULL_RES);
        let probes = (border_probe(&shot), shot.px(w - 1, h - 1));
        r.check(
            &format!(
                "overlay on monitor {id} ({w}x{h} at {}%)",
                m["scale_percent"]
            ),
            started["isError"] == false
                && (shot.width, shot.height) == (w, h)
                && near(probes.0, ORANGE)
                && near(probes.1, ORANGE),
            format!(
                "left probe {:?}, far corner {:?}, captured {}x{}",
                probes.0, probes.1, shot.width, shot.height
            ),
        );
        let handle = started["structuredContent"]["handle"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        client.call("aeye_stop_overlay", json!({ "handle": handle }));
    }

    // Bounded lifetime.
    let short = client.call(
        "aeye_start_overlay",
        json!({ "monitor_id": primary.id, "duration_secs": 1 }),
    );
    let short_handle = short["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    sleep(Duration::from_millis(1700));
    let expired = client.call("aeye_stop_overlay", json!({ "handle": short_handle }));
    r.check(
        "overlay expires by itself",
        expired["structuredContent"]["ended_by"] == "expired",
        expired["structuredContent"].to_string(),
    );

    // Local stop control. Only injected after start_overlay succeeded, which requires this
    // process to hold the hotkey, so the chord cannot reach another application.
    let keyed = client.call(
        "aeye_start_overlay",
        json!({ "monitor_id": primary.id, "duration_secs": 30 }),
    );
    let keyed_handle = keyed["structuredContent"]["handle"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let injected = if keyed["isError"] == false {
        sleep(SETTLE);
        inject_stop_chord()
    } else {
        0
    };
    sleep(SETTLE);
    let hotkey_shot = client.capture(&primary.id, FULL_RES);
    let by_key = client.call("aeye_stop_overlay", json!({ "handle": keyed_handle }));
    r.check(
        "Ctrl+Alt+Q removes the overlay",
        by_key["structuredContent"]["ended_by"] == "local_hotkey"
            && !near(border_probe(&hotkey_shot), ORANGE),
        format!(
            "injected {injected}/6 events; {}; probe {:?}",
            by_key["structuredContent"],
            border_probe(&hotkey_shot)
        ),
    );

    // Session end removes everything and releases the hotkey.
    let last = client.call(
        "aeye_start_overlay",
        json!({ "monitor_id": primary.id, "duration_secs": 300 }),
    );
    r.check(
        "overlay running at shutdown",
        last["isError"] == false,
        last["structuredContent"].to_string(),
    );
    // A second server cannot take the hotkey, so it must refuse to show an overlay at all.
    let mut rival = Client::spawn(&["--allow-capture"]);
    rival.handshake();
    let refused = rival.call("aeye_start_overlay", json!({ "monitor_id": primary.id }));
    let refusal = refused["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    r.check(
        "second server refuses an overlay while the hotkey is held",
        refused["isError"] == true && refusal.contains("hotkey"),
        refusal,
    );
    drop(rival);

    let status = client.close_and_wait();
    r.check(
        "exits when stdin closes",
        status.is_some_and(|s| s.success()),
        format!("{status:?}"),
    );
    sleep(SETTLE);

    let mut fresh = Client::spawn(&["--allow-capture"]);
    fresh.handshake();
    let (fresh_primary, _) = primary_monitor(&mut fresh);
    let gone = fresh.capture(&fresh_primary.id, FULL_RES);
    r.check(
        "overlay removed with the process",
        !near(border_probe(&gone), ORANGE),
        format!("probe {:?}", border_probe(&gone)),
    );
    let reuse = fresh.call(
        "aeye_start_overlay",
        json!({ "monitor_id": fresh_primary.id, "duration_secs": 2 }),
    );
    r.check(
        "hotkey was released by the first process",
        reuse["isError"] == false,
        reuse["content"][0]["text"].to_string(),
    );
    fresh.close_and_wait();

    r.finish();
}

#[test]
#[ignore = "needs an interactive Windows desktop; see the module docs"]
fn capture_is_refused_without_allow_capture() {
    let mut r = Report::default();
    let mut client = Client::spawn(&[]);
    client.handshake();
    let (primary, _) = primary_monitor(&mut client);
    let refused = client.call("aeye_capture_monitor", json!({ "monitor_id": primary.id }));
    let text = refused["content"][0]["text"].as_str().unwrap_or_default();
    r.check(
        "capture refused without --allow-capture",
        refused["isError"] == true
            && text.contains("--allow-capture")
            && refused["content"].as_array().is_some_and(|c| c.len() == 1),
        text,
    );
    r.finish();
}
