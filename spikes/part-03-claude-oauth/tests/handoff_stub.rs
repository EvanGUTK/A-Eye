//! Token handoff to a stub SDK-runner child process (this crate's own binary
//! re-executed with `--sdk-stub`). SYNTHETIC token only.

use spike_03_claude_oauth::handoff::{read_proc, run_handoff, run_handoff_with, StubMode, OUTRANKING_ENV, TOKEN_ENV};
use spike_03_claude_oauth::secret::Secret;
use std::path::Path;

const STUB: &str = env!("CARGO_BIN_EXE_spike-03-claude-oauth");

fn token() -> Secret {
    Secret::new(format!(
        "SYNTH-HANDOFF-{}",
        spike_03_claude_oauth::pkce::random_hex(16).unwrap()
    ))
}

#[test]
fn clean_handoff_env_only() {
    let t = token();
    let r = run_handoff(Path::new(STUB), StubMode::Clean, &t).expect("handoff");
    assert!(r.env_has_token, "token must be in child's environ: {r:?}");
    assert!(r.stub_saw_token && r.stub_fingerprint_matches, "{r:?}");
    assert!(!r.argv_has_token, "token leaked into /proc/<pid>/cmdline");
    assert!(!r.stub_self_argv_has_token);
    assert!(!r.stdout_has_token, "token leaked to stdout");
    assert!(!r.stderr_has_token, "token leaked to stderr");
    assert!(!r.disk_has_token, "token written to disk");
    assert!(r.outranking_env_present.is_empty(), "{:?}", r.outranking_env_present);
    assert!(r.env_keys.iter().any(|k| k == TOKEN_ENV));
    assert!(r.exit_ok);
    assert!(r.is_clean_handoff());
}

#[test]
fn ambient_api_keys_are_not_forwarded() {
    // Plant ambient credentials in the PARENT; the child env is allowlist-built
    // and the report lists the child's actual /proc/<pid>/environ keys.
    for k in OUTRANKING_ENV {
        std::env::set_var(k, "SYNTH-AMBIENT");
    }
    let t = token();
    let r = run_handoff(Path::new(STUB), StubMode::Clean, &t).unwrap();
    for k in OUTRANKING_ENV {
        assert!(!r.env_keys.iter().any(|e| e == k), "{k} forwarded to child");
    }
}

#[test]
fn leak_detectors_fire() {
    let t = token();
    let r = run_handoff(Path::new(STUB), StubMode::LeakStdout, &t).unwrap();
    assert!(r.stdout_has_token && !r.is_clean_handoff());
    let r = run_handoff(Path::new(STUB), StubMode::LeakStderr, &t).unwrap();
    assert!(r.stderr_has_token && !r.is_clean_handoff());
    let r = run_handoff(Path::new(STUB), StubMode::LeakFile, &t).unwrap();
    assert!(r.disk_has_token && !r.is_clean_handoff());
    let r = run_handoff(Path::new(STUB), StubMode::LeakArgv, &t).unwrap();
    assert!(r.argv_has_token && r.stub_self_argv_has_token && !r.is_clean_handoff());
}

#[test]
fn scratch_dir_is_removed_afterwards() {
    let t = token();
    let r = run_handoff(Path::new(STUB), StubMode::LeakFile, &t).unwrap();
    assert!(r.disk_has_token);
    assert!(!r.scratch.exists(), "scratch dir (holding a leaked token) must be deleted");
}

/// Which of `cmdline` / `environ` the injected reader should fail on.
fn failing_inspection_leaves_nothing_behind(fail_on: &'static str) {
    use std::sync::Mutex;
    let t = token();
    let seen: Mutex<Option<(u32, std::path::PathBuf)>> = Mutex::new(None);
    let reader = |pid: u32, file: &str| -> std::io::Result<Vec<u8>> {
        if file == fail_on {
            // Record the live child's pid and its private scratch dir (its cwd).
            let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))?;
            *seen.lock().unwrap() = Some((pid, cwd));
            return Err(std::io::Error::other("injected /proc failure"));
        }
        read_proc(pid, file)
    };
    let r = run_handoff_with(Path::new(STUB), StubMode::Clean, &t, &reader);
    let err = r.expect_err("inspection failure must surface as Err");
    assert!(err.contains("injected"), "{err}");
    let (pid, scratch) = seen.lock().unwrap().clone().expect("reader saw the child");
    // The child must already be killed AND reaped: no /proc entry (a zombie would still have one).
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "stub pid {pid} still present after run_handoff returned Err (orphan/zombie holding the token)"
    );
    assert!(!scratch.exists(), "scratch dir {scratch:?} left behind");
}

#[test]
fn failed_cmdline_inspection_kills_and_reaps_child() {
    failing_inspection_leaves_nothing_behind("cmdline");
}

#[test]
fn failed_environ_inspection_kills_and_reaps_child() {
    failing_inspection_leaves_nothing_behind("environ");
}
