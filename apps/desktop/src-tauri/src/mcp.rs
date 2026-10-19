//! A deliberately small host for part 09's pinned MCP contract. Only the native host owns pipes.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const PROTOCOL: &str = "2025-11-25";
// Part 09 allows 8 MiB PNGs, which expand to ~11 MiB in base64, plus metadata.
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const EXIT_TIMEOUT: Duration = Duration::from_secs(2);
const EXIT_POLL: Duration = Duration::from_millis(20);
const REQUIRED_TOOLS: [&str; 4] = [
    "aeye_list_monitors",
    "aeye_start_overlay",
    "aeye_stop_overlay",
    "aeye_capture_monitor",
];

pub struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Result<Value, String>>,
    next_id: u64,
    pub healthy: bool,
}

impl Client {
    pub fn spawn(path: &Path) -> Result<Self, String> {
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut command = Command::new(path);
        // Part 09 no longer accepts --allow-capture (removed in #49): capture/overlay now
        // require a scope lease (aeye_request_watch) instead of a process-lifetime flag.
        // TODO(#52): this host does not yet call aeye_request_watch or pass lease_id to
        // aeye_start_overlay/aeye_capture_monitor, so those two calls will be denied
        // (NO_ACTIVE_SCOPE) until that wiring lands. Spawning with no flag at least lets the
        // sidecar start so list_monitors/status keep working instead of the child refusing to
        // launch outright.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        Self::start(command)
    }

    fn start(mut command: Command) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("Could not start the bundled MCP sidecar: {e}"))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("Sidecar stdout pipe missing")?;
        let (sender, replies) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let reply = read_message(&mut reader);
                let stop = reply.is_err();
                if sender.send(reply).is_err() || stop {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            replies,
            next_id: 1,
            healthy: true,
        })
    }

    #[cfg(all(test, windows))]
    pub(crate) fn synthetic(script: &str) -> Self {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-Command", script]);
        Self::start(command).expect("start synthetic stdio fixture")
    }

    pub fn initialize(&mut self) -> Result<(), String> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL, "capabilities": {},
                "clientInfo": { "name": "aeye-desktop", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        validate_initialize(&result)?;
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
        let tools = self.request("tools/list", json!({}))?;
        validate_tools(&tools)
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        let pipe = self.stdin.as_mut().ok_or("MCP connection is closed")?;
        writeln!(pipe, "{message}")
            .and_then(|_| pipe.flush())
            .map_err(|e| format!("Could not write to the MCP sidecar: {e}"))
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
    }

    fn request_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let result = self
            .send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .and_then(|_| {
                self.replies.recv_timeout(timeout).map_err(|_| {
                    "MCP reply timed out or the sidecar exited; reconnect to retry".to_string()
                })?
            })
            .and_then(|reply| response_result(reply, id));
        if result.is_err() {
            self.healthy = false;
        }
        result
    }

    pub fn call(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )?;
        tool_result(result)
    }

    pub fn close(&mut self) {
        self.healthy = false;
        // EOF is the server's authoritative shutdown signal, including overlay/hotkey cleanup.
        self.stdin.take();
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => thread::sleep(EXIT_POLL),
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Could not inspect MCP child exit: {e}");
                    break;
                }
            }
        }
        if let Err(e) = self.child.kill() {
            eprintln!("Could not terminate MCP child: {e}");
        }
        if let Err(e) = self.child.wait() {
            eprintln!("Could not reap MCP child: {e}");
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

fn read_message(reader: &mut impl BufRead) -> Result<Value, String> {
    let mut bytes = Vec::new();
    (&mut *reader)
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| format!("Could not read the MCP pipe: {e}"))?;
    if bytes.is_empty() {
        return Err("MCP sidecar closed its output pipe".into());
    }
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("MCP reply exceeded the 16 MiB limit".into());
    }
    if bytes.last() != Some(&b'\n') {
        return Err("MCP sidecar returned an incomplete message".into());
    }
    // Never include raw stdout or image data in an error/log.
    serde_json::from_slice(&bytes).map_err(|_| "MCP sidecar returned invalid JSON".into())
}

fn response_result(reply: Value, id: u64) -> Result<Value, String> {
    if reply["jsonrpc"] != "2.0" || reply["id"].as_u64() != Some(id) {
        return Err("MCP reply did not match the pending request".into());
    }
    if reply.get("error").is_some() {
        return Err("MCP sidecar rejected the protocol request".into());
    }
    reply
        .get("result")
        .filter(|v| v.is_object())
        .cloned()
        .ok_or("MCP reply has no result object".into())
}

fn validate_initialize(result: &Value) -> Result<(), String> {
    if result["protocolVersion"] != PROTOCOL || !result["capabilities"]["tools"].is_object() {
        return Err("The bundled sidecar has an unsupported MCP version or no tools".into());
    }
    Ok(())
}

fn validate_tools(result: &Value) -> Result<(), String> {
    let tools = result["tools"].as_array().ok_or("MCP tool list missing")?;
    if REQUIRED_TOOLS
        .iter()
        .any(|name| !tools.iter().any(|t| t["name"] == *name))
    {
        return Err("The bundled MCP sidecar is missing a desktop tool".into());
    }
    Ok(())
}

fn tool_result(result: Value) -> Result<Value, String> {
    match result["isError"].as_bool() {
        Some(false) => Ok(result),
        Some(true) => {
            let message = result["content"]
                .as_array()
                .and_then(|items| items.iter().find(|item| item["type"] == "text"))
                .and_then(|item| item["text"].as_str())
                .unwrap_or("MCP tool failed");
            Err(message.chars().take(512).collect())
        }
        None => Err("Malformed MCP tool outcome".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn framing_rejects_invalid_incomplete_and_oversized_output() {
        for bytes in [b"not-json\n".as_slice(), b"{}", b""] {
            assert!(read_message(&mut Cursor::new(bytes)).is_err());
        }
        assert!(read_message(&mut Cursor::new(vec![b' '; MAX_MESSAGE_BYTES + 1])).is_err());
        assert_eq!(
            read_message(&mut Cursor::new(b"{}\r\n")).unwrap(),
            json!({})
        );
    }

    #[test]
    fn rpc_errors_and_wrong_ids_are_not_success() {
        assert!(response_result(json!({"jsonrpc":"2.0","id":2,"result":{}}), 1).is_err());
        assert!(response_result(json!({"jsonrpc":"2.0","id":1,"error":{"code":-1}}), 1).is_err());
        assert!(response_result(json!({"jsonrpc":"2.0","id":1,"result":null}), 1).is_err());
    }

    #[test]
    fn handshake_requires_compatible_version_and_tools() {
        assert!(
            validate_initialize(&json!({"protocolVersion":"old","capabilities":{"tools":{}}}))
                .is_err()
        );
        assert!(validate_initialize(
            &json!({"protocolVersion":PROTOCOL,"capabilities":{"tools":{}}})
        )
        .is_ok());
        assert!(validate_tools(&json!({"tools":[]})).is_err());
        assert!(
            validate_tools(&json!({"tools":REQUIRED_TOOLS.map(|name| json!({"name":name}))}))
                .is_ok()
        );
    }

    #[test]
    fn tool_failure_is_distinct_from_transport_failure() {
        assert_eq!(
            tool_result(
                json!({"isError":true,"content":[{"type":"text","text":"capture disabled"}]})
            )
            .unwrap_err(),
            "capture disabled"
        );
        assert!(tool_result(json!({"content":[]})).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn child_timeout_poisoning_and_eof_cleanup_are_bounded() {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-Command",
            "while ([Console]::ReadLine() -ne $null) {}",
        ]);
        let mut client = Client::start(command).unwrap();
        let began = Instant::now();
        assert!(client
            .request_with_timeout("ping", json!({}), Duration::from_millis(100))
            .is_err());
        assert!(!client.healthy);
        client.close();
        assert!(client.child.try_wait().unwrap().is_some());
        assert!(began.elapsed() < Duration::from_secs(5));
    }
}
