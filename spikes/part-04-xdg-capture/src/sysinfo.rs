//! Read-only environment probes for the capability record.
//!
//! Everything here shells out to read-only tools (`busctl introspect` on the
//! already-running frontend, `pacman -Q`, `hyprctl monitors/version`) or reads
//! files. Nothing here starts a service, takes a screenshot or touches the
//! portal's ScreenCast interface. Every probe degrades to "unknown"/`None`.

use std::fs;
use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

#[derive(Debug, Clone)]
pub struct OsInfo {
    pub pretty_name: String,
    pub kernel: String,
    pub gpu_drivers: Vec<String>,
}

pub fn os_info() -> OsInfo {
    let pretty_name = fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    OsInfo {
        pretty_name,
        kernel,
        gpu_drivers: gpu_drivers(),
    }
}

/// Kernel DRM driver per card, e.g. `card1=amdgpu` (symlink basename only).
fn gpu_drivers() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir("/sys/class/drm") {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let drv = fs::read_link(e.path().join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));
            if let Some(d) = drv {
                out.push(format!("{name}={d}"));
            }
        }
    }
    out.sort();
    out
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub xdg_current_desktop: String,
    pub xdg_session_type: String,
    pub wayland_display: String,
    pub compositor_version: String,
}

pub fn session_info() -> SessionInfo {
    let desktop = env_or("XDG_CURRENT_DESKTOP", "unset");
    let compositor_version = if desktop.to_lowercase().contains("hyprland") {
        run("hyprctl", &["version"])
            .and_then(|s| s.lines().next().map(|l| l.to_string()))
            .unwrap_or_else(|| "unknown (hyprctl unavailable)".to_string())
    } else {
        "unknown (no version probe for this compositor)".to_string()
    };
    SessionInfo {
        xdg_current_desktop: desktop,
        xdg_session_type: env_or("XDG_SESSION_TYPE", "unset"),
        wayland_display: env_or("WAYLAND_DISPLAY", "unset"),
        compositor_version,
    }
}

/// One `*.portal` backend description file.
#[derive(Debug, Clone)]
pub struct PortalFile {
    pub file: String,
    pub dbus_name: String,
    pub use_in: Vec<String>,
    pub has_screencast: bool,
}

#[derive(Debug, Clone)]
pub struct PortalInfo {
    pub frontend_package: String,
    /// Whether the already-running frontend exports `org.freedesktop.portal.ScreenCast`.
    pub frontend_exports_screencast: Option<bool>,
    pub backends: Vec<PortalFile>,
    /// Running `xdg-desktop-portal*` processes (comm names).
    pub running_portal_processes: Vec<String>,
    /// Backends declaring ScreenCast whose `UseIn` matches the current desktop.
    pub screencast_backend_for_desktop: Option<(PortalFile, String)>,
}

pub fn portal_info(current_desktop: &str) -> PortalInfo {
    let frontend_package = run("pacman", &["-Q", "xdg-desktop-portal"])
        .unwrap_or_else(|| "xdg-desktop-portal (version unknown: pacman unavailable)".to_string());

    let frontend_exports_screencast = run(
        "busctl",
        &[
            "--user",
            "introspect",
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "--no-pager",
        ],
    )
    .map(|s| s.contains("org.freedesktop.portal.ScreenCast"));

    let mut backends = Vec::new();
    if let Ok(rd) = fs::read_dir("/usr/share/xdg-desktop-portal/portals") {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("portal") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let get = |key: &str| {
                text.lines()
                    .find_map(|l| l.strip_prefix(&format!("{key}=")))
                    .unwrap_or("")
                    .to_string()
            };
            let use_in = get("UseIn")
                .split(';')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            backends.push(PortalFile {
                file: e.file_name().to_string_lossy().to_string(),
                dbus_name: get("DBusName"),
                use_in,
                has_screencast: get("Interfaces")
                    .contains("org.freedesktop.impl.portal.ScreenCast"),
            });
        }
    }
    backends.sort_by(|a, b| a.file.cmp(&b.file));

    let running_portal_processes = run("ps", &["-eo", "args="])
        .map(|s| portal_process_names(&s))
        .unwrap_or_default();

    let desktops: Vec<String> = current_desktop
        .split(':')
        .map(|s| s.to_lowercase())
        .collect();
    let screencast_backend_for_desktop = backends
        .iter()
        .find(|b| {
            b.has_screencast
                && b.use_in
                    .iter()
                    .any(|u| desktops.contains(&u.to_lowercase()))
        })
        .map(|b| {
            let pkg_name = format!("xdg-desktop-portal-{}", b.file.trim_end_matches(".portal"));
            let ver = run("pacman", &["-Q", &pkg_name])
                .unwrap_or_else(|| format!("{pkg_name} (version unknown)"));
            (b.clone(), ver)
        });

    PortalInfo {
        frontend_package,
        frontend_exports_screencast,
        backends,
        running_portal_processes,
        screencast_backend_for_desktop,
    }
}

/// Full executable names of running `xdg-desktop-portal*` processes, from
/// `ps -eo args=` output (comm is truncated to 15 chars, args is not).
pub fn portal_process_names(ps_args_output: &str) -> Vec<String> {
    let mut names: Vec<String> = ps_args_output
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter_map(|exe| exe.rsplit('/').next())
        .filter(|n| n.starts_with("xdg-desktop-portal"))
        .map(|n| n.to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[derive(Debug, Clone)]
pub struct Monitor {
    pub name: String,
    pub width: i64,
    pub height: i64,
    pub x: i64,
    pub y: i64,
    pub scale: f64,
}

impl Monitor {
    /// Logical (compositor-space) size = physical / scale, rounded.
    pub fn logical_size(&self) -> (i64, i64) {
        (
            (self.width as f64 / self.scale).round() as i64,
            (self.height as f64 / self.scale).round() as i64,
        )
    }
}

/// Monitor layout as reported by the compositor (Hyprland only in this spike).
/// `None` means the layout could not be obtained (not an error).
pub fn monitors() -> Option<Vec<Monitor>> {
    let json = run("hyprctl", &["monitors", "-j"])?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let arr = v.as_array()?;
    let mut out = Vec::new();
    for m in arr {
        out.push(Monitor {
            name: m.get("name")?.as_str()?.to_string(),
            width: m.get("width")?.as_i64()?,
            height: m.get("height")?.as_i64()?,
            x: m.get("x")?.as_i64()?,
            y: m.get("y")?.as_i64()?,
            scale: m.get("scale")?.as_f64()?,
        });
    }
    Some(out)
}

/// (runtime libpipewire version, build-time libpipewire version).
pub fn pipewire_versions() -> (String, String) {
    pipewire::init();
    // SAFETY: pw_get_library_version returns a static NUL-terminated string.
    let runtime = unsafe {
        let p = pipewire::sys::pw_get_library_version();
        if p.is_null() {
            "unknown".to_string()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().to_string()
        }
    };
    (runtime, env!("SPIKE_PW_BUILD_VERSION").to_string())
}

/// Crate versions pinned by the committed Cargo.lock (embedded at build time).
pub fn locked_version(name: &str) -> String {
    const LOCK: &str = include_str!("../Cargo.lock");
    let mut lines = LOCK.lines();
    while let Some(l) = lines.next() {
        if l == format!("name = \"{name}\"") {
            if let Some(v) = lines.next().and_then(|v| v.strip_prefix("version = ")) {
                return v.trim_matches('"').to_string();
            }
        }
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_version_finds_pinned_crates() {
        assert_eq!(locked_version("ashpd"), "0.13.13");
        assert_eq!(locked_version("pipewire"), "0.10.1");
        assert_eq!(locked_version("png"), "0.18.1");
        assert_eq!(locked_version("definitely-not-a-crate"), "unknown");
    }

    #[test]
    fn portal_process_names_are_not_truncated() {
        let ps = "/usr/lib/xdg-desktop-portal\n/usr/lib/xdg-desktop-portal-gtk --verbose\nbash\n/usr/bin/foo-xdg-desktop-portal\n";
        assert_eq!(
            portal_process_names(ps),
            vec!["xdg-desktop-portal", "xdg-desktop-portal-gtk"]
        );
    }

    #[test]
    fn logical_size_divides_by_scale() {
        let m = Monitor {
            name: "X".into(),
            width: 2560,
            height: 1440,
            x: 0,
            y: 0,
            scale: 1.6,
        };
        assert_eq!(m.logical_size(), (1600, 900));
    }
}
