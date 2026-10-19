# HANDOFF — resume here

Last updated 2026-10-19 by hermes (orchestrator). Read this first on resume, before re-reading
`spikes/README.md`, `docs/planning/ROADMAP.md`'s "Current status" section, or the Obsidian project
log at `AI Projects/A-Eye.md` (path: `/mnt/e/VAULT/USAAS/AI Projects/A-Eye.md`) — this file is the
short version, those are the detailed ones.

## What's true right now

- Repo lives at `~/projects/code/A-Eye` in WSL; `main` is pushed to `origin/main`, clean, no open
  agent worktrees, no agents currently running.
- Dev machine is Windows (a real desktop, not a VM) accessed from this WSL session via
  `powershell.exe` — native Rust/Tauri builds go through the Windows-side `cargo`/`rustc`/`node`
  over a UNC path (`\\wsl$\Ubuntu\...`), with `$env:CARGO_TARGET_DIR` set to a local Windows path to
  dodge a cargo incremental-compile lock bug over UNC. Full recipe: `spikes/README.md`'s
  "Platform pivot" section.
- Phase 0 (feasibility) is substantially done on Windows: live Codex OAuth, native capture+overlay,
  voice (STT/TTS), and a packaged Tauri shell are all proven and merged. Claude OAuth is
  intentionally NOT attempted (Anthropic policy, issue #11). Full Phase 0 table:
  `docs/planning/ROADMAP.md` → "Current status (2026-10-18)".
- The first real runnable product exists: `apps/desktop` (PR #43) — a Tauri window hosting the
  part-09 MCP server as a sidecar, with live monitor list, overlay controls, and frame preview.
  As of today (PRs #49, #48, #50, all merged, built/tested on Linux only — none live-verified on
  Windows yet):
  - part-09's capture/overlay tools now require a valid lease (part-10's `LeaseBroker`) instead of
    the blunt `--allow-capture` flag; lease grant is still auto-approved inside the MCP tool call
    (no GUI consent prompt yet) — documented as a known gap, not hidden.
  - Codex OAuth tokens persist via the part-01 credential store and reload/refresh on startup
    instead of re-running the full OAuth dance every launch (issue #10 closed).
  - Vision (part-06) is wired as an explicit "Describe frame" button (issue #47 closed, PR #50) —
    captures a frame, sends it to a local Ollama vision model (default `qwen3-vl:2b`), shows the
    text description in the UI. Follow-up issue #51 opened: Windows owner-PID check on the loopback
    Ollama port (currently any local process on that port could receive the frame).
  - Still not wired: voice (part-08) into `apps/desktop`.

## Open decisions that need the human, not an agent

1. **Claude OAuth release gate** (issue #22 item 3): pursue written Anthropic approval for
   A-Eye-owned Claude OAuth, or formally drop it in favor of the supported routes (user's own
   `claude` CLI login via MCP, or their own `ANTHROPIC_API_KEY`)? Blocks any Claude-dependent
   SaaS/commercial work.
2. **Voice model license** (issue #34, ADR019): confirm the exact license of archived piper +
   pick a specific MIT/CC0 voice model before voice ships as a real feature, not just a spike.

## Immediate next steps (engineering, no human decision needed first)

See `docs/planning/ROADMAP.md`'s "Current status" section for the full ranked list. Short version:
live-verify today's three merges on the real Windows machine (scope leases gating capture/overlay,
Codex token persistence, vision "Describe frame" against a real Ollama install), replace the
auto-approve lease grant with a real GUI consent prompt, and address follow-up issue #51 (Windows
owner-PID check on the loopback Ollama port). Then: wire voice (part-08) into `apps/desktop`.

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
