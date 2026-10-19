//! A-Eye spike 09: an MCP server over stdio that exposes Windows screen capture and the border
//! overlay to a coding agent.
//!
//! The host launches this process and talks JSON-RPC over its stdin/stdout. There is no network
//! listener of any kind. stdout carries protocol messages only; diagnostics go to stderr.
//!
//! Usage: spike-09-mcp-stdio [--allow-capture]
//!   --allow-capture   let aeye_capture_monitor return pixels. Off by default: scope leases
//!                     (spike 10) do not exist yet, so the user opts in when wiring up the server.

// Off Windows only the tests build the portable layers; the real backend is Windows-only.
#![cfg_attr(not(windows), allow(dead_code))]

mod backend;
mod image;
mod protocol;
mod tools;
#[cfg(windows)]
mod win;

const USAGE: &str = "usage: spike-09-mcp-stdio [--allow-capture]";

#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Run { allow_capture: bool },
    Help,
}

fn parse_cli(args: impl Iterator<Item = String>) -> Result<Cli, String> {
    let mut allow_capture = false;
    for arg in args {
        match arg.as_str() {
            "--allow-capture" => allow_capture = true,
            "--help" | "-h" => return Ok(Cli::Help),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Cli::Run { allow_capture })
}

#[cfg(not(windows))]
fn main() {
    eprintln!("spike 09 drives Windows capture and overlay APIs and only runs on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    use std::io;

    let allow_capture = match parse_cli(std::env::args().skip(1)) {
        Ok(Cli::Run { allow_capture }) => allow_capture,
        Ok(Cli::Help) => {
            println!("{USAGE}");
            return;
        }
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let mut backend = match win::WindowsBackend::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[aeye-mcp] startup failed: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "[aeye-mcp] serving MCP {} on stdio; capture {}",
        protocol::PROTOCOL_VERSION,
        if allow_capture {
            "ENABLED (--allow-capture)"
        } else {
            "disabled (start with --allow-capture to enable)"
        }
    );
    let config = tools::ToolConfig { allow_capture };
    if let Err(e) = protocol::serve(
        io::stdin().lock(),
        io::stdout().lock(),
        &mut backend,
        config,
    ) {
        eprintln!("[aeye-mcp] stdio error: {e}");
        std::process::exit(1);
    }
    eprintln!("[aeye-mcp] stdin closed; overlays removed; exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> Result<Cli, String> {
        parse_cli(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn capture_is_off_unless_asked() {
        assert_eq!(
            cli(&[]),
            Ok(Cli::Run {
                allow_capture: false
            })
        );
        assert_eq!(
            cli(&["--allow-capture"]),
            Ok(Cli::Run {
                allow_capture: true
            })
        );
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        assert!(cli(&["--port", "8080"]).is_err());
        assert_eq!(cli(&["--help"]), Ok(Cli::Help));
    }
}
