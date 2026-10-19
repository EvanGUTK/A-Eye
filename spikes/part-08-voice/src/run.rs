//! Runs an external tool once and measures it from spawn to exit.

use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const READ_CHUNK_BYTES: usize = 8192;
const ERROR_TAIL_CHARS: usize = 400;

pub struct Run {
    /// Spawn to process exit with stdout and stderr fully drained.
    pub wall: Duration,
    /// Spawn to the first byte arriving on stdout, if any did.
    pub first_stdout: Option<Duration>,
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

type StdoutCapture = io::Result<(Vec<u8>, Option<Instant>)>;

/// Spawns `cmd`, optionally feeds `stdin_text` and closes stdin, and returns timings plus output.
///
/// The clock starts immediately before spawn, so process start-up (and any model load the tool
/// does itself) is included. Callers decide what that means for "end of input".
pub fn run_timed(mut cmd: Command, stdin_text: Option<&str>) -> Result<Run, String> {
    cmd.stdin(if stdin_text.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {:?}: {e}", cmd.get_program()))?;
    let mut stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut stderr = child.stderr.take().ok_or("no stderr pipe")?;

    // Drain both pipes concurrently so a chatty tool can never block on a full pipe.
    let out_thread = thread::spawn(move || -> StdoutCapture {
        let mut buf = Vec::new();
        let mut chunk = [0u8; READ_CHUNK_BYTES];
        let mut first = None;
        loop {
            let n = stdout.read(&mut chunk)?;
            if n == 0 {
                return Ok((buf, first));
            }
            first.get_or_insert_with(Instant::now);
            buf.extend_from_slice(&chunk[..n]);
        }
    });
    let err_thread = thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf)?;
        Ok(buf)
    });

    if let Some(text) = stdin_text {
        let mut stdin = child.stdin.take().ok_or("no stdin pipe")?;
        if let Err(e) = stdin.write_all(text.as_bytes()) {
            // Best effort: the write error is the failure we report.
            let _ = child.kill();
            return Err(format!("write stdin: {e}"));
        }
        // stdin drops here, which is the end-of-input signal for the tool.
    }

    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    let (stdout, first) = out_thread
        .join()
        .map_err(|_| "stdout reader thread panicked".to_string())?
        .map_err(|e| format!("read stdout: {e}"))?;
    let stderr = err_thread
        .join()
        .map_err(|_| "stderr reader thread panicked".to_string())?
        .map_err(|e| format!("read stderr: {e}"))?;
    let wall = start.elapsed();

    Ok(Run {
        wall,
        first_stdout: first.map(|t| t.duration_since(start)),
        success: status.success(),
        code: status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Last few hundred characters of a tool's stderr, for error messages.
pub fn tail(s: &str) -> String {
    let trimmed = s.trim();
    let skip = trimmed.chars().count().saturating_sub(ERROR_TAIL_CHARS);
    trimmed.chars().skip(skip).collect()
}
