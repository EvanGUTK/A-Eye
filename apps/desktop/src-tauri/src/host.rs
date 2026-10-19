use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::mcp::Client;

const OVERLAY_SECONDS: u64 = 60;
const PREVIEW_MAX_DIMENSION: u32 = 1568;
const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Connect {},
    Disconnect {},
    Status {},
    RefreshMonitors {},
    ShowOverlay { monitor_id: String },
    HideOverlay {},
    Capture { monitor_id: String },
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Monitor {
    pub id: String,
    pub device: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_percent: u32,
    pub primary: bool,
}

#[derive(Serialize)]
pub struct OverlayStatus {
    monitor_id: String,
    remaining_secs: u64,
}

struct Overlay {
    handle: String,
    monitor_id: String,
    deadline: Instant,
}

#[derive(Serialize)]
pub struct Status {
    connected: bool,
    error: Option<String>,
    monitors: Vec<Monitor>,
    overlay: Option<OverlayStatus>,
}

#[derive(Serialize)]
pub struct Preview {
    monitor_id: String,
    data_url: String,
    width: u32,
    height: u32,
    original_width: u32,
    original_height: u32,
    captured_at_ms: u64,
}

#[derive(Serialize)]
pub struct Outcome {
    status: Status,
    preview: Option<Preview>,
}

pub struct Host {
    sidecar: PathBuf,
    client: Option<Client>,
    monitors: Vec<Monitor>,
    overlay: Option<Overlay>,
    error: Option<String>,
}

impl Host {
    pub fn new(sidecar: PathBuf) -> Self {
        Self {
            sidecar,
            client: None,
            monitors: vec![],
            overlay: None,
            error: None,
        }
    }

    pub fn shutdown(&mut self) {
        // Drop closes stdin, waits for normal native cleanup, then kills a wedged child.
        self.client.take();
        self.monitors.clear();
        self.overlay = None;
    }

    pub fn perform(&mut self, action: Action) -> Outcome {
        let polling = matches!(action, Action::Status {});
        if !polling {
            self.error = None;
        }
        let result = self.execute(action);
        let preview = match result {
            Ok(preview) => preview,
            Err(error) => {
                self.error = Some(error);
                if self.client.as_ref().is_some_and(|client| !client.healthy) {
                    self.shutdown();
                }
                None
            }
        };
        Outcome {
            status: Status {
                connected: self.client.is_some(),
                error: self.error.clone(),
                monitors: self.monitors.clone(),
                overlay: self.overlay.as_ref().map(|overlay| OverlayStatus {
                    monitor_id: overlay.monitor_id.clone(),
                    remaining_secs: overlay
                        .deadline
                        .saturating_duration_since(Instant::now())
                        .as_secs(),
                }),
            },
            preview,
        }
    }

    fn execute(&mut self, action: Action) -> Result<Option<Preview>, String> {
        match action {
            Action::Connect {} => {
                self.shutdown();
                let mut client = Client::spawn(&self.sidecar)?;
                client.initialize()?;
                self.client = Some(client);
                if let Err(error) = self.refresh() {
                    self.shutdown();
                    return Err(error);
                }
            }
            Action::Disconnect {} => self.shutdown(),
            Action::Status {} => {
                if self.client.is_some() {
                    self.refresh()?;
                }
                if self
                    .overlay
                    .as_ref()
                    .is_some_and(|o| Instant::now() >= o.deadline)
                {
                    self.hide_overlay()?;
                }
            }
            Action::RefreshMonitors {} => {
                self.hide_overlay()?;
                self.refresh()?;
            }
            Action::ShowOverlay { monitor_id } => {
                self.validate_current_monitor(&monitor_id)?;
                if self.overlay.is_some() {
                    return Err("Remove the current overlay before showing another".into());
                }
                let result = self.client()?.call("aeye_start_overlay", json!({
                    "monitor_id": monitor_id, "color": "#FF8A3D", "duration_secs": OVERLAY_SECONDS
                }))?;
                match parse_overlay(&result, &monitor_id) {
                    Ok(overlay) => self.overlay = Some(overlay),
                    Err(error) => {
                        // An untrackable border must not survive until an unknown expiry.
                        self.shutdown();
                        return Err(error);
                    }
                }
            }
            Action::HideOverlay {} => self.hide_overlay()?,
            Action::Capture { monitor_id } => {
                let selected = self.validate_current_monitor(&monitor_id)?;
                if self
                    .overlay
                    .as_ref()
                    .is_some_and(|o| o.monitor_id != monitor_id)
                {
                    return Err(
                        "Remove the other monitor's overlay before capturing this display".into(),
                    );
                }
                let result = self.client()?.call(
                    "aeye_capture_monitor",
                    json!({
                        "monitor_id": monitor_id, "max_dimension": PREVIEW_MAX_DIMENSION
                    }),
                )?;
                let preview = parse_preview(result, &monitor_id)?;
                if preview.original_width != selected.width
                    || preview.original_height != selected.height
                {
                    return Err("Display dimensions changed during capture; refresh and choose the display again".into());
                }
                return Ok(Some(preview));
            }
        }
        Ok(None)
    }

    fn client(&mut self) -> Result<&mut Client, String> {
        self.client
            .as_mut()
            .ok_or("MCP sidecar is disconnected; reconnect first".into())
    }

    fn validate_current_monitor(&mut self, id: &str) -> Result<Monitor, String> {
        let selected = self
            .monitors
            .iter()
            .find(|monitor| monitor.id == id)
            .cloned()
            .ok_or("Choose a current monitor; refresh monitors if the display changed")?;
        self.refresh()?;
        if !self.monitors.contains(&selected) {
            self.hide_overlay()?;
            return Err(
                "The selected display changed; choose a display again before capture".into(),
            );
        }
        Ok(selected)
    }

    fn refresh(&mut self) -> Result<(), String> {
        let result = self.client()?.call("aeye_list_monitors", json!({}))?;
        let monitors: Vec<Monitor> =
            serde_json::from_value(result["structuredContent"]["monitors"].clone())
                .map_err(|_| "The sidecar returned an invalid monitor list")?;
        if monitors.len() > 128
            || monitors.iter().any(|m| {
                m.id.is_empty()
                    || m.id.len() > 128
                    || m.device.len() > 512
                    || m.width == 0
                    || m.height == 0
            })
        {
            return Err("The sidecar returned invalid monitor identities or dimensions".into());
        }
        if let Some(overlay) = self.overlay.as_ref() {
            let old = self.monitors.iter().find(|m| m.id == overlay.monitor_id);
            let current = monitors.iter().find(|m| m.id == overlay.monitor_id);
            if old != current {
                self.hide_overlay()?;
            }
        }
        self.monitors = monitors;
        Ok(())
    }

    fn hide_overlay(&mut self) -> Result<(), String> {
        if let Some(overlay) = self.overlay.as_ref() {
            let handle = overlay.handle.clone();
            self.client()?
                .call("aeye_stop_overlay", json!({ "handle": handle }))?;
            self.overlay = None;
        }
        Ok(())
    }
}

fn parse_overlay(result: &Value, monitor_id: &str) -> Result<Overlay, String> {
    let meta = &result["structuredContent"];
    let handle = meta["handle"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or("Overlay result has an invalid handle; sidecar stopped")?;
    let seconds = meta["expires_in_secs"]
        .as_u64()
        .filter(|s| *s > 0 && *s <= OVERLAY_SECONDS)
        .ok_or("Overlay result has an invalid expiry; sidecar stopped")?;
    if meta["monitor_id"] != monitor_id {
        return Err("Overlay result did not match the selected monitor; sidecar stopped".into());
    }
    Ok(Overlay {
        handle: handle.into(),
        monitor_id: monitor_id.into(),
        deadline: Instant::now() + Duration::from_secs(seconds),
    })
}

#[derive(Deserialize)]
struct ImageMeta {
    monitor_id: String,
    image_size: [u32; 2],
    original_size: [u32; 2],
    png_bytes: usize,
    mime_type: String,
}

fn parse_preview(result: Value, monitor_id: &str) -> Result<Preview, String> {
    let meta: ImageMeta = serde_json::from_value(result["structuredContent"].clone())
        .map_err(|_| "Capture result has invalid image metadata")?;
    let items = result["content"]
        .as_array()
        .ok_or("Capture result has no content")?;
    let image = items
        .iter()
        .find(|item| item["type"] == "image")
        .ok_or("Capture result has no image")?;
    let encoded = image["data"]
        .as_str()
        .ok_or("Capture result has no image data")?;
    if meta.monitor_id != monitor_id
        || meta.mime_type != "image/png"
        || image["mimeType"] != "image/png"
        || meta.png_bytes > MAX_PNG_BYTES
        || encoded.len() > MAX_PNG_BYTES.div_ceil(3) * 4
        || meta.image_size.contains(&0)
        || meta.original_size.contains(&0)
        || meta
            .image_size
            .iter()
            .any(|dimension| *dimension > PREVIEW_MAX_DIMENSION)
        || meta.image_size[0] > meta.original_size[0]
        || meta.image_size[1] > meta.original_size[1]
    {
        return Err("Capture result did not match the selected display or image bounds".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Capture result has invalid base64")?;
    if bytes.len() != meta.png_bytes
        || bytes.len() < 33
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || bytes[8..12] != 13_u32.to_be_bytes()
        || &bytes[12..16] != b"IHDR"
        || bytes[16..20] != meta.image_size[0].to_be_bytes()
        || bytes[20..24] != meta.image_size[1].to_be_bytes()
    {
        return Err("Capture PNG header did not match its metadata".into());
    }
    let captured_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "System clock is before the Unix epoch")?
        .as_millis()
        .try_into()
        .map_err(|_| "Capture timestamp overflow")?;
    Ok(Preview {
        monitor_id: monitor_id.into(),
        data_url: format!("data:image/png;base64,{encoded}"),
        width: meta.image_size[0],
        height: meta.image_size[1],
        original_width: meta.original_size[0],
        original_height: meta.original_size[1],
        captured_at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        // Synthetic 1x1 PNG. No private screen data is used in ordinary tests.
        let data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aX1sAAAAASUVORK5CYII=";
        let len = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap()
            .len();
        json!({"structuredContent":{"monitor_id":"test","image_size":[1,1],"original_size":[1,1],"png_bytes":len,"mime_type":"image/png"},
            "content":[{"type":"image","mimeType":"image/png","data":data}]})
    }

    #[test]
    fn preview_requires_matching_monitor_mime_size_and_png_header() {
        assert_eq!(parse_preview(fixture(), "test").unwrap().width, 1);
        assert!(parse_preview(fixture(), "other").is_err());
        for (path, value) in [
            ("mime_type", json!("text/html")),
            ("image_size", json!([2, 1])),
            ("png_bytes", json!(0)),
        ] {
            let mut result = fixture();
            result["structuredContent"][path] = value;
            assert!(parse_preview(result, "test").is_err());
        }
        let mut result = fixture();
        result["content"][0]["data"] = json!("not a PNG");
        assert!(parse_preview(result, "test").is_err());
    }

    #[test]
    fn disconnected_actions_do_not_claim_success_or_capture() {
        let mut host = Host::new(PathBuf::from("missing-sidecar"));
        let status = host.perform(Action::Status {});
        assert!(!status.status.connected);
        let capture = host.perform(Action::Capture {
            monitor_id: "old".into(),
        });
        assert!(capture.preview.is_none());
        assert!(capture.status.error.is_some());
        let connect = host.perform(Action::Connect {});
        assert!(!connect.status.connected);
        assert!(connect.status.monitors.is_empty());
        assert!(connect.status.error.is_some());
    }

    #[test]
    fn command_surface_rejects_arbitrary_tools_and_unknown_arguments() {
        assert!(
            serde_json::from_value::<Action>(json!({"type":"tools/call","name":"anything"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<Action>(json!({"type":"connect","path":"untrusted.exe"}))
                .is_err()
        );
        assert!(serde_json::from_value::<Action>(json!({"type":"capture"})).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn geometry_change_refuses_capture_before_calling_the_capture_tool() {
        let script = r#"
while (($line = [Console]::ReadLine()) -ne $null) {
    $request = $line | ConvertFrom-Json
    if ($request.params.name -eq 'aeye_list_monitors') {
        $result = @{ isError = $false; structuredContent = @{ monitors = @(@{
            id = 'test'; device = 'fixture'; x = 0; y = 0; width = 200; height = 100; scale_percent = 100; primary = $true
        }) } }
    } else { $result = @{ isError = $true; content = @(@{ type = 'text'; text = 'TEST: capture crossed changed-display boundary' }) } }
    [Console]::WriteLine((@{ jsonrpc = '2.0'; id = $request.id; result = $result } | ConvertTo-Json -Depth 10 -Compress))
}
"#;
        let mut host = Host::new(PathBuf::new());
        host.client = Some(Client::synthetic(script));
        host.monitors.push(Monitor {
            id: "test".into(),
            device: "fixture".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            scale_percent: 100,
            primary: true,
        });
        let outcome = host.perform(Action::Capture {
            monitor_id: "test".into(),
        });
        assert!(outcome.preview.is_none());
        assert_eq!(
            outcome.status.error.as_deref(),
            Some("The selected display changed; choose a display again before capture")
        );
        assert_eq!(outcome.status.monitors[0].width, 200);
    }

    #[test]
    fn untrackable_overlay_results_are_rejected() {
        for meta in [
            json!({}),
            json!({"handle":"h","monitor_id":"other","expires_in_secs":60}),
            json!({"handle":"h","monitor_id":"test","expires_in_secs":600}),
        ] {
            assert!(parse_overlay(&json!({"structuredContent":meta}), "test").is_err());
        }
    }

    #[cfg(windows)]
    #[test]
    fn sidecar_exit_clears_connection_and_cached_monitors() {
        let mut host = Host::new(PathBuf::new());
        host.client = Some(Client::synthetic("exit 0"));
        let outcome = host.perform(Action::Status {});
        assert!(!outcome.status.connected);
        assert!(outcome.status.monitors.is_empty());
        assert!(outcome.status.overlay.is_none());
        assert!(outcome.status.error.is_some());
    }
}
