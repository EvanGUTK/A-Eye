//! Speech-to-text through the whisper.cpp `whisper-cli` binary.

use std::path::Path;
use std::process::Command;

use crate::run::{ms, run_timed, tail};
use crate::text::number_after;

const THREADS: &str = "4";
const LANGUAGE: &str = "en";
/// Recorded in the capability record so the numbers can be reproduced.
pub const ARGS_NOTE: &str = "-t 4 -nt -l en (beam 5 / best-of 5 defaults, temperature fallback on)";

pub struct SttRun {
    pub transcript: String,
    /// Process spawn to transcript fully read: model load included.
    pub wall_ms: f64,
    pub engine_load_ms: Option<f64>,
    pub engine_total_ms: Option<f64>,
}

pub fn transcribe(whisper_cli: &Path, model: &Path, wav: &Path) -> Result<SttRun, String> {
    let mut cmd = Command::new(whisper_cli);
    cmd.arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .args(["-t", THREADS, "-nt", "-l", LANGUAGE]);
    // Run from the tool's own (local) directory, not from a possibly-UNC working directory.
    if let Some(dir) = whisper_cli.parent() {
        cmd.current_dir(dir);
    }
    let run = run_timed(cmd, None)?;
    if !run.success {
        return Err(format!(
            "whisper-cli exited with {:?}: {}",
            run.code,
            tail(&run.stderr)
        ));
    }
    Ok(SttRun {
        transcript: String::from_utf8_lossy(&run.stdout).trim().to_string(),
        wall_ms: ms(run.wall),
        engine_load_ms: timing_ms(&run.stderr, "load time ="),
        engine_total_ms: timing_ms(&run.stderr, "total time ="),
    })
}

/// Reads a `whisper_print_timings:` line such as `load time =   115.85 ms` from stderr.
fn timing_ms(stderr: &str, marker: &str) -> Option<f64> {
    stderr
        .lines()
        .find(|l| l.contains("whisper_print_timings:") && l.contains(marker))
        .and_then(|l| number_after(l, marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STDERR: &str = "\
whisper_print_timings:     load time =   115.85 ms
whisper_print_timings:      mel time =     2.97 ms
whisper_print_timings:   encode time =   510.95 ms /     1 runs (   510.95 ms per run)
whisper_print_timings:    total time =   753.38 ms
";

    #[test]
    fn parses_engine_timings() {
        assert_eq!(timing_ms(STDERR, "load time ="), Some(115.85));
        assert_eq!(timing_ms(STDERR, "total time ="), Some(753.38));
    }

    #[test]
    fn missing_timing_is_none_not_zero() {
        assert_eq!(timing_ms("main: processing audio", "total time ="), None);
    }
}
