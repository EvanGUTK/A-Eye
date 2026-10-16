//! Capability record: environment facts + a table of checks with honest status.

use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Pass,
    Fail,
    Unverified,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Unverified => "UNVERIFIED",
        }
    }
}

pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl Check {
    pub fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Check { name, status: Status::Pass, detail: detail.into() }
    }
    pub fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Check { name, status: Status::Fail, detail: detail.into() }
    }
    pub fn unverified(name: &'static str, detail: impl Into<String>) -> Self {
        Check { name, status: Status::Unverified, detail: detail.into() }
    }
    /// PASS when `ok`, otherwise FAIL with `detail_fail`.
    pub fn from_bool(name: &'static str, ok: bool, detail_pass: String, detail_fail: String) -> Self {
        if ok { Check::pass(name, detail_pass) } else { Check::fail(name, detail_fail) }
    }
}

/// Run a command with a hard timeout, return trimmed stdout of the first line.
pub fn command_first_line(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    String::from_utf8(out.stdout).ok()?.lines().next().map(|l| l.trim().to_string())
}

pub fn os_pretty_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "unknown".into())
}

pub fn kernel() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

pub fn session_summary() -> String {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    format!(
        "XDG_SESSION_TYPE={} XDG_CURRENT_DESKTOP={} WAYLAND_DISPLAY={}",
        get("XDG_SESSION_TYPE").unwrap_or_else(|| "unset".into()),
        get("XDG_CURRENT_DESKTOP").unwrap_or_else(|| "unset".into()),
        get("WAYLAND_DISPLAY").unwrap_or_else(|| "unset".into()),
    )
}

/// Resolved crate versions straight from the committed Cargo.lock (compile-time).
pub fn locked_versions(names: &[&str]) -> Vec<String> {
    const LOCK: &str = include_str!("../Cargo.lock");
    let unq = |l: &str, key: &str| {
        l.strip_prefix(key).and_then(|r| r.strip_prefix(" = \"")).and_then(|r| r.strip_suffix('"')).map(str::to_string)
    };
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for l in LOCK.lines() {
        if let Some(n) = unq(l, "name") {
            current = Some(n);
        } else if let Some(v) = unq(l, "version")
            && let Some(n) = current.take().filter(|n| names.contains(&n.as_str()))
        {
            out.push(format!("{n} {v}"));
        }
    }
    out
}

pub fn print_table(checks: &[Check]) {
    let w = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for c in checks {
        println!("  {:<10}  {:<w$}  {}", c.status.label(), c.name, c.detail, w = w);
    }
}

pub fn count(checks: &[Check], s: Status) -> usize {
    checks.iter().filter(|c| c.status == s).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_versions_reads_cargo_lock() {
        let v = locked_versions(&["keyring", "reqwest", "sha2"]);
        assert_eq!(v.len(), 3, "{v:?}");
        assert!(v.iter().any(|s| s == "keyring 4.2.0"));
    }

    #[test]
    fn command_first_line_times_out_and_handles_missing() {
        assert_eq!(command_first_line("echo", &["hi"], Duration::from_secs(2)).as_deref(), Some("hi"));
        assert!(command_first_line("definitely-not-a-binary-xyz", &[], Duration::from_secs(1)).is_none());
        let started = std::time::Instant::now();
        assert!(command_first_line("sleep", &["5"], Duration::from_millis(200)).is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
