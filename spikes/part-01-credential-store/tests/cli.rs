//! Black-box CLI behaviour: the binary must refuse unsafe targets *before* it
//! touches the keyring or writes anything, and must not treat flags as paths.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

fn run(args: &[&str], env_dir: Option<&PathBuf>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_spike-01-credential-store"));
    c.args(args).env_remove("AEYE_SPIKE_CRED_DIR");
    if let Some(d) = env_dir {
        c.env("AEYE_SPIKE_CRED_DIR", d);
    }
    c.output().expect("spawn binary")
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("aeye-spike01-cli-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn run_refuses_directory_with_existing_credentials_file_and_leaves_it_alone() {
    for via_env in [false, true] {
        let d = tmp(if via_env { "env" } else { "arg" });
        let sub = d.join("a-eye");
        fs::create_dir_all(&sub).unwrap();
        let file = sub.join("credentials.enc");
        fs::write(&file, b"PRECIOUS-USER-BYTES").unwrap();

        let out = if via_env {
            run(&[], Some(&d))
        } else {
            run(&[d.to_str().unwrap()], None)
        };

        assert_eq!(out.status.code(), Some(2), "must refuse with exit 2");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("refusing"), "stderr: {stderr}");
        assert!(out.stdout.is_empty(), "no record may be printed on refusal");
        assert_eq!(fs::read(&file).unwrap(), b"PRECIOUS-USER-BYTES");
        assert!(sub.is_dir());
        fs::remove_dir_all(d).unwrap();
    }
}

#[test]
fn unknown_flag_is_rejected_and_not_used_as_a_directory() {
    let out = run(&["--bogus-flag"], None);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown option"), "stderr: {stderr}");
    assert!(stderr.contains("usage:"));
    assert!(out.stdout.is_empty());
    assert!(
        !PathBuf::from("--bogus-flag").exists(),
        "flag must not become a path"
    );
}

#[test]
fn help_prints_usage_and_exits_zero_without_running() {
    let out = run(&["--help"], None);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("usage:"));
    assert!(
        !stdout.contains("ACTIVE STORAGE PATH"),
        "help must not run the spike"
    );
}
