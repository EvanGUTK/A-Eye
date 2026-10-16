//! Capability record: sections of key/value facts plus a PASS/FAIL/UNVERIFIED
//! check table. A check is PASS only when it was actually exercised and
//! observed; anything not run is UNVERIFIED, never PASS.

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Unverified,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Unverified => "UNVERIFIED",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub id: &'static str,
    pub name: &'static str,
    pub status: Status,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Section {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct Record {
    pub sections: Vec<Section>,
    pub checks: Vec<Check>,
    pub proved: Vec<String>,
    pub unknown: Vec<String>,
}

impl Record {
    pub fn section(&mut self, title: &str) -> &mut Section {
        if let Some(i) = self.sections.iter().position(|s| s.title == title) {
            return &mut self.sections[i];
        }
        self.sections.push(Section {
            title: title.to_string(),
            rows: Vec::new(),
        });
        self.sections.last_mut().expect("just pushed")
    }

    pub fn row(&mut self, section: &str, key: &str, value: impl Into<String>) {
        self.section(section)
            .rows
            .push((key.to_string(), value.into()));
    }

    pub fn check(
        &mut self,
        id: &'static str,
        name: &'static str,
        status: Status,
        reason: impl Into<String>,
    ) {
        self.checks.push(Check {
            id,
            name,
            status,
            reason: reason.into(),
        });
    }

    pub fn status_of(&self, id: &str) -> Option<Status> {
        self.checks.iter().find(|c| c.id == id).map(|c| c.status)
    }

    pub fn proved(&mut self, s: impl Into<String>) {
        self.proved.push(s.into());
    }

    pub fn unknown(&mut self, s: impl Into<String>) {
        self.unknown.push(s.into());
    }

    /// Checks that must all PASS for the spike to claim a proven live capture.
    pub const REQUIRED_FOR_LIVE: [&'static str; 10] =
        ["P1", "P2", "P3", "P4", "F1", "F2", "F3", "F4", "N1", "N2"];

    /// One line that cannot be mistaken for success: computed from the rows,
    /// independent of the process exit code (0 whenever a record is produced).
    pub fn spike_status(&self) -> String {
        for id in Self::REQUIRED_FOR_LIVE {
            match self.checks.iter().find(|c| c.id == id) {
                Some(c) if c.status == Status::Pass => {}
                Some(c) => {
                    return format!(
                        "SPIKE STATUS: live capture NOT proven (reason: {} {} is {}: {})",
                        c.id,
                        c.name,
                        c.status.label(),
                        c.reason
                    )
                }
                None => {
                    return format!(
                    "SPIKE STATUS: live capture NOT proven (reason: check {id} was not recorded)"
                )
                }
            }
        }
        if self.status_of("N3") == Some(Status::Fail) {
            return "SPIKE STATUS: live capture NOT proven (reason: N3 frame is one uniform colour, possible blank capture)".to_string();
        }
        "SPIKE STATUS: live capture proven (portal consent, PipeWire frame and PNG all observed)"
            .to_string()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "================ A-Eye spike 04 capability record ================"
        );
        for s in &self.sections {
            let _ = writeln!(out, "\n[{}]", s.title);
            let w = s.rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
            for (k, v) in &s.rows {
                let _ = writeln!(out, "  {k:<w$}  {v}");
            }
        }
        let _ = writeln!(out, "\n[checks]");
        let idw = self.checks.iter().map(|c| c.id.len()).max().unwrap_or(0);
        let nw = self.checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
        for c in &self.checks {
            let _ = writeln!(
                out,
                "  {:<idw$}  {:<nw$}  {:<10}  {}",
                c.id,
                c.name,
                c.status.label(),
                c.reason
            );
        }
        let count = |s: Status| self.checks.iter().filter(|c| c.status == s).count();
        let _ = writeln!(
            out,
            "  totals: {} PASS, {} FAIL, {} UNVERIFIED",
            count(Status::Pass),
            count(Status::Fail),
            count(Status::Unverified)
        );
        let _ = writeln!(out, "\n[what was proved]");
        if self.proved.is_empty() {
            let _ = writeln!(out, "  (nothing beyond environment probing)");
        }
        for p in &self.proved {
            let _ = writeln!(out, "  - {p}");
        }
        let _ = writeln!(out, "\n[what failed or is unknown]");
        if self.unknown.is_empty() {
            let _ = writeln!(out, "  (none)");
        }
        for p in &self.unknown {
            let _ = writeln!(out, "  - {p}");
        }
        let _ = writeln!(out, "\n{}", self.spike_status());
        let _ = writeln!(
            out,
            "==================================================================="
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spike_status_is_not_proven_until_every_required_check_passes() {
        let mut r = Record::default();
        assert!(r.spike_status().contains("NOT proven"));
        for id in Record::REQUIRED_FOR_LIVE {
            r.check(id, "x", Status::Pass, "ok");
        }
        assert!(r.spike_status().contains("live capture proven"));
        r.check("N3", "uniform", Status::Fail, "blank");
        assert!(r.spike_status().contains("NOT proven"));
        let mut r2 = Record::default();
        for id in Record::REQUIRED_FOR_LIVE {
            let st = if id == "F2" {
                Status::Unverified
            } else {
                Status::Pass
            };
            r2.check(id, "x", st, "why");
        }
        let line = r2.spike_status();
        assert!(line.contains("NOT proven") && line.contains("F2") && line.contains("UNVERIFIED"));
    }

    #[test]
    fn render_lists_checks_and_totals() {
        let mut r = Record::default();
        r.row("env", "os", "Arch");
        r.check("A", "first", Status::Pass, "ok");
        r.check("B", "second", Status::Unverified, "not run");
        r.check("C", "third", Status::Fail, "boom");
        r.proved("a thing");
        r.unknown("another thing");
        let text = r.render();
        assert!(text.contains("1 PASS, 1 FAIL, 1 UNVERIFIED"));
        assert!(text.contains("UNVERIFIED  not run"));
        assert!(text.contains("- a thing"));
        assert!(text.contains("SPIKE STATUS: live capture NOT proven"));
        assert_eq!(r.status_of("B"), Some(Status::Unverified));
        assert_eq!(r.status_of("Z"), None);
    }
}
