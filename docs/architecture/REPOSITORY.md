# Repository hierarchy and ownership

## What exists now

The repository contains a researched planning foundation, native SVG design assets, a draft observation schema/example, contributor instructions, templates, and a documentation checker. There is no runnable application scaffold disguised as completed functionality.

## Target hierarchy

Indented paths below are a proposed future layout; empty source packages have deliberately not been created.

```text
A-Eye/
  apps/
    desktop/
      src/
        features/{dock,scope,sessions,evidence,approvals,settings}/
        components/
        styles/
      src-tauri/
        src/
        capabilities/
    bridge/
      src/{agent,mcp,ipc}/
  crates/
    core/
    capture-windows/
    audio/
    policy/
    local-store/
  packages/
    contracts/
    adapter-codex/
    adapter-claude/
    voice-providers/
  tests/
    fixtures/
    contracts/
    native-windows/
    usability/
  schemas/
  examples/
  assets/
  docs/
    architecture/
    research/
    design/
    planning/
    decisions/
  scripts/
  .github/
```

## Module responsibilities

| Path | Responsibility | Dependency rule |
| --- | --- | --- |
| desktop/src | User interaction | Calls typed core commands; no provider secrets |
| desktop/src-tauri | Shell wiring, tray, windows, lifecycle | Thin composition root |
| crates/core | Session coordination and event routing | Depends on interfaces, not UI |
| crates/capture-windows | WinRT/Win32 acquisition and coordinates | Exports authorized cropped evidence |
| crates/policy | Scope leases, revocation, containment | Must be testable without a model |
| crates/audio | Devices, buffers, playback cancellation | No code-edit authority |
| crates/local-store | Metadata migrations and persistence | No raw credential storage |
| apps/bridge | Sidecar entry points | Delegates to provider packages |
| packages/contracts | Generated types/validators | No native/platform dependencies |
| packages/adapter-* | Provider lifecycle and event mapping | No direct capture-policy override |
| packages/voice-providers | Recognition/synthesis interfaces | No silent provider fallback |

Start with fewer physical crates if separate packages add overhead. Preserve conceptual boundaries, and split code when ownership or build isolation warrants it. An architectural diagram is not a mandate to create dozens of empty libraries.

## Tooling plan

Use a pnpm workspace for TypeScript and a Cargo workspace for native modules after the Phase 0 decision. Commit lockfiles once dependencies exist. Add documented commands for development, type checking, native checks, packaging, and fixture replay. Never document commands that do not yet run.

Branch convention: `main` is the coherent baseline; short-lived `feat/`, `fix/`, `docs/`, and `spike/` branches hold focused changes. No permanent second development branch is necessary. Use PRs for implementation work and record spike conclusions even when prototypes are discarded.

## Documentation maintenance

Update the feature status, source date, compatibility matrix, and roadmap when a gate changes. Keep experiments explicitly labeled. Store sanitized screenshots only with intentional consent; use synthetic application fixtures for routine tests. A new feature proposal must state the user problem, smallest slice, dependencies, and release gate.
