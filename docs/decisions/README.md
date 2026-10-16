# Architecture decision record

Date: 2026-10-15. All decisions below are **proposed**, not experimentally validated. Amend or supersede after Phase 0; keep the rationale history.

| ADR | Decision | Rationale | Revisit when |
| --- | --- | --- | --- |
| 001 | Native desktop companion plus MCP | Capture/voice/session ownership exceed a standalone tool server | A supported host offers equivalent native lifecycle |
| 002 | Windows-first | Keeps native scope finite and matches initial development context | Windows workflow is stable and platform demand is demonstrated |
| 003 | Tauri/Rust with TypeScript bridge | Separates native resources from provider protocol ecosystem | Packaging or implementation spike favors Electron |
| 004 | Companion and managed modes remain distinct | Avoids unsupported live-terminal/session promises | Provider exposes verified safe attachment |
| 005 | One coding authority per session | Prevents conflicting changes and narrative truth | Explicit multi-agent coordination becomes necessary |
| 006 | Push-to-talk before hands-free | Reduces echo, accidental input, and turn-taking complexity | Usability results justify full-duplex work |
| 007 | Adaptive images before continuous video transport | Matches available agent evidence workflows and bounded costs | A verified video-native path improves target tasks |
| 008 | Scope leases and epoch revocation | Makes consent and stop enforceable at dispatch | Security review finds a stronger mechanism |
| 009 | Ephemeral image/audio buffers by default | Avoids unneeded capture history | Users opt into a documented retention feature |
| 010 | No hosted application backend initially | Single-user workflow does not require accounts/sync | Team or hosted-service requirements are accepted |
| 011 | Preserve OS capture indicators | Custom branding must not evade system permission behavior | Supported explicit user consent enables customization |
| 012 | No A2A in initial scope | Human-to-agent interface does not require agent delegation | Real delegation use case is validated |

## Decision template

Title; status; date; context; alternatives; decision; consequences; primary sources; experiment result; owner; review trigger. A source link establishes a documented primitive, not a measured A-Eye implementation result.
