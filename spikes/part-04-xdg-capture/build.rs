//! Records build-time facts for the capability record: the rustc version and
//! the libpipewire version that `pipewire-sys` was compiled against
//! (via the same `pkg-config` lookup).

use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> String {
    match Command::new(cmd).args(args).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    println!(
        "cargo:rustc-env=SPIKE_RUSTC_VERSION={}",
        run(&rustc, &["--version"])
    );
    println!(
        "cargo:rustc-env=SPIKE_PW_BUILD_VERSION={}",
        run("pkg-config", &["--modversion", "libpipewire-0.3"])
    );
    println!("cargo:rerun-if-changed=build.rs");
}
