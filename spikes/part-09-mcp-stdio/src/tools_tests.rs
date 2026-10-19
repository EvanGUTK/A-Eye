use super::fake::{FakeBackend, FAKE_MONITOR_ID};
use super::*;
use spike_10_scope_leases::{ManualClock, MAX_OUTSTANDING_LEASES};

fn run<C: Clock>(name: &str, args: Value, config: &ToolConfig<C>) -> (Value, FakeBackend) {
    let mut backend = FakeBackend::default();
    let result = invoke(name, args, &mut backend, config);
    (result, backend)
}

fn invoke<C: Clock>(
    name: &str,
    args: Value,
    backend: &mut FakeBackend,
    config: &ToolConfig<C>,
) -> Value {
    call(name, args, backend, config)
        .expect("known tool")
        .into_result()
}

fn manual_config() -> (ToolConfig<Arc<ManualClock>>, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new());
    (
        ToolConfig {
            broker: Arc::new(LeaseBroker::with_clock(clock.clone())),
        },
        clock,
    )
}

fn grant<C: Clock>(config: &ToolConfig<C>, monitor: &str, actions: Actions, ttl: u64) -> String {
    config
        .broker
        .grant(LeaseRequest {
            scope: Scope::monitor(monitor),
            actions,
            ttl: Duration::from_secs(ttl),
            observed_epoch: config.broker.epoch(),
        })
        .unwrap()
        .id
        .to_string()
}

fn both_actions() -> Actions {
    Actions::NONE.with(Action::Capture).with(Action::Overlay)
}

fn request_args<C: Clock>(config: &ToolConfig<C>) -> Value {
    json!({
        "monitor_id": FAKE_MONITOR_ID,
        "actions": ["capture", "overlay"],
        "ttl_secs": 5,
        "observed_epoch": config.broker.epoch().get(),
    })
}

fn assert_error(result: &Value, expected: &str) {
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(expected),
        "expected {expected:?}: {result}"
    );
    assert!(result.get("structuredContent").is_none(), "{result}");
    assert!(
        result["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] == "text"),
        "denial leaked an image: {result}"
    );
}

#[test]
fn colour_parsing() {
    assert_eq!(parse_colour("#FF8C00").unwrap(), Rgb(255, 140, 0));
    assert_eq!(parse_colour("00ff7f").unwrap(), Rgb(0, 255, 127));
    for bad in ["", "#FFF", "#GG0000", "#FF8C0000", "orange", "#ff8c0é"] {
        assert!(parse_colour(bad).is_err(), "{bad} should be rejected");
    }
}

#[test]
fn unknown_tool_is_none() {
    let mut backend = FakeBackend::default();
    assert!(call("nope", json!({}), &mut backend, &ToolConfig::default()).is_none());
    assert!(backend.log.is_empty());
}

#[test]
fn missing_malformed_and_unknown_leases_deny_both_actions_before_backend() {
    let config = ToolConfig::default();
    for tool in [TOOL_CAPTURE_MONITOR, TOOL_START_OVERLAY] {
        for lease in [None, Some(""), Some("bogus"), Some("lease-0.0")] {
            let mut args = json!({ "monitor_id": FAKE_MONITOR_ID });
            if let Some(id) = lease {
                args["lease_id"] = json!(id);
            }
            let (result, backend) = run(tool, args, &config);
            assert_error(&result, "NO_ACTIVE_SCOPE");
            assert!(
                backend.log.is_empty(),
                "{tool} reached backend with {lease:?}"
            );
        }
    }
}

#[test]
fn lease_from_another_broker_does_not_authorize_dispatch() {
    let issuer = ToolConfig::default();
    let config = ToolConfig::default();
    let lease = grant(&issuer, FAKE_MONITOR_ID, both_actions(), 5);
    for tool in [TOOL_CAPTURE_MONITOR, TOOL_START_OVERLAY] {
        let (result, backend) = run(
            tool,
            json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
            &config,
        );
        assert_error(&result, "NO_ACTIVE_SCOPE");
        assert!(backend.log.is_empty());
    }
}

#[test]
fn action_mismatch_denies_both_actions_before_backend() {
    let config = ToolConfig::default();
    for (tool, granted) in [
        (TOOL_CAPTURE_MONITOR, Action::Overlay),
        (TOOL_START_OVERLAY, Action::Capture),
    ] {
        let lease = grant(&config, FAKE_MONITOR_ID, Actions::NONE.with(granted), 5);
        let (result, backend) = run(
            tool,
            json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
            &config,
        );
        assert_error(&result, "NO_ACTIVE_SCOPE");
        assert!(backend.log.is_empty());
    }
}

#[test]
fn monitor_scope_mismatch_denies_both_actions_before_backend() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    for tool in [TOOL_CAPTURE_MONITOR, TOOL_START_OVERLAY] {
        for monitor in ["hm-2", "hm-1 ", "", "hm-1\0"] {
            let (result, backend) = run(
                tool,
                json!({ "monitor_id": monitor, "lease_id": lease }),
                &config,
            );
            assert_error(&result, "SCOPE_MISMATCH");
            assert!(backend.log.is_empty());
        }
    }
}

#[test]
fn revoke_before_dispatch_denies_both_actions_with_stale_epoch() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    config.broker.revoke_all();
    for tool in [TOOL_CAPTURE_MONITOR, TOOL_START_OVERLAY] {
        let (result, backend) = run(
            tool,
            json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
            &config,
        );
        assert_error(&result, "STALE_EPOCH");
        assert!(backend.log.is_empty());
    }
}

#[test]
fn expiry_at_exact_deadline_denies_both_actions_before_backend() {
    let (config, clock) = manual_config();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    clock.advance(Duration::from_secs(5));
    for tool in [TOOL_CAPTURE_MONITOR, TOOL_START_OVERLAY] {
        let (result, backend) = run(
            tool,
            json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
            &config,
        );
        assert_error(&result, "NO_ACTIVE_SCOPE");
        assert!(backend.log.is_empty());
    }
}

#[test]
fn capture_returns_png_image_content_and_metadata() {
    let config = ToolConfig::default();
    config.broker.revoke_all();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    let (result, backend) = run(
        TOOL_CAPTURE_MONITOR,
        json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease, "max_dimension": 100 }),
        &config,
    );
    assert_eq!(result["isError"], false);
    assert_eq!(backend.log, ["capture hm-1"]);
    assert_eq!(result["content"][0]["type"], "image");
    assert_eq!(result["content"][0]["mimeType"], "image/png");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(result["content"][0]["data"].as_str().unwrap())
        .unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(&bytes))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (100, 50));
    assert_eq!(&pixels[..3], &[10, 20, 30]);
    let metadata = &result["structuredContent"];
    assert_eq!(metadata["original_size"], json!([400, 200]));
    assert_eq!(metadata["image_size"], json!([100, 50]));
    assert_eq!(metadata["scale"], 0.25);
    assert_eq!(metadata["png_bytes"], bytes.len());
    assert_eq!(metadata["capture_epoch"], config.broker.epoch().get());
    assert_eq!(metadata["monitor_id"], FAKE_MONITOR_ID);
    assert_eq!(metadata["cursor_drawn"], false);
    assert_eq!(metadata["note"], UNTRUSTED_NOTE);
    let text_metadata: Value =
        serde_json::from_str(result["content"][1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text_metadata, metadata);
}

#[test]
fn capture_defaults_and_dimension_boundaries() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    for (dimension, expected) in [
        (None, [400, 200]),
        (Some(64), [64, 32]),
        (Some(8192), [400, 200]),
    ] {
        let mut args = json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease });
        if let Some(dimension) = dimension {
            args["max_dimension"] = json!(dimension);
        }
        let (result, _) = run(TOOL_CAPTURE_MONITOR, args, &config);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["image_size"], json!(expected));
    }
}

#[test]
fn capture_rejects_bad_arguments_before_backend() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    for mut args in [
        json!({}),
        json!({ "monitor_id": 3 }),
        json!({ "monitor_id": FAKE_MONITOR_ID, "max_dimension": 63 }),
        json!({ "monitor_id": FAKE_MONITOR_ID, "max_dimension": 8193 }),
        json!({ "monitor_id": FAKE_MONITOR_ID, "max_dimension": -1 }),
        json!({ "monitor_id": FAKE_MONITOR_ID, "extra": true }),
    ] {
        args["lease_id"] = json!(lease);
        let (result, backend) = run(TOOL_CAPTURE_MONITOR, args.clone(), &config);
        assert_eq!(result["isError"], true, "{args}");
        assert!(backend.log.is_empty(), "{args} reached backend");
    }
    let (result, backend) = run(
        TOOL_CAPTURE_MONITOR,
        json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": 3 }),
        &config,
    );
    assert_error(&result, "invalid arguments");
    assert!(backend.log.is_empty());
}

#[test]
fn backend_errors_become_tool_errors() {
    let config = ToolConfig::default();
    let lease = grant(&config, "bogus", both_actions(), 5);
    let (result, backend) = run(
        TOOL_CAPTURE_MONITOR,
        json!({ "monitor_id": "bogus", "lease_id": lease }),
        &config,
    );
    assert_error(&result, "unknown monitor_id");
    assert_eq!(backend.log, ["capture bogus"]);
    let (result, _) = run(TOOL_STOP_OVERLAY, json!({ "handle": "missing" }), &config);
    assert_error(&result, "unknown overlay handle");
}

#[test]
fn revoke_during_capture_discards_pixels() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    let broker = config.broker.clone();
    let mut backend = FakeBackend {
        during_work: Some(Box::new(move || {
            broker.revoke_all();
        })),
        ..Default::default()
    };
    let result = invoke(
        TOOL_CAPTURE_MONITOR,
        json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
        &mut backend,
        &config,
    );
    assert_error(&result, "STALE_EPOCH");
    assert_eq!(backend.log, ["capture hm-1"]);
}

#[test]
fn expiry_during_capture_discards_pixels_at_deadline() {
    let (config, clock) = manual_config();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    let mut backend = FakeBackend {
        during_work: Some(Box::new(move || clock.advance(Duration::from_secs(5)))),
        ..Default::default()
    };
    let result = invoke(
        TOOL_CAPTURE_MONITOR,
        json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
        &mut backend,
        &config,
    );
    assert_error(&result, "NO_ACTIVE_SCOPE");
    assert_eq!(backend.log, ["capture hm-1"]);
}

#[test]
fn overlay_defaults_and_custom_settings() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    for (settings, expected) in [
        (json!({}), "start hm-1 Rgb(255, 140, 0) 60s"),
        (
            json!({ "color": "00ff7f", "duration_secs": 1 }),
            "start hm-1 Rgb(0, 255, 127) 1s",
        ),
        (
            json!({ "duration_secs": 600 }),
            "start hm-1 Rgb(255, 140, 0) 600s",
        ),
    ] {
        let mut args = json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease });
        args.as_object_mut()
            .unwrap()
            .extend(settings.as_object().unwrap().clone());
        let maximum_duration = args
            .get("duration_secs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_OVERLAY_SECS);
        let (result, backend) = run(TOOL_START_OVERLAY, args, &config);
        assert_eq!(result["isError"], false);
        assert_eq!(backend.log, [expected]);
        assert_eq!(result["structuredContent"]["handle"], "overlay-1");
        assert_eq!(result["structuredContent"]["local_stop"], "Ctrl+Alt+Q");
        assert_eq!(
            result["structuredContent"]["maximum_duration_secs"],
            maximum_duration
        );
        assert_eq!(result["structuredContent"]["ends_on_lease_expiry"], true);
        assert_eq!(
            result["structuredContent"]["capture_epoch"],
            config.broker.epoch().get()
        );
    }
}

#[test]
fn overlay_rejects_bad_arguments_before_backend() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    for mut args in [
        json!({}),
        json!({ "monitor_id": 3 }),
        json!({ "monitor_id": FAKE_MONITOR_ID, "duration_secs": 0 }),
        json!({ "monitor_id": FAKE_MONITOR_ID, "duration_secs": 601 }),
        json!({ "monitor_id": FAKE_MONITOR_ID, "color": "red" }),
        json!({ "monitor_id": FAKE_MONITOR_ID, "extra": true }),
    ] {
        args["lease_id"] = json!(lease);
        let (result, backend) = run(TOOL_START_OVERLAY, args.clone(), &config);
        assert_eq!(result["isError"], true, "{args}");
        assert!(backend.log.is_empty(), "{args} reached backend");
    }
}

#[test]
fn revoke_during_overlay_discards_handle_and_cleans_up() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    let broker = config.broker.clone();
    let mut backend = FakeBackend {
        during_work: Some(Box::new(move || {
            broker.revoke_all();
        })),
        ..Default::default()
    };
    let result = invoke(
        TOOL_START_OVERLAY,
        json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
        &mut backend,
        &config,
    );
    assert_error(&result, "STALE_EPOCH");
    assert_eq!(
        backend.log,
        ["start hm-1 Rgb(255, 140, 0) 60s", "stop overlay-1"]
    );
    assert!(backend.overlays.is_empty());
}

#[test]
fn expiry_during_overlay_discards_handle_and_cleans_up_at_deadline() {
    let (config, clock) = manual_config();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    let mut backend = FakeBackend {
        during_work: Some(Box::new(move || clock.advance(Duration::from_secs(5)))),
        ..Default::default()
    };
    let result = invoke(
        TOOL_START_OVERLAY,
        json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
        &mut backend,
        &config,
    );
    assert_error(&result, "NO_ACTIVE_SCOPE");
    assert_eq!(
        backend.log,
        ["start hm-1 Rgb(255, 140, 0) 60s", "stop overlay-1"]
    );
    assert!(backend.overlays.is_empty());
}

#[test]
fn denied_overlay_cleanup_failure_is_surfaced() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    let broker = config.broker.clone();
    let mut backend = FakeBackend {
        during_work: Some(Box::new(move || {
            broker.revoke_all();
        })),
        cleanup_error: true,
        ..Default::default()
    };
    let result = invoke(
        TOOL_START_OVERLAY,
        json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
        &mut backend,
        &config,
    );
    assert_error(&result, "STALE_EPOCH");
    assert_error(&result, "overlay cleanup failed");
    assert_eq!(backend.log.last().unwrap(), "stop overlay-1");
    assert_eq!(backend.overlays, ["overlay-1"]);
}

#[test]
fn stopping_an_overlay_is_ungated_even_after_revocation() {
    let config = ToolConfig::default();
    let lease = grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    let mut backend = FakeBackend::default();
    let started = invoke(
        TOOL_START_OVERLAY,
        json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease }),
        &mut backend,
        &config,
    );
    assert_eq!(started["isError"], false);
    config.broker.revoke_all();
    let result = invoke(
        TOOL_STOP_OVERLAY,
        json!({ "handle": started["structuredContent"]["handle"] }),
        &mut backend,
        &config,
    );
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["ended_by"], "stopped");
    assert!(backend.overlays.is_empty());
}

#[test]
fn status_reports_current_epoch_active_leases_and_explicit_auto_approval() {
    let (config, clock) = manual_config();
    let (initial, backend) = run(TOOL_STATUS, Value::Null, &config);
    assert_eq!(initial["isError"], false);
    assert_eq!(initial["structuredContent"]["epoch"], 0);
    assert_eq!(initial["structuredContent"]["active_leases"], 0);
    assert_eq!(
        initial["structuredContent"]["approval_mode"],
        "spike_auto_approve"
    );
    assert!(backend.log.is_empty());
    grant(&config, FAKE_MONITOR_ID, both_actions(), 5);
    let (active, _) = run(TOOL_STATUS, json!({}), &config);
    assert_eq!(active["structuredContent"]["active_leases"], 1);
    clock.advance(Duration::from_secs(5));
    let (expired, _) = run(TOOL_STATUS, json!({}), &config);
    assert_eq!(expired["structuredContent"]["active_leases"], 0);
    config.broker.revoke_all();
    let (stopped, _) = run(TOOL_STATUS, json!({}), &config);
    assert_eq!(stopped["structuredContent"]["epoch"], 1);
}

#[test]
fn request_watch_auto_approves_only_requested_scope_actions_and_ttl() {
    let (config, clock) = manual_config();
    let mut args = request_args(&config);
    args["actions"] = json!(["capture"]);
    let (result, backend) = run(TOOL_REQUEST_WATCH, args, &config);
    assert_eq!(result["isError"], false);
    assert_eq!(backend.log, ["list_monitors", "ensure_local_stop"]);
    let lease = &result["structuredContent"];
    assert_eq!(lease["epoch"], config.broker.epoch().get());
    assert_eq!(lease["monitor_id"], FAKE_MONITOR_ID);
    assert_eq!(lease["actions"], json!(["capture"]));
    assert_eq!(lease["expires_in_secs"], 5);
    assert_eq!(lease["approval_mode"], "spike_auto_approve");
    let args = json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": lease["lease_id"] });
    let (capture, _) = run(TOOL_CAPTURE_MONITOR, args.clone(), &config);
    assert_eq!(capture["isError"], false);
    let (overlay, backend) = run(TOOL_START_OVERLAY, args.clone(), &config);
    assert_error(&overlay, "NO_ACTIVE_SCOPE");
    assert!(backend.log.is_empty());
    clock.advance(Duration::from_secs(5));
    let (expired, backend) = run(TOOL_CAPTURE_MONITOR, args, &config);
    assert_error(&expired, "NO_ACTIVE_SCOPE");
    assert!(backend.log.is_empty());
}

#[test]
fn request_watch_validates_actions_ttl_and_unknown_fields_before_backend() {
    let config = ToolConfig::default();
    for (field, invalid) in [
        ("actions", json!([])),
        ("actions", json!(["capture", "capture"])),
        ("actions", json!(["overlay", "overlay"])),
        ("actions", json!(["capture", "overlay", "capture"])),
        ("actions", json!(["delete"])),
        ("ttl_secs", json!(0)),
        ("ttl_secs", json!(601)),
        ("ttl_secs", json!(-1)),
        ("ttl_secs", json!(1.5)),
        ("extra", json!(true)),
    ] {
        let mut args = request_args(&config);
        args[field] = invalid;
        let (result, backend) = run(TOOL_REQUEST_WATCH, args.clone(), &config);
        assert_eq!(result["isError"], true, "{args}");
        assert!(backend.log.is_empty(), "{args} reached backend");
        assert_eq!(config.broker.active_leases(), 0);
    }
    for field in ["monitor_id", "actions", "ttl_secs", "observed_epoch"] {
        let mut args = request_args(&config);
        args.as_object_mut().unwrap().remove(field);
        let (result, backend) = run(TOOL_REQUEST_WATCH, args, &config);
        assert_error(&result, "invalid arguments");
        assert!(backend.log.is_empty());
    }
}

#[test]
fn request_watch_accepts_ttl_boundaries_and_overlay_only_action() {
    let config = ToolConfig::default();
    for ttl in [1, MAX_TTL.as_secs()] {
        let mut args = request_args(&config);
        args["ttl_secs"] = json!(ttl);
        args["actions"] = json!(["overlay"]);
        let (result, _) = run(TOOL_REQUEST_WATCH, args, &config);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["expires_in_secs"], ttl);
        let (capture, backend) = run(
            TOOL_CAPTURE_MONITOR,
            json!({ "monitor_id": FAKE_MONITOR_ID, "lease_id": result["structuredContent"]["lease_id"] }),
            &config,
        );
        assert_error(&capture, "NO_ACTIVE_SCOPE");
        assert!(backend.log.is_empty());
    }
}

#[test]
fn request_watch_rejects_unknown_monitor_without_granting_or_preparing_stop() {
    let config = ToolConfig::default();
    let mut args = request_args(&config);
    args["monitor_id"] = json!("hm-unknown");
    let (result, backend) = run(TOOL_REQUEST_WATCH, args, &config);
    assert_error(&result, "unknown monitor_id");
    assert_eq!(backend.log, ["list_monitors"]);
    assert_eq!(config.broker.active_leases(), 0);
}

#[test]
fn request_watch_rejects_past_and_future_approval_epochs_before_backend() {
    let config = ToolConfig::default();
    let old_args = request_args(&config);
    config.broker.revoke_all();
    let mut future_args = request_args(&config);
    future_args["observed_epoch"] = json!(config.broker.epoch().get() + 1);
    for args in [old_args, future_args] {
        let (result, backend) = run(TOOL_REQUEST_WATCH, args, &config);
        assert_error(&result, "STALE_EPOCH");
        assert!(backend.log.is_empty());
        assert_eq!(config.broker.active_leases(), 0);
    }
}

#[test]
fn stop_during_local_stop_preparation_rejects_late_approval() {
    let config = ToolConfig::default();
    let broker = config.broker.clone();
    let mut backend = FakeBackend {
        on_prepare_stop: Some(Box::new(move || {
            broker.revoke_all();
        })),
        ..Default::default()
    };
    let result = invoke(
        TOOL_REQUEST_WATCH,
        request_args(&config),
        &mut backend,
        &config,
    );
    assert_error(&result, "STALE_EPOCH");
    assert_eq!(backend.log, ["list_monitors", "ensure_local_stop"]);
    assert_eq!(config.broker.active_leases(), 0);
}

#[test]
fn native_stop_registration_failure_grants_no_lease() {
    let config = ToolConfig::default();
    let mut backend = FakeBackend {
        stop_unavailable: true,
        ..Default::default()
    };
    let result = invoke(
        TOOL_REQUEST_WATCH,
        request_args(&config),
        &mut backend,
        &config,
    );
    assert_eq!(result["isError"], true);
    assert_eq!(backend.log, ["list_monitors", "ensure_local_stop"]);
    assert_eq!(config.broker.active_leases(), 0);
}

#[test]
fn request_watch_preserves_lease_cap_and_recovers_after_expiry() {
    let (config, clock) = manual_config();
    for _ in 0..MAX_OUTSTANDING_LEASES {
        let (result, _) = run(TOOL_REQUEST_WATCH, request_args(&config), &config);
        assert_eq!(result["isError"], false);
    }
    let (full, _) = run(TOOL_REQUEST_WATCH, request_args(&config), &config);
    assert_error(&full, "maximum");
    assert_eq!(config.broker.active_leases(), MAX_OUTSTANDING_LEASES);
    clock.advance(Duration::from_secs(5));
    let (new, _) = run(TOOL_REQUEST_WATCH, request_args(&config), &config);
    assert_eq!(new["isError"], false);
    assert_eq!(config.broker.active_leases(), 1);
}

#[test]
fn no_argument_tools_accept_missing_null_and_empty_but_not_extras() {
    let config = ToolConfig::default();
    for tool in [TOOL_STATUS, TOOL_LIST_MONITORS, TOOL_GET_CURSOR] {
        for args in [Value::Null, json!({})] {
            let (result, _) = run(tool, args, &config);
            assert_eq!(result["isError"], false, "{tool}: {result}");
        }
        let (result, backend) = run(tool, json!({ "x": 1 }), &config);
        assert_error(&result, "invalid arguments");
        assert!(backend.log.is_empty());
    }
    let (monitors, _) = run(TOOL_LIST_MONITORS, json!({}), &config);
    assert_eq!(
        monitors["structuredContent"]["monitors"][0]["id"],
        FAKE_MONITOR_ID
    );
    let (cursor, _) = run(TOOL_GET_CURSOR, json!({}), &config);
    assert_eq!(cursor["structuredContent"]["x"], 5);
    assert_eq!(cursor["structuredContent"]["y"], 6);
    assert_eq!(
        cursor["structuredContent"]["monitor"]["monitor_id"],
        FAKE_MONITOR_ID
    );
}

#[test]
fn every_definition_is_well_formed_and_gated_actions_require_lease_ids() {
    let defs = definitions();
    let mut names = Vec::new();
    for definition in defs.as_array().unwrap() {
        let name = definition["name"].as_str().unwrap();
        assert!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'),
            "{name}"
        );
        assert_eq!(definition["inputSchema"]["type"], "object");
        assert_eq!(definition["inputSchema"]["additionalProperties"], false);
        assert!(definition["description"].as_str().unwrap().len() > 10);
        if [TOOL_CAPTURE_MONITOR, TOOL_START_OVERLAY].contains(&name) {
            assert!(definition["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .contains(&json!("lease_id")));
            assert_eq!(
                definition["inputSchema"]["properties"]["lease_id"]["type"],
                "string"
            );
        }
        names.push(name.to_string());
    }
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len());
    assert_eq!(names.len(), 7);
    assert!(names.iter().any(|name| name == TOOL_STATUS));
    assert!(names.iter().any(|name| name == TOOL_REQUEST_WATCH));
}
