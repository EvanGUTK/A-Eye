# HANDOFF — resume here

Last updated 2026-10-18 by hermes (orchestrator). Read this first on resume, before re-reading
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
  It is NOT yet wired to scope leases (part-10), vision (part-06), or voice (part-08).

## Open decisions that need the human, not an agent

1. **Claude OAuth release gate** (issue #22 item 3): pursue written Anthropic approval for
   A-Eye-owned Claude OAuth, or formally drop it in favor of the supported routes (user's own
   `claude` CLI login via MCP, or their own `ANTHROPIC_API_KEY`)? Blocks any Claude-dependent
   SaaS/commercial work.
2. **Voice model license** (issue #34, ADR019): confirm the exact license of archived piper +
   pick a specific MIT/CC0 voice model before voice ships as a real feature, not just a spike.

## Immediate next steps (engineering, no human decision needed first)

See `docs/planning/ROADMAP.md`'s "Current status" section for the full ranked list. Short version:
wire scope leases into part-09 (replace `--allow-capture`), persist Codex tokens via the part-01
credential store (issue #10), wire vision into `apps/desktop`.

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
