# Spike 09: MCP stdio server (Windows capture + overlay as tools)

A standalone Rust MCP server for a coding agent. The host launches it as a child process and
talks JSON-RPC over stdin/stdout. **There is no network listener**: no HTTP, no websocket, no
port. stdout carries MCP messages only; logs go to stderr. Windows only.

Issues: #28 (original spike), #46 (lease integration). Status and what is unproven: see the table in `../README.md` and the end of this file.

## Build and run

Native Windows `cargo` over the UNC path, with the target dir on a local drive (see
`../README.md`, "Platform pivot"):

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\aeye09-target"
cd \\wsl$\Ubuntu\home\usaas\projects\code\A-Eye-worktrees\scope-leases-wire\spikes\part-09-mcp-stdio
cargo build
cargo test                                                          # protocol + tool logic, no desktop needed
cargo test --test live_stdio -- --ignored --nocapture --test-threads=1   # live: needs an interactive desktop
```

The live test captures the primary monitor, draws borders on every monitor for a few seconds and
injects Ctrl+Alt+Q. Run it when you are not in the middle of something.

Wire it into a host:

**Spike-only consent gap:** every `aeye_request_watch` call is auto-approved without a local user
prompt. Starting this server lets the MCP caller request capture/overlay leases on any enumerated
monitor. This exercises lease enforcement, not genuine user consent. The startup log, tool
instructions, tool descriptions and status/grant results disclose this. Do not treat it as a
production approval path. `--allow-capture` is removed and rejected; there is only the lease gate.

```
claude mcp add aeye -- C:\path\to\spike-09-mcp-stdio.exe
```

## Tools

Names follow `docs/architecture/CONTRACTS.md` where it has one (`aeye_` prefix).

| Tool | Does |
|---|---|
| `aeye_status()` | Current epoch, active lease count, `approval_mode: "spike_auto_approve"`, local stop shortcut. No pixels. |
| `aeye_request_watch(monitor_id, actions, ttl_secs, observed_epoch)` | Simulates local approval and grants one whole-monitor lease. `actions` is a unique nonempty list of `"capture"` and/or `"overlay"`; TTL 1..600 s. Returns `lease_id`, `epoch`, requested scope/actions/TTL and the simulated approval mode. Fails if the epoch moved or Ctrl+Alt+Q cannot be registered. |
| `aeye_list_monitors` | id, device, position/size in physical px, DPI scale, primary. No pixels. |
| `aeye_capture_monitor(monitor_id, lease_id, max_dimension?)` | One PNG `image` content item plus size/scale metadata. Longest side capped at `max_dimension` (default 1568, 64..8192). Cursor not drawn. Requires Capture permission on this monitor; includes `capture_epoch` in metadata. |
| `aeye_start_overlay(monitor_id, lease_id, color?, duration_secs?)` | Orange (default) click-through border, returns a handle. Default 60 s, max 600 s. One per monitor. Requires Overlay permission; also stops at lease expiry. `maximum_duration_secs` is the requested upper bound, not the remaining lease TTL. |
| `aeye_stop_overlay(handle)` | Removes it; reports whether this call, an expiry, the hotkey or an external close ended it. |
| `aeye_get_cursor_position` | Physical virtual-desktop px, plus monitor id and position relative to it. |

Monitor ids are `hm-<hex HMONITOR>`: valid for the server process only (the contract calls ids
instance-scoped). They are looked up in a fresh enumeration on every call, so a stale id is
rejected rather than silently pointing at something else.

## Decisions

- **One shared broker, two action gates.** Part 10 is a local MIT-licensed path dependency (no new
  external dependency). The request thread, hotkey thread and overlay workers share the same
  `Arc<LeaseBroker>`. Capture and overlay start call `check` before backend work and `verify` after
  result preparation, immediately before returning. PNG/base64 encoding is covered by the final
  verification. A failed overlay verification stops its newly created handle and returns an error.
  Capture metadata and overlay results carry the authorized `capture_epoch`.
- **Denials are MCP tool errors**, with `isError: true`, text starting with the broker's contract
  code, and no image or successful handle: `NO_ACTIVE_SCOPE` for absent/malformed/unknown/expired
  lease ids or an action not granted, `SCOPE_MISMATCH` for another monitor, `STALE_EPOCH` after a
  global stop. A stale `observed_epoch` also returns `STALE_EPOCH`. No automatic retry or renewal.
  As in part 10, grants prune dead records, so a previously stale id may later say `NO_ACTIVE_SCOPE`.
- **Explicit spike exception to part 10's design.** `aeye_status` exposes the epoch and
  `aeye_request_watch` calls `grant` after validating scope/actions/TTL and preparing the native stop.
  This deliberately simulates the missing local approval UI, as required for this integration
  spike. Replace it with a native approval control before product use; an agent request is not
  actual local consent. `revoke_all` remains native-only, not an MCP tool.
- **Local stop control.** Before the first grant (including capture-only), register one process-wide
  Ctrl+Alt+Q hotkey. If unavailable, no lease is issued. The handler revokes all leases before
  removing overlays, independently of the request thread. The registration stays until shutdown,
  even if no overlays exist. An exiting hotkey loop revokes leases as well. Overlay workers poll
  authorization every 10 ms, so an overlay cannot intentionally run for its full requested duration
  after lease expiry/revocation. `aeye_stop_overlay` stays ungated so cleanup always remains possible;
  it removes a border without revoking other capture/overlay permissions. EOF/transport failure
  revokes leases, removes overlays and releases the hotkey.
- **Hand-written protocol layer, no MCP SDK.** Seven tools need only initialize, ping, tools/list and
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

## Lease flow over MCP

After `initialize`, call these tools in order (use a real id from `aeye_list_monitors`):

```json
{"name":"aeye_status","arguments":{}}
{"name":"aeye_request_watch","arguments":{"monitor_id":"hm-example","actions":["capture","overlay"],"ttl_secs":60,"observed_epoch":0}}
{"name":"aeye_capture_monitor","arguments":{"monitor_id":"hm-example","lease_id":"lease-0.0","max_dimension":800}}
{"name":"aeye_start_overlay","arguments":{"monitor_id":"hm-example","lease_id":"lease-0.0","duration_secs":30}}
```

These are `tools/call` params, not standalone JSON-RPC messages. Substitute the current epoch and
returned lease id; the example values grant nothing by themselves. Press Ctrl+Alt+Q locally, then
try the old lease again: it must return `STALE_EPOCH` with no image/handle. Read status again and
request a new lease explicitly to resume. Stop never erases images already received by the host.
Existing callers must remove the legacy flag and adopt this flow; the desktop app is outside this
change and has not been migrated or verified against the changed sidecar interface.

## Lease integration verification (2026-10-19, Linux only)

This session has no Windows machine. Linux runs exercise the real broker and portable MCP/tool
logic with synthetic frames/backends. The `cfg(windows)` backend, hotkey, overlay workers and
`tests/live_stdio.rs` are excluded by that target, even with `--all-targets`.

Verified here with Rust/Cargo 1.93.1: 51 part-09 unit tests (31 tool boundary tests), 25
part-10 unit tests plus 7 threaded/real-clock integration tests, and all 8 broker evidence-binary
checks. Both crates pass strict Clippy and formatting; part 09 passes Linux `cargo check
--all-targets`. Documentation validation and `git diff --check` pass. The Windows live test runs
zero tests on Linux because the entire file is target-gated.

Review: CodeRabbit could not run because its CLI is not installed (`coderabbit: command not
found`). Substituted a thorough self-review of correctness, authorization, concurrency, error
handling, test coverage and style, plus a separate read-only agent review. Fixed its findings:
wrong-action live expectation now uses `NO_ACTIVE_SCOPE`, and overlay duration metadata is a
maximum rather than a false remaining lifetime. Self-review also hardened hotkey startup timeout
cleanup, marks a dying hotkey unavailable before epoch revocation, and preserves native worker
denial codes. Review does not replace the deferred Windows build or human review.

Run locally on Linux:

```bash
cargo test --manifest-path spikes/part-09-mcp-stdio/Cargo.toml --locked
cargo check --manifest-path spikes/part-09-mcp-stdio/Cargo.toml --all-targets --locked
cargo clippy --manifest-path spikes/part-09-mcp-stdio/Cargo.toml --all-targets --locked -- -D warnings
cargo fmt --manifest-path spikes/part-09-mcp-stdio/Cargo.toml --check
cargo test --manifest-path spikes/part-10-scope-leases/Cargo.toml --locked
cargo run --manifest-path spikes/part-10-scope-leases/Cargo.toml --locked
cargo clippy --manifest-path spikes/part-10-scope-leases/Cargo.toml --all-targets --locked -- -D warnings
cargo fmt --manifest-path spikes/part-10-scope-leases/Cargo.toml --check
python3 scripts/check_docs.py
```

**Deferred Windows acceptance:** use the native Windows build/test commands above, plus
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`. The updated ignored live test
explicitly requests leases and checks missing/unknown/malformed ids, wrong scope/action, TTL
expiry, capture-only Ctrl+Alt+Q epoch revocation, stale approval refusal, overlay lease expiry,
revoked-lease refusal before explicit renewal, rival-server grant refusal and shutdown cleanup.
The Windows integration has not been built, type-checked or run in this session. A human must run
it on an interactive desktop; no Windows success is claimed here.

**Known gaps:** auto-approval replaces user consent; ids are guessable and not session-bound;
whole-monitor scopes only, no topology invalidation; no full observation envelope; running native
capture is discarded on failed delivery verification rather than interrupted. The broker releases
its lock at `verify`, leaving the existing stop-between-verify-and-stdio-send gap. Epoch metadata
lets downstream consumers reject stale results but no downstream enforcement is implemented here.
Polling and OS scheduling also leave a delay before a revoked/expired overlay disappears.

## Historical evidence before lease integration (Windows desktop, 4 monitors incl. one portrait, all at 100%)

The following evidence belongs to the original flag-gated implementation, not this integration:

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
- Original run had no scope leases, approvals or per-call consent; see the Linux-only integration evidence above.
- Behaviour when the machine is locked or the desktop is not interactive.
