//! Text-to-speech through the piper binary.

use std::path::Path;
use std::process::Command;

use crate::run::{ms, run_timed, tail};
use crate::text::number_after;

const MS_PER_SEC: f64 = 1000.0;

pub struct TtsRun {
    /// Process spawn to the process having exited with all output written/read.
    pub wall_ms: f64,
    /// Process spawn to the first raw PCM byte on stdout (raw mode only).
    pub first_audio_ms: Option<f64>,
    pub stdout_bytes: usize,
    pub engine_load_ms: Option<f64>,
    pub engine_infer_ms: Option<f64>,
    pub engine_audio_secs: Option<f64>,
}

fn base_command(piper: &Path, model: &Path) -> Command {
    let mut cmd = Command::new(piper);
    cmd.arg("-m").arg(model);
    // Run from the tool's own (local) directory and point at its bundled phoneme data explicitly.
    if let Some(dir) = piper.parent() {
        cmd.arg("--espeak_data").arg(dir.join("espeak-ng-data"));
        cmd.current_dir(dir);
    }
    cmd
}

/// Synthesises `text` into a WAV file at `out`. Input is fully available at spawn (stdin is
/// written and closed immediately), so `wall_ms` is input-complete to audio-file-ready plus
/// process start-up and voice load.
pub fn synth_to_file(piper: &Path, model: &Path, text: &str, out: &Path) -> Result<TtsRun, String> {
    let mut cmd = base_command(piper, model);
    cmd.arg("-f").arg(out);
    finish(cmd, text)
}

/// Synthesises `text` as raw PCM on stdout, to measure time to the first audio byte.
pub fn synth_raw(piper: &Path, model: &Path, text: &str) -> Result<TtsRun, String> {
    let mut cmd = base_command(piper, model);
    cmd.arg("--output_raw");
    finish(cmd, text)
}

fn finish(cmd: Command, text: &str) -> Result<TtsRun, String> {
    let run = run_timed(cmd, Some(text))?;
    if !run.success {
        return Err(format!(
            "piper exited with {:?}: {}",
            run.code,
            tail(&run.stderr)
        ));
    }
    Ok(TtsRun {
        wall_ms: ms(run.wall),
        first_audio_ms: run.first_stdout.map(ms),
        stdout_bytes: run.stdout.len(),
        engine_load_ms: number_after(&run.stderr, "Loaded voice in").map(|s| s * MS_PER_SEC),
        engine_infer_ms: number_after(&run.stderr, "infer=").map(|s| s * MS_PER_SEC),
        engine_audio_secs: number_after(&run.stderr, "audio="),
    })
}
