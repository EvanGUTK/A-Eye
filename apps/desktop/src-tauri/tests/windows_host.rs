//! Explicit live acceptance: captures monitors into memory; never saves pixels.
#![cfg(windows)]

use std::path::PathBuf;

use aeye_desktop::host::{Action, Host};
use serde_json::{to_value, Value};

fn action(host: &mut Host, action: Action) -> Value {
    let value = to_value(host.perform(action)).unwrap();
    assert!(
        value["status"]["error"].is_null(),
        "host error: {}",
        value["status"]["error"]
    );
    value
}

#[test]
#[ignore = "explicit live local-model call; set AEYE_TEST_SIDECAR and AEYE_TEST_OLLAMA_PORT"]
fn real_capture_then_real_ollama_describe() {
    use aeye_desktop::mcp::Client;
    use aeye_desktop::vision::{Describer, Ollama};
    use serde_json::json;

    // Bypasses apps/desktop's Host (issue #52: it does not yet request/pass a lease) by talking
    // to the part-09 sidecar directly, to exercise the real vision.rs/Ollama path against a real
    // capture and a real local model, independent of that unrelated gap.
    let path = PathBuf::from(std::env::var_os("AEYE_TEST_SIDECAR").expect("set AEYE_TEST_SIDECAR"));
    let port: u16 = std::env::var("AEYE_TEST_OLLAMA_PORT")
        .expect("set AEYE_TEST_OLLAMA_PORT")
        .parse()
        .expect("AEYE_TEST_OLLAMA_PORT must be a u16");
    let model =
        std::env::var("AEYE_TEST_OLLAMA_MODEL").unwrap_or_else(|_| "qwen3-vl:2b".to_string());

    let mut client = Client::spawn(&path).expect("spawn sidecar");
    client.initialize().expect("initialize");
    let monitors = client
        .call("aeye_list_monitors", json!({}))
        .expect("list_monitors")["structuredContent"]["monitors"]
        .as_array()
        .cloned()
        .expect("monitors array");
    assert!(!monitors.is_empty());
    let monitor_id = monitors[0]["id"].as_str().unwrap().to_string();

    let epoch = client
        .call("aeye_status", json!({}))
        .expect("status")["structuredContent"]["epoch"]
        .clone();

    let lease = client
        .call(
            "aeye_request_watch",
            json!({
                "monitor_id": monitor_id, "actions": ["capture"], "ttl_secs": 30,
                "observed_epoch": epoch
            }),
        )
        .expect("request_watch");
    let lease_id = lease["structuredContent"]["lease_id"]
        .as_str()
        .expect("lease_id")
        .to_string();

    let frame = client
        .call(
            "aeye_capture_monitor",
            json!({ "monitor_id": monitor_id, "lease_id": lease_id }),
        )
        .expect("capture_monitor");
    let image_item = frame["content"]
        .as_array()
        .and_then(|c| c.iter().find(|i| i["type"] == "image"))
        .expect("image content item");
    let png_base64 = image_item["data"].as_str().expect("image data").to_string();

    let describer = Ollama::new(port, &model).expect("Ollama client construction failed");
    let description = describer
        .describe(&png_base64, &|| false)
        .expect("live describe call failed");
    assert!(!description.trim().is_empty(), "empty description");
    println!("REAL Ollama description ({model} on port {port}): {description}");

    client.close();
}

#[test]
#[ignore = "explicit Windows desktop capture; set AEYE_TEST_SIDECAR to the built part-09 exe"]
fn real_host_handshake_monitors_overlay_capture_and_disconnect() {
    let path = PathBuf::from(std::env::var_os("AEYE_TEST_SIDECAR").expect("set AEYE_TEST_SIDECAR"));
    let mut host = Host::new(path);
    let connected = action(&mut host, Action::Connect {});
    assert_eq!(connected["status"]["connected"], true);
    let monitors = connected["status"]["monitors"].as_array().unwrap();
    assert!(!monitors.is_empty());
    println!("Native host connected; {} monitor(s)", monitors.len());
    for monitor in monitors {
        let id = monitor["id"].as_str().unwrap().to_string();
        let shown = action(
            &mut host,
            Action::ShowOverlay {
                monitor_id: id.clone(),
            },
        );
        assert_eq!(shown["status"]["overlay"]["monitor_id"], id);
        let frame = action(
            &mut host,
            Action::Capture {
                monitor_id: id.clone(),
            },
        );
        assert_eq!(frame["preview"]["monitor_id"], id);
        assert_eq!(frame["preview"]["original_width"], monitor["width"]);
        assert_eq!(frame["preview"]["original_height"], monitor["height"]);
        assert!(frame["preview"]["data_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        let hidden = action(&mut host, Action::HideOverlay {});
        assert!(hidden["status"]["overlay"].is_null());
    }
    action(&mut host, Action::Status {});
    action(&mut host, Action::RefreshMonitors {});
    let disconnected = action(&mut host, Action::Disconnect {});
    assert_eq!(disconnected["status"]["connected"], false);
    assert!(disconnected["status"]["monitors"]
        .as_array()
        .unwrap()
        .is_empty());
    action(&mut host, Action::Connect {});
    host.shutdown();
    assert_eq!(
        action(&mut host, Action::Status {})["status"]["connected"],
        false
    );
    println!(
        "Real host overlay/capture on every monitor, stop, refresh, disconnect, reconnect: PASS"
    );
}
