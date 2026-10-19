//! A-Eye spike 09: an MCP server over stdio that exposes Windows screen capture and the border
//! overlay to a coding agent.
//!
//! The host launches this process and talks JSON-RPC over its stdin/stdout. There is no network
//! listener of any kind. stdout carries protocol messages only; diagnostics go to stderr.
//!
//! Usage: spike-09-mcp-stdio
//! Capture and overlay start require scope leases. MCP lease requests auto-approve in this spike
//! without a local user prompt; this is not production consent.

// Off Windows only the tests build the portable layers; the real backend is Windows-only.
#![cfg_attr(not(windows), allow(dead_code))]

mod backend;
mod image;
mod protocol;
mod tools;
#[cfg(windows)]
mod win;

const USAGE: &str = "usage: spike-09-mcp-stdio";

#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Run,
    Help,
}

fn parse_cli(mut args: impl Iterator<Item = String>) -> Result<Cli, String> {
    match args.next().as_deref() {
        None => Ok(Cli::Run),
        Some("--help" | "-h") => Ok(Cli::Help),
        Some(other) => Err(format!("unknown argument {other}")),
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("spike 09 drives Windows capture and overlay APIs and only runs on Windows");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    use std::io;

    match parse_cli(std::env::args().skip(1)) {
        Ok(Cli::Run) => {}
        Ok(Cli::Help) => {
            println!("{USAGE}");
            return;
        }
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let config = tools::ToolConfig::default();
    let mut backend = match win::WindowsBackend::new(std::sync::Arc::clone(&config.broker)) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[aeye-mcp] startup failed: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "[aeye-mcp] serving MCP {} on stdio; scope leases required; WARNING: spike-only \
         MCP lease requests AUTO-APPROVE without local user consent",
        protocol::PROTOCOL_VERSION,
    );
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
    fn legacy_capture_gate_is_rejected() {
        assert_eq!(cli(&[]), Ok(Cli::Run));
        assert!(cli(&["--allow-capture"]).is_err());
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        assert!(cli(&["--port", "8080"]).is_err());
        assert_eq!(cli(&["--help"]), Ok(Cli::Help));
    }
}
