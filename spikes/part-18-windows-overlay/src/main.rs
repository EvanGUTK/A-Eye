//! A-Eye spike 18: Win32 layered, click-through, non-activating, topmost
//! orange border overlay.
//!
//! Prints a capability record and writes evidence to the gitignored `out/`.
//! Exit code is 0 whenever a record was produced, including when a check
//! FAILs or stays UNVERIFIED; read the `SPIKE STATUS` line, not the exit code.
//!
//! Usage: spike-18-windows-overlay [--region x,y,w,h] [--hold-secs N]
//!                                 [--control] [--selftest-hotkey]
//!   default: border around the primary monitor, shown for 3 s, then closed.
//!   Stop early with Ctrl+Alt+Q (global hotkey, needs no focus) or Ctrl+C.

#[cfg(not(windows))]
fn main() {
    eprintln!("spike 18 uses Win32 layered windows and only builds on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::run();
}
