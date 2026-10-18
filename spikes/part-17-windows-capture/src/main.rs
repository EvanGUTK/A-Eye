//! A-Eye spike 17: Windows.Graphics.Capture -> one monitor frame -> PNG.
//!
//! Prints a capability record and writes evidence to the gitignored `out/`.
//! Exit code is 0 whenever a record was produced, including when a check
//! FAILs or stays UNVERIFIED; read the `SPIKE STATUS` line, not the exit code.
//!
//! Usage: spike-17-windows-capture [--monitor <index>] [--all]
//!   default: the primary monitor only; `--all` captures every monitor (M2 evidence).
//! Pixels are never printed or displayed; the PNG is written to `out/` only.

#[cfg(not(windows))]
fn main() {
    eprintln!("spike 17 captures via Windows.Graphics.Capture and only builds on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::run();
}
