//! A-Eye spike 04 runner. Exit code 0 whenever the capability record was
//! produced, including when the portal is unavailable, consent was denied or
//! consent timed out (those are recorded as FAIL/UNVERIFIED, never as PASS).

use std::time::Instant;

use spike_04_xdg_capture::portal::{self, PortalError, StartedSession, CONSENT_TIMEOUT};
use spike_04_xdg_capture::record::{Record, Status};
use spike_04_xdg_capture::sysinfo;

fn environment(rec: &mut Record) {
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
    rec.row("session", "XDG_CURRENT_DESKTOP", sess.xdg_current_desktop.clone());
    rec.row("session", "XDG_SESSION_TYPE", sess.xdg_session_type.clone());
    rec.row("session", "WAYLAND_DISPLAY", sess.wayland_display.clone());
    rec.row("session", "compositor version", sess.compositor_version.clone());

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

async fn portal_stage(rec: &mut Record) -> Option<StartedSession> {
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
            rec.check("P1", "ScreenCast interface bound", Status::Fail, e.to_string());
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
                rec.proved(
                    "negative result: this session offers no ScreenCast portal (ashpd PortalNotFound; \
                     frontend introspection and *.portal backend files agree), so no consent dialog can appear",
                );
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
        f.interface_version.map(|v| v.to_string()).unwrap_or_default(),
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

    let selected = match created.select_monitor().await {
        Ok(s) => {
            rec.check(
                "P3",
                "select_sources(monitor)",
                Status::Pass,
                format!(
                    "monitor only, single selection, cursor={}, persist=DoNot",
                    s.facts.chosen_cursor_mode.clone().unwrap_or_default()
                ),
            );
            s
        }
        Err(e) => {
            rec.check("P3", "select_sources(monitor)", Status::Fail, e.to_string());
            rec.check(
                "P4",
                "consent dialog -> selected stream",
                Status::Unverified,
                "not attempted",
            );
            rec.unknown(format!("portal: {e}"));
            return None;
        }
    };

    println!(
        "Waiting up to {}s for you to approve the compositor screen-share dialog...",
        CONSENT_TIMEOUT.as_secs()
    );
    match selected.start(CONSENT_TIMEOUT).await {
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
        Err(PortalError::TimedOut) => {
            rec.check(
                "P4",
                "consent dialog -> selected stream",
                Status::Unverified,
                "UNVERIFIED: no consent within 90s",
            );
            rec.unknown("consent dialog timed out after 90s; no live frame was attempted");
            None
        }
        Err(PortalError::Denied) => {
            rec.check(
                "P4",
                "consent dialog -> selected stream",
                Status::Fail,
                "FAIL: user denied",
            );
            rec.unknown("user denied the consent dialog; no live frame was attempted");
            None
        }
        Err(e) => {
            rec.check(
                "P4",
                "consent dialog -> selected stream",
                Status::Fail,
                e.to_string(),
            );
            rec.unknown(format!("portal start: {e}"));
            None
        }
    }
}

fn record_stream(rec: &mut Record, s: &StartedSession, mons: &Option<Vec<sysinfo::Monitor>>) {
    let st = &s.stream;
    rec.row("selected stream", "PipeWire node id", st.node_id.to_string());
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
    match matched {
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
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let t0 = Instant::now();
    let mut rec = Record::default();
    println!("A-Eye spike 04: XDG ScreenCast portal -> PipeWire frame -> PNG");
    environment(&mut rec);
    let mons = record_monitors(&mut rec);

    if let Some(started) = portal_stage(&mut rec).await {
        record_stream(&mut rec, &started, &mons);
        rec.check(
            "P5",
            "no capture before started session",
            Status::Pass,
            "compile-time: CaptureTarget is only built from a StartedSession (see portal.rs)",
        );
        match started.close().await {
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
            Status::Pass,
            "compile-time: no CaptureTarget exists without a StartedSession, so none was attempted",
        );
        rec.check(
            "S1",
            "portal session closed",
            Status::Unverified,
            "no started session to close",
        );
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
    rec.row("run", "wall time", format!("{:.2}s", t0.elapsed().as_secs_f64()));
    print!("{}", rec.render());
}
