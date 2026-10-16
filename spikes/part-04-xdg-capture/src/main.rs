//! A-Eye spike 04 runner. Exit code 0 whenever the capability record was
//! produced, including when the portal is unavailable, consent was denied or
//! consent timed out (those are recorded as FAIL/UNVERIFIED, never as PASS).

use std::time::Instant;

use spike_04_xdg_capture::frame;
use spike_04_xdg_capture::portal::{self, PortalError, StartedSession, CONSENT_TIMEOUT};
use spike_04_xdg_capture::pw;
use spike_04_xdg_capture::record::{Record, Status};
use spike_04_xdg_capture::sysinfo;

fn environment(rec: &mut Record) -> (sysinfo::SessionInfo, sysinfo::PortalInfo) {
    let os = sysinfo::os_info();
    rec.row("system", "OS", os.pretty_name);
    rec.row("system", "kernel", os.kernel);
    rec.row(
        "system",
        "GPU driver(s)",
        if os.gpu_drivers.is_empty() {
            "unknown (no /sys/class/drm card driver link)".to_string()
        } else {
            os.gpu_drivers.join(", ")
        },
    );

    let sess = sysinfo::session_info();
    rec.row(
        "session",
        "XDG_CURRENT_DESKTOP",
        sess.xdg_current_desktop.clone(),
    );
    rec.row("session", "XDG_SESSION_TYPE", sess.xdg_session_type.clone());
    rec.row("session", "WAYLAND_DISPLAY", sess.wayland_display.clone());
    rec.row(
        "session",
        "compositor version",
        sess.compositor_version.clone(),
    );

    let p = sysinfo::portal_info(&sess.xdg_current_desktop);
    rec.row("portal", "frontend package", p.frontend_package.clone());
    rec.row(
        "portal",
        "frontend exports ScreenCast",
        match p.frontend_exports_screencast {
            Some(b) => b.to_string(),
            None => "unknown (busctl introspect failed)".to_string(),
        },
    );
    rec.row(
        "portal",
        "running portal processes",
        if p.running_portal_processes.is_empty() {
            "none seen".to_string()
        } else {
            p.running_portal_processes.join(", ")
        },
    );
    for b in &p.backends {
        rec.row(
            "portal",
            "backend file",
            format!(
                "{} dbus={} UseIn={} ScreenCast={}",
                b.file,
                b.dbus_name,
                b.use_in.join(";"),
                b.has_screencast
            ),
        );
    }
    match &p.screencast_backend_for_desktop {
        Some((b, ver)) => rec.row(
            "portal",
            "ScreenCast backend for this desktop",
            format!("{} ({ver})", b.dbus_name),
        ),
        None => rec.row(
            "portal",
            "ScreenCast backend for this desktop",
            format!(
                "NONE installed (no *.portal file declares ScreenCast for '{}')",
                sess.xdg_current_desktop
            ),
        ),
    }

    let (pw_rt, pw_build) = sysinfo::pipewire_versions();
    rec.row("versions", "libpipewire runtime", pw_rt);
    rec.row("versions", "libpipewire build-time", pw_build);
    rec.row("versions", "ashpd", sysinfo::locked_version("ashpd"));
    rec.row(
        "versions",
        "pipewire (pipewire-rs)",
        sysinfo::locked_version("pipewire"),
    );
    rec.row("versions", "png", sysinfo::locked_version("png"));
    rec.row("versions", "zbus", sysinfo::locked_version("zbus"));
    rec.row("versions", "tokio", sysinfo::locked_version("tokio"));
    rec.row("versions", "rustc", env!("SPIKE_RUSTC_VERSION"));
    (sess, p)
}

fn record_monitors(rec: &mut Record) -> Option<Vec<sysinfo::Monitor>> {
    let mons = sysinfo::monitors();
    match &mons {
        Some(m) => {
            rec.row(
                "monitors",
                "count (compositor, hyprctl)",
                m.len().to_string(),
            );
            for mon in m {
                let (lw, lh) = mon.logical_size();
                rec.row(
                    "monitors",
                    "monitor",
                    format!(
                        "{} {}x{} px scale {} logical {}x{} at ({},{})",
                        mon.name, mon.width, mon.height, mon.scale, lw, lh, mon.x, mon.y
                    ),
                );
            }
        }
        None => rec.row(
            "monitors",
            "count",
            "UNVERIFIED (hyprctl monitors unavailable)",
        ),
    }
    mons
}

/// Computed from the probe results, not a fixed string.
fn unavailable_evidence(portal: &sysinfo::PortalInfo, desktop: &str) -> String {
    let declaring = portal.backends.iter().filter(|b| b.has_screencast).count();
    format!(
        "negative result: ashpd PortalNotFound for ScreenCast; frontend introspection says exports ScreenCast={}; \
         {declaring} of {} *.portal backend files declare ScreenCast; ScreenCast backend for '{desktop}': {}; \
         so no consent dialog can appear",
        portal
            .frontend_exports_screencast
            .map(|b| b.to_string())
            .unwrap_or_else(|| "unknown".into()),
        portal.backends.len(),
        portal
            .screencast_backend_for_desktop
            .as_ref()
            .map(|(b, _)| b.dbus_name.clone())
            .unwrap_or_else(|| "none".into()),
    )
}

/// Record a failed/timed-out interactive step on `id`/`name`.
fn record_consent_failure(rec: &mut Record, id: &'static str, name: &'static str, e: &PortalError) {
    match e {
        PortalError::TimedOut => {
            rec.check(
                id,
                name,
                Status::Unverified,
                "UNVERIFIED: no consent within 90s",
            );
            rec.unknown("consent timed out after 90s; no live frame was attempted");
        }
        PortalError::Denied => {
            rec.check(id, name, Status::Fail, "FAIL: user denied");
            rec.unknown("user denied the monitor selection / consent; no live frame was attempted");
        }
        other => {
            rec.check(id, name, Status::Fail, other.to_string());
            rec.unknown(format!("portal {id}: {other}"));
        }
    }
}

async fn portal_stage(
    rec: &mut Record,
    portal_info: &sysinfo::PortalInfo,
    desktop: &str,
) -> Option<StartedSession> {
    let created = match portal::create().await {
        Ok(c) => {
            rec.check(
                "P1",
                "ScreenCast interface bound",
                Status::Pass,
                format!(
                    "interface version {}",
                    c.facts
                        .interface_version
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                ),
            );
            rec.check(
                "P2",
                "create_session",
                Status::Pass,
                "session object created",
            );
            c
        }
        Err(e) => {
            rec.check(
                "P1",
                "ScreenCast interface bound",
                Status::Fail,
                e.to_string(),
            );
            rec.check(
                "P2",
                "create_session",
                Status::Unverified,
                "not attempted: interface unavailable",
            );
            rec.check(
                "P3",
                "select_sources(monitor)",
                Status::Unverified,
                "not attempted",
            );
            rec.check(
                "P4",
                "consent dialog -> selected stream",
                Status::Unverified,
                "not attempted: no portal to show a dialog",
            );
            if matches!(e, PortalError::Unavailable(_)) {
                rec.proved(unavailable_evidence(portal_info, desktop));
                rec.unknown(
                    "remediation needs a system change owned by Chief/user: install a ScreenCast backend \
                     for this compositor (e.g. xdg-desktop-portal-hyprland on Hyprland) and re-run",
                );
            }
            rec.unknown(format!("portal: {e}"));
            return None;
        }
    };
    let f = created.facts.clone();
    rec.row(
        "portal",
        "ScreenCast interface version",
        f.interface_version
            .map(|v| v.to_string())
            .unwrap_or_default(),
    );
    rec.row(
        "portal",
        "available source types",
        f.available_source_types
            .clone()
            .unwrap_or_else(|| "unknown".into()),
    );
    rec.row(
        "portal",
        "available cursor modes",
        f.available_cursor_modes
            .clone()
            .unwrap_or_else(|| "unknown".into()),
    );

    // The picker can appear during SelectSources (xdph) or Start (others):
    // one consent budget spans both, announced before either call.
    println!(
        "Waiting up to {}s for you to pick a monitor / approve the compositor screen-share dialog...",
        CONSENT_TIMEOUT.as_secs()
    );
    let selected = match created.select_monitor(CONSENT_TIMEOUT).await {
        Ok(s) => {
            rec.check(
                "P3",
                "select_sources(monitor)",
                Status::Pass,
                format!(
                    "response OK (not cancelled); monitor only, single selection, cursor={}, persist=DoNot",
                    s.facts.chosen_cursor_mode.clone().unwrap_or_default()
                ),
            );
            s
        }
        Err(e) => {
            record_consent_failure(rec, "P3", "select_sources(monitor)", &e);
            rec.check(
                "P4",
                "consent dialog -> selected stream",
                Status::Unverified,
                "not reached: SelectSources did not succeed",
            );
            return None;
        }
    };

    match selected.start().await {
        Ok(started) => {
            rec.check(
                "P4",
                "consent dialog -> selected stream",
                Status::Pass,
                format!(
                    "consent granted; {} stream(s), using first",
                    started.stream_count
                ),
            );
            Some(started)
        }
        Err(e) => {
            record_consent_failure(rec, "P4", "consent dialog -> selected stream", &e);
            None
        }
    }
}

fn record_stream(
    rec: &mut Record,
    s: &StartedSession,
    mons: &Option<Vec<sysinfo::Monitor>>,
) -> Option<sysinfo::Monitor> {
    let st = &s.stream;
    rec.row(
        "selected stream",
        "PipeWire node id",
        st.node_id.to_string(),
    );
    rec.row(
        "selected stream",
        "source type",
        st.source_type
            .clone()
            .unwrap_or_else(|| "not reported".into()),
    );
    rec.row(
        "selected stream",
        "logical size (portal)",
        st.logical_size
            .map(|(w, h)| format!("{w}x{h}"))
            .unwrap_or_else(|| "not reported".into()),
    );
    rec.row(
        "selected stream",
        "position (portal)",
        st.position
            .map(|(x, y)| format!("({x},{y})"))
            .unwrap_or_else(|| "not reported".into()),
    );
    rec.row(
        "selected stream",
        "stream id",
        st.stream_id
            .clone()
            .unwrap_or_else(|| "not reported".into()),
    );
    let complete = st.logical_size.is_some() && st.position.is_some() && st.source_type.is_some();
    rec.check(
        "P6",
        "stream metadata (node, size, pos, type)",
        if complete {
            Status::Pass
        } else {
            Status::Unverified
        },
        if complete {
            "all four reported by portal".to_string()
        } else {
            "portal omitted size/position/source type".to_string()
        },
    );

    // Map the selected stream to a compositor monitor by position + logical size.
    let matched = match (mons, st.position, st.logical_size) {
        (Some(m), Some((x, y)), Some((w, h))) => m.iter().find(|mon| {
            mon.x == x as i64 && mon.y == y as i64 && mon.logical_size() == (w as i64, h as i64)
        }),
        _ => None,
    };
    let matched = matched.cloned();
    match &matched {
        Some(m) => {
            rec.row(
                "selected stream",
                "matched monitor (hyprctl)",
                format!("{} scale {}", m.name, m.scale),
            );
            rec.check(
                "M1",
                "monitor id / scale of selected stream",
                Status::Pass,
                format!(
                    "{} scale {} (scale from compositor; portal reports logical size only)",
                    m.name, m.scale
                ),
            );
        }
        None => rec.check(
            "M1",
            "monitor id / scale of selected stream",
            Status::Unverified,
            "portal gives no scale and the stream could not be matched to a compositor monitor",
        ),
    }
    matched
}

fn unverified_capture_checks(rec: &mut Record, why: &str) {
    rec.check(
        "F1",
        "PipeWire stream connected + format negotiated",
        Status::Unverified,
        why.to_string(),
    );
    rec.check(
        "F2",
        "first real frame within 10s",
        Status::Unverified,
        why.to_string(),
    );
    rec.check(
        "F3",
        "buffer type identified (SHM, not DMA-BUF)",
        Status::Unverified,
        why.to_string(),
    );
    rec.check(
        "F4",
        "frame dimensions match portal monitor",
        Status::Unverified,
        why.to_string(),
    );
}

/// #14: consume exactly one frame from the node the user selected.
async fn capture_stage(
    rec: &mut Record,
    started: &StartedSession,
    matched: &Option<sysinfo::Monitor>,
) -> Option<pw::Captured> {
    let target = match started.open_pipe_wire_remote().await {
        Ok(t) => t,
        Err(e) => {
            unverified_capture_checks(rec, &format!("open_pipe_wire_remote failed: {e}"));
            rec.unknown(format!("portal: {e}"));
            return None;
        }
    };
    let joined =
        tokio::task::spawn_blocking(move || pw::capture_one_frame(target, pw::FIRST_FRAME_TIMEOUT))
            .await;
    let result = match joined {
        Ok(r) => r,
        Err(e) => {
            unverified_capture_checks(rec, &format!("capture thread failed: {e}"));
            return None;
        }
    };
    match result {
        Err(e) => {
            let negotiated = matches!(
                &e,
                pw::CaptureError::Timeout {
                    negotiated: Some(_),
                    ..
                } | pw::CaptureError::UnsupportedBuffer(_)
                    | pw::CaptureError::BadFrame(_)
            );
            rec.check(
                "F1",
                "PipeWire stream connected + format negotiated",
                if negotiated {
                    Status::Pass
                } else {
                    Status::Fail
                },
                e.to_string(),
            );
            rec.check(
                "F2",
                "first real frame within 10s",
                Status::Fail,
                e.to_string(),
            );
            rec.check(
                "F3",
                "buffer type identified (SHM, not DMA-BUF)",
                Status::Unverified,
                "no frame received",
            );
            rec.check(
                "F4",
                "frame dimensions match portal monitor",
                Status::Unverified,
                "no frame received",
            );
            rec.unknown(format!("pipewire: {e}"));
            None
        }
        Ok(c) => {
            let m = &c.meta;
            let sec = "first frame";
            rec.row(sec, "negotiated format", m.negotiated_format.clone());
            rec.row(sec, "dimensions", format!("{}x{}", m.width, m.height));
            rec.row(sec, "stride (bytes)", m.stride.to_string());
            rec.row(
                sec,
                "bytes in chunk",
                format!("{} (offset {})", m.chunk_size, m.chunk_offset),
            );
            rec.row(sec, "buffer type", m.buffer_type.clone());
            rec.row(
                sec,
                "framerate",
                format!("{}/{}", m.framerate.0, m.framerate.1),
            );
            rec.row(sec, "dmabuf modifier", m.modifier.to_string());
            rec.row(
                sec,
                "timestamp (pts ns)",
                m.pts_ns
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "not provided by producer".into()),
            );
            rec.row(
                sec,
                "sequence",
                m.seq
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "not provided".into()),
            );
            rec.row(
                sec,
                "empty buffers skipped first",
                m.skipped_buffers.to_string(),
            );
            rec.row(
                sec,
                "stream connect -> first frame",
                format!("{:.1} ms", m.connect_to_frame.as_secs_f64() * 1e3),
            );
            rec.row(
                sec,
                "portal confirm -> first frame",
                m.confirm_to_frame
                    .map(|d| format!("{:.1} ms", d.as_secs_f64() * 1e3))
                    .unwrap_or_else(|| "n/a".into()),
            );

            rec.check(
                "F1",
                "PipeWire stream connected + format negotiated",
                Status::Pass,
                format!("{} {}x{}", m.negotiated_format, m.width, m.height),
            );
            rec.check(
                "F2",
                "first real frame within 10s",
                Status::Pass,
                format!(
                    "{:.0} ms after connect",
                    m.connect_to_frame.as_secs_f64() * 1e3
                ),
            );
            let shm = m.buffer_type.contains("MemFd") || m.buffer_type.contains("MemPtr");
            rec.check(
                "F3",
                "buffer type identified (SHM, not DMA-BUF)",
                if shm { Status::Pass } else { Status::Fail },
                format!("{} (CPU-mapped; DMA-BUF was not requested)", m.buffer_type),
            );

            // F4: compare with the portal-reported size, accounting for scale.
            let portal_logical = started.stream.logical_size;
            let (status, why) = match (matched, portal_logical) {
                (Some(mon), _) if (m.width as i64, m.height as i64) == (mon.width, mon.height) => (
                    Status::Pass,
                    format!("frame {}x{} == physical size of {} (logical {:?} x scale {})", m.width, m.height, mon.name, portal_logical, mon.scale),
                ),
                (Some(mon), _) => (
                    Status::Fail,
                    format!("frame {}x{} != {}x{} physical size of {}", m.width, m.height, mon.width, mon.height, mon.name),
                ),
                (None, Some((w, h))) if (m.width as i64, m.height as i64) == (w as i64, h as i64) => (
                    Status::Pass,
                    format!("frame equals portal size {w}x{h} (monitor scale not determined)"),
                ),
                (None, Some((w, h))) => (
                    Status::Unverified,
                    format!("frame {}x{} differs from portal logical {w}x{h}; scale unknown so cannot confirm", m.width, m.height),
                ),
                (None, None) => (Status::Unverified, "portal reported no size".to_string()),
            };
            rec.check("F4", "frame dimensions match portal monitor", status, why);
            rec.proved(format!(
                "received 1 real frame via portal stream node {}: {} {}x{} stride {} in {} buffer",
                started.stream.node_id,
                m.negotiated_format,
                m.width,
                m.height,
                m.stride,
                m.buffer_type
            ));
            Some(c)
        }
    }
}

/// #15: convert the real frame (stride/format aware) and save a PNG under the
/// gitignored out/ directory. Pixels are never printed or displayed; only the
/// PNG header and a uniform-colour boolean are inspected.
fn png_stage(rec: &mut Record, captured: Option<&pw::Captured>) {
    const NO_FRAME: &str = "UNVERIFIED: no real frame was received, so no real-frame PNG exists";
    let Some(c) = captured else {
        rec.check(
            "N1",
            "PNG saved from a real frame",
            Status::Unverified,
            NO_FRAME,
        );
        rec.check(
            "N2",
            "PNG pixel size == frame size (no DPI scaling)",
            Status::Unverified,
            NO_FRAME,
        );
        rec.check(
            "N3",
            "frame is not a single uniform colour",
            Status::Unverified,
            NO_FRAME,
        );
        return;
    };
    let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("out");
    match frame::save_png(&c.frame, &out_dir, "a-eye-spike-04-frame.png") {
        Ok(s) => {
            rec.row("png", "path", s.path.display().to_string());
            rec.row("png", "file size", format!("{} bytes", s.bytes));
            rec.row(
                "png",
                "encode time",
                format!("{:.1} ms", s.encode_time.as_secs_f64() * 1e3),
            );
            rec.row(
                "png",
                "header",
                format!(
                    "{}x{} {} {}",
                    s.header.width, s.header.height, s.header.color_type, s.header.bit_depth
                ),
            );
            rec.row(
                "png",
                "privacy",
                "file mode 0600 in gitignored out/; contents deliberately not displayed",
            );
            rec.check(
                "N1",
                "PNG saved from a real frame",
                Status::Pass,
                format!("{} bytes written and header re-read", s.bytes),
            );
            let same = (s.header.width, s.header.height) == (c.frame.width, c.frame.height);
            rec.check(
                "N2",
                "PNG pixel size == frame size (no DPI scaling)",
                if same { Status::Pass } else { Status::Fail },
                format!(
                    "png {}x{} vs frame {}x{}",
                    s.header.width, s.header.height, c.frame.width, c.frame.height
                ),
            );
            let uniform = c.frame.is_uniform();
            rec.check(
                "N3",
                "frame is not a single uniform colour",
                if uniform { Status::Fail } else { Status::Pass },
                if uniform {
                    "every pixel identical: possible blank/black capture (heuristic)"
                } else {
                    "pixels vary (content not inspected)"
                },
            );
            rec.proved(format!(
                "real frame converted (stride {} -> tight RGB8) and saved as {}x{} PNG",
                c.frame.stride, s.header.width, s.header.height
            ));
        }
        Err(e) => {
            rec.check("N1", "PNG saved from a real frame", Status::Fail, e.clone());
            rec.check(
                "N2",
                "PNG pixel size == frame size (no DPI scaling)",
                Status::Unverified,
                "PNG not saved",
            );
            rec.check(
                "N3",
                "frame is not a single uniform colour",
                Status::Unverified,
                "PNG not saved",
            );
            rec.unknown(format!("png: {e}"));
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let t0 = Instant::now();
    let mut rec = Record::default();
    println!("A-Eye spike 04: XDG ScreenCast portal -> PipeWire frame -> PNG");
    let (sess, portal_info) = environment(&mut rec);
    let mons = record_monitors(&mut rec);

    // Known limitations / deliberate deviations, always listed.
    rec.unknown(
        "#15 asked for /tmp/a-eye-spike-04-frame.png; out/ (gitignored, mode 0600) is used deliberately \
         because captures may contain private screen content",
    );
    rec.unknown(
        "#14 'copy to CPU if DMA-BUF' is refused by design: DMA-BUF is never requested and a DmaBuf \
         buffer fails the capture instead of being imported",
    );
    rec.unknown(
        "GPUs here are AMD + NVIDIA: whether the compositor portal falls back to SHM buffers when only \
         MemPtr|MemFd are offered is unverified until a real run",
    );
    rec.unknown(
        "scale is read from the compositor (hyprctl); the portal reports logical size only",
    );

    if let Some(started) = portal_stage(&mut rec, &portal_info, &sess.xdg_current_desktop).await {
        let matched = record_stream(&mut rec, &started, &mons);
        rec.check(
            "P5",
            "no capture before started session",
            Status::Pass,
            "observed: capture began only after start() returned the selected stream; also structural (CaptureTarget is only built from a StartedSession)",
        );
        let captured = capture_stage(&mut rec, &started, &matched).await;
        // Close the portal session first: capture is finished, so revoke the
        // stream before spending time on encoding.
        let closed = started.close().await;
        png_stage(&mut rec, captured.as_ref());
        match closed {
            Ok(()) => rec.check(
                "S1",
                "portal session closed",
                Status::Pass,
                "Session.Close returned",
            ),
            Err(e) => rec.check("S1", "portal session closed", Status::Fail, e.to_string()),
        }
    } else {
        rec.check(
            "P5",
            "no capture before started session",
            Status::Unverified,
            "structural (by type design, not exercised at runtime): no session started, so the ordering was not observed",
        );
        rec.check(
            "S1",
            "portal session closed",
            Status::Unverified,
            "no started session to close",
        );
        rec.check(
            "M1",
            "monitor id / scale of selected stream",
            Status::Unverified,
            "no started portal session, so no stream was selected",
        );
        unverified_capture_checks(
            &mut rec,
            "no started portal session, so no stream to consume",
        );
        png_stage(&mut rec, None);
    }
    rec.check(
        "M2",
        "two monitors with different scales",
        Status::Unverified,
        match &mons {
            Some(m) if m.len() > 1 => format!(
                "{} monitors present with scales {:?}, but one run captures one selected monitor",
                m.len(),
                m.iter().map(|x| x.scale).collect::<Vec<_>>()
            ),
            _ => "fewer than two monitors or layout unobtainable".to_string(),
        },
    );
    rec.row(
        "run",
        "unit tests",
        "not executed by cargo run; see `cargo test` (synthetic frames, 10 pixel formats, padded strides)",
    );
    rec.row(
        "run",
        "wall time",
        format!("{:.2}s", t0.elapsed().as_secs_f64()),
    );
    print!("{}", rec.render());
}
