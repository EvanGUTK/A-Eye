# HANDOFF — resume here

Last updated 2026-10-19 (later same day) by hermes (orchestrator). Read this first on resume,
before re-reading `spikes/README.md`, `docs/planning/ROADMAP.md`'s "Current status" section, or
the Obsidian project log at `AI Projects/A-Eye.md`
(path: `/mnt/e/VAULT/USAAS/AI Projects/A-Eye.md`) — this file is the short version, those are the
detailed ones.

## What's true right now

- Repo lives at `~/projects/code/A-Eye` in WSL; `main` is pushed to `origin/main`, clean, no open
  agent worktrees, no agents currently running.
- Dev machine is Windows (a real desktop, not a VM) accessed from this WSL session via
  `powershell.exe` — native Rust/Tauri builds go through the Windows-side `cargo`/`rustc`/`node`
  over a UNC path (`\\wsl$\Ubuntu\...`), with `$env:CARGO_TARGET_DIR` set to a local Windows path to
  dodge a cargo incremental-compile lock bug over UNC. Full recipe: `spikes/README.md`'s
  "Platform pivot" section. `powershell.exe -Command` with `$ErrorActionPreference="Continue"` is
  required when piping cargo/node output through `Select-Object` — the default `Stop` turns
  cargo's normal stderr "Compiling ..." lines into a fatal `NativeCommandError`, which looks like a
  build failure but isn't; check for "Finished `...` profile" in the output, not the exit code.
- Phase 0 (feasibility) is substantially done on Windows: live Codex OAuth, native capture+overlay,
  voice (STT/TTS), and a packaged Tauri shell are all proven and merged. Claude OAuth is
  intentionally NOT attempted (Anthropic policy, issue #11). Full Phase 0 table:
  `docs/planning/ROADMAP.md` → "Current status (2026-10-18)".
- The first real runnable product exists: `apps/desktop` (PR #43) — a Tauri window hosting the
  part-09 MCP server as a sidecar, with live monitor list, overlay controls, and frame preview.
  As of today (PRs #49, #48, #50 merged, then live-verified on Windows; PR #53 fixed what that
  verification found):
  - part-09's capture/overlay tools now require a valid lease (part-10's `LeaseBroker`) instead of
    the blunt `--allow-capture` flag; lease grant is still auto-approved inside the MCP tool call
    (no GUI consent prompt yet) — documented as a known gap, not hidden. **Live-verified on
    Windows**: 51 unit tests + the live interactive-desktop test (39 checks) pass natively — real
    capture, overlay, lease grant/deny, epoch revocation, and the Ctrl+Alt+Q stop control all
    proven on the real desktop.
  - Codex OAuth tokens persist via the part-01 credential store and reload/refresh on startup
    instead of re-running the full OAuth dance every launch (issue #10 closed). **Live-verified on
    Windows**: the `keyring` crate auto-selects Windows Credential Manager under the
    `SecretServiceBackend` abstraction (not actually Secret Service/D-Bus — that name is a
    Linux-era leftover) and real write/read/delete round trips pass. Originally didn't even
    compile on Windows (several `std::os::unix`-only code paths); fixed in PR #53.
  - Vision (part-06) is wired as an explicit "Describe frame" button (issue #47 closed, PR #50) —
    captures a frame, sends it to a local Ollama vision model (default `qwen3-vl:2b`), shows the
    text description in the UI. Follow-up issue #51 opened: Windows owner-PID check on the loopback
    Ollama port (currently any local process on that port could receive the frame). **Not yet
    live-verified against a real Ollama**: this dev machine's only running Ollama instance is
    inside WSL on `127.0.0.1:11434`; WSL/Windows share that loopback address (mirrored networking)
    so Windows can't bind its own `ollama serve` there, but Windows also can't actually reach the
    WSL-side instance (connect times out) — needs the user's input on how to proceed (stop WSL's
    Ollama temporarily? pull `qwen3-vl:2b` there instead?) since other things may depend on the
    WSL instance already running (see "Open decisions" below).
  - **apps/desktop's own MCP host was never updated for scope leases** (found via live
    verification): it still launched the sidecar with `--allow-capture`, which PR #49 made part-09
    reject outright, so the whole app would have failed to even connect. PR #53 removed the dead
    flag so the sidecar at least starts, but Capture/Show overlay are still denied
    (`NO_ACTIVE_SCOPE`) because the host doesn't call `aeye_request_watch` or pass a `lease_id` —
    tracked as issue #52, a real (not cosmetic) follow-up.
  - Still not wired: voice (part-08) into `apps/desktop`.

## Open decisions that need the human, not an agent

1. **Claude OAuth release gate** (issue #22 item 3): pursue written Anthropic approval for
   A-Eye-owned Claude OAuth, or formally drop it in favor of the supported routes (user's own
   `claude` CLI login via MCP, or their own `ANTHROPIC_API_KEY`)? Blocks any Claude-dependent
   SaaS/commercial work.
2. **Voice model license** (issue #34, ADR019): confirm the exact license of archived piper +
   pick a specific MIT/CC0 voice model before voice ships as a real feature, not just a spike.
3. **Ollama port conflict** (new today): this dev machine already runs an Ollama instance inside
   WSL (serving other local-model workflows — `dmux-localcoder`, `ollama-gpu-guard.py`, etc.) bound
   to `127.0.0.1:11434`. Because of WSL2 mirrored networking, that binding blocks a native Windows
   `ollama serve` from using the same port, yet Windows can't actually reach the WSL instance
   either. Live-verifying `apps/desktop`'s "Describe frame" feature needs a real Ollama reachable
   from Windows with `qwen3-vl:2b` pulled — ask before stopping/reconfiguring the user's existing
   WSL Ollama, since other running agents may depend on it.

## Immediate next steps (engineering, no human decision needed first)

See `docs/planning/ROADMAP.md`'s "Current status" section for the full ranked list. Short version:
wire apps/desktop to part-10 scope leases for real (issue #52 — `aeye_request_watch` +
`lease_id` plumbing, not just unblocking the sidecar), replace the lease auto-approve stub with a
real GUI consent prompt, address issue #51 (Windows owner-PID check on the loopback Ollama port),
then live-verify vision "Describe frame" once decision #3 above is resolved. Then: wire voice
(part-08) into `apps/desktop`.

## Standing operating rules (from the user, this session)

- Merging a finished round's PRs is the default action — don't ask permission, just do it one PR
  at a time, syncing `main` between each (see `multi-agent-dev-orchestration` skill).
- When CodeRabbit is rate-limited, agents self-review instead of waiting on the quota reset.
- Agent rosters should skew ~75% codex / 25% claude.
- Every dispatched agent gets its own `herdr worktree`; the orchestrator never shares a checkout
  with a working agent.
- No locally-hosted web server reachable from the LAN, ever — MCP stdio / native exe only. Kill
  any preview/dev server an agent leaves running that binds beyond `localhost` loopback.
- Hermes answers agents' plan-approval/clarifying questions itself (A2A) — never relay them to
  the user.
- Replies to the user: TLDR style, short and bulleted, lead with the answer.
