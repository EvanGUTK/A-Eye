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
