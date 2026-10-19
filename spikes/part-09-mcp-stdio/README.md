# Spike 09: MCP stdio server (Windows capture + overlay as tools)

A standalone Rust MCP server for a coding agent. The host launches it as a child process and
talks JSON-RPC over stdin/stdout. **There is no network listener**: no HTTP, no websocket, no
port. stdout carries MCP messages only; logs go to stderr. Windows only.

Issue: #28. Status and what is unproven: see the table in `../README.md` and the end of this file.

## Build and run

Native Windows `cargo` over the UNC path, with the target dir on a local drive (see
`../README.md`, "Platform pivot"):

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\aeye09-target"
cd \\wsl$\Ubuntu\home\usaas\projects\code\A-Eye-worktrees\mcp-bridge\spikes\part-09-mcp-stdio
cargo build
cargo test                                                          # protocol + tool logic, no desktop needed
cargo test --test live_stdio -- --ignored --nocapture --test-threads=1   # live: needs an interactive desktop
```

The live test captures the primary monitor, draws borders on every monitor for a few seconds and
injects Ctrl+Alt+Q. Run it when you are not in the middle of something.

Wire it into a host (the user opts into capture by passing `--allow-capture`):

```
claude mcp add aeye -- C:\path\to\spike-09-mcp-stdio.exe --allow-capture
```

## Tools

Names follow `docs/architecture/CONTRACTS.md` where it has one (`aeye_` prefix).

| Tool | Does |
|---|---|
| `aeye_list_monitors` | id, device, position/size in physical px, DPI scale, primary. No pixels. |
| `aeye_capture_monitor(monitor_id, max_dimension?)` | One PNG `image` content item plus size/scale metadata. Longest side capped at `max_dimension` (default 1568, 64..8192). Cursor not drawn. Refused unless started with `--allow-capture`. |
| `aeye_start_overlay(monitor_id, color?, duration_secs?)` | Orange (default) click-through border, returns a handle. Default 60 s, max 600 s. One per monitor. |
| `aeye_stop_overlay(handle)` | Removes it; reports whether this call, an expiry, the hotkey or an external close ended it. |
| `aeye_get_cursor_position` | Physical virtual-desktop px, plus monitor id and position relative to it. |

Monitor ids are `hm-<hex HMONITOR>`: valid for the server process only (the contract calls ids
instance-scoped). They are looked up in a fresh enumeration on every call, so a stale id is
rejected rather than silently pointing at something else.

## Decisions

- **Capture is opt-in.** Scope leases (spike 10) do not exist, so `aeye_capture_monitor` returns an
  error unless the user started the server with `--allow-capture`. This is a coarse stand-in, not a
  lease: once enabled, any tool call can capture any whole monitor.
- **Local stop control.** Every overlay is bounded in time and one process-wide Ctrl+Alt+Q hotkey
  removes all of them. If the hotkey cannot be registered (for example another A-Eye process holds
  it) `aeye_start_overlay` fails instead of showing an overlay nobody can dismiss. Consequence: two
  servers cannot show overlays at the same time.
- **Hand-written protocol layer, no MCP SDK.** Five tools need only initialize, ping, tools/list and
  tools/call, and it avoids an async runtime. It was written against the 2025-11-25 lifecycle,
  transport and tools pages; it answers every client with that version. The official Rust SDK
  (`rmcp`) is the candidate when the surface grows; that is a graduation decision.
- **Code reuse is a copy, on purpose.** Spikes 17 and 18 are bin-only crates with private,
  print-as-you-go code, and `../README.md` says "no shared crates until a spike graduates". Making
  them libraries would mean editing parts outside this task. So `src/win/monitors.rs`,
  `capture.rs` and `overlay.rs` are adapted copies (each says so in its header). When the spikes
  graduate, fold the copies into one capture crate. Deviations from the originals: capture closes
  its session on every error path; overlays run on one thread each and are stopped through an
  atomic flag (no cross-thread HWND use); the hotkey is process-wide.
- **Screen content is untrusted.** The tool description and result metadata say so; nothing the
  server returns is an instruction.

## Evidence (this Windows desktop, 4 monitors incl. one portrait, all at 100%)

Proven:

- 27 unit tests: framing, lifecycle, error codes, argument validation, the capture gate, tool
  schemas, downscale and PNG round trip (also pass on the WSL toolchain).
- Live test through the real binary over stdio, 28 checks: handshake, `tools/list`, monitor list,
  cursor, full-size capture equals the monitor size and is not blank, downscale, border visible in
  a capture of the monitor at the exact colour on **all 4 monitors** (including the portrait and
  both far corners), custom colour, border gone after stop, one overlay per monitor, double stop
  is an error, expiry, an injected Ctrl+Alt+Q removes the overlay, a second server refuses to show
  an overlay while the hotkey is held, the process exits when stdin closes and its overlay goes
  with it, and the hotkey is free for the next process. Zero TCP/UDP endpoints owned by the server
  process (one snapshot taken mid-session via `Get-NetTCPConnection`/`Get-NetUDPEndpoint`).
- Real host: Claude Code 2.1.303, started with a throwaway `--mcp-config`, completed the handshake,
  listed the tools and called `aeye_list_monitors` (answer matched the live test: 4 monitors,
  primary 2560x1440).
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` clean.

**Not proven:**

- An image result rendered by a real host: the host test deliberately used no pixel-returning
  tool. Host limits on image size are unknown; the 8 MiB PNG cap and 1568 px default are guesses.
- Codex CLI as a host. Any protocol version other than 2025-11-25.
- Mixed-DPI monitors (all four are 100%), HDR, monitors added or removed mid-session.
- Borders and captures were checked by pixel value, not looked at by eye.
- Overlay stop paths other than call, expiry, hotkey and process exit (for example the console
  Ctrl+C handler from spike 18 is not carried over; an external `WM_CLOSE` is coded as
  `closed_externally` but untested).
- Scope leases, approvals and any per-call consent: none exist here.
- Behaviour when the machine is locked or the desktop is not interactive.
