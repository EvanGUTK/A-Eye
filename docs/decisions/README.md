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
| 013 | Linux-first (Arch/Wayland) supersedes Windows-first | Active development hardware is Arch Linux; XDG portal + PipeWire is a mature capture path; avoids Windows-only toolchain overhead during initial build | Windows demand is demonstrated and a dedicated maintainer is available |
| 014 | OAuth 2.0 + PKCE for Claude and Codex | Users have existing subscriptions; PKCE avoids client secrets in distributed code; reuses provider's own auth UX | Provider publishes a dedicated third-party registration path that improves on PKCE |
| 015 | Local Ollama model for A-Eye's internal visual processing | Keeps frame change detection and evidence description on-device; no API cost; low latency; works offline | A measured spike shows local inference is too slow or inaccurate on target hardware |
| 016 | Grok via API key (no OAuth) | xAI has no consumer OAuth subscription path; OpenAI-compatible API means no separate adapter required | xAI introduces an OAuth flow |
| 017 | Two-tier model architecture: local (Ollama) for evidence intelligence + OAuth provider for coding work | Separates fast cheap on-device inference from expensive repository-aware coding agent; neither layer can be conflated with the other | A single provider offers both real-time vision and coding at acceptable cost and latency |
| 018 | Windows-first (supersedes ADR013) | Development moved to a real Windows desktop; ADR013's revisit trigger ("Windows demand is demonstrated and a dedicated maintainer is available") is now true. Confirmed native build bridge (WSL repo + Windows-side cargo/rustc over UNC path); parts 09, 16, 17, 18 all built and proven Windows-native | Linux/Wayland demand is demonstrated and a dedicated maintainer picks it back up (parts 05-07, 10-15 stay paused, not deleted) |
| 019 | Voice engine stays on archived MIT piper; only CC0/MIT-licensed voice models ship | piper's maintained successor (piper1-gpl) is GPL-3.0, which would force any linking/combined work to also be GPL-3.0 — incompatible with A-Eye's MIT license and a commercial/SaaS distribution model. Several common piper voices are CC BY-NC-SA (non-commercial), which blocks commercial shipping outright regardless of engine. Decided by the human (CEO call via hermes), 2026-10-18, after part-08's spike flagged the gap | A legal review clears GPL-3.0 isolation (e.g. confirmed arm's-length external-process boundary) or a non-GPL maintained TTS alternative is adopted |

## Open follow-up: confirm a shippable voice model (tracks ADR019)

part-08's spike pinned `piper 2023.11.14-2` (archived, MIT) and voice `en_US-lessac-medium` (trained on Blizzard 2013; exact license not yet confirmed here). Before voice ships as a real feature (not a spike), confirm in writing:
1. The archived piper binary's exact license text (not just "last MIT release" by assumption).
2. At least one specific voice model confirmed MIT/CC0/Apache — not CC BY-NC-SA — suitable for commercial distribution.
3. Re-check piper1-gpl isn't silently pulled in as a transitive dependency anywhere in the build.

## Decision template

Title; status; date; context; alternatives; decision; consequences; primary sources; experiment result; owner; review trigger. A source link establishes a documented primitive, not a measured A-Eye implementation result.
