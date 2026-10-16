# Agent integration strategy

## Two products surfaces, one capture core

**Companion:** the user runs their usual CLI and adds A-Eye's MCP tools. The agent requests a permitted image. Text/voice input remains with that host unless a separately supported bridge exists. This mode is useful by itself but is not advertised as complete bidirectional voice.

**Managed:** the user starts/selects an A-Eye-managed agent session. A-Eye routes voice, evidence, responses, and approval events. This is the planned route for the complete experience.

## Capability matrix

All A-Eye implementation cells are currently untested.

| Capability | Codex managed candidate | Claude managed candidate | Existing CLI + MCP |
| --- | --- | --- | --- |
| Backend | app-server | Agent SDK | Host's MCP support |
| Text and images | Documented; validate version | Streaming-input image messages documented | Image tool-result support must be tested |
| Conversation continuity | Explicit thread binding | Explicit SDK session binding | Host owns conversation |
| Mid-turn input | Probe supported steering | Queue/interruption semantics need adapter tests | No generic user-turn injection |
| Spoken output | A-Eye voice pipeline | A-Eye voice pipeline | Not guaranteed by MCP |
| Approvals | Map provider events | Map permission callbacks | Host owns code approvals |
| Live terminal attachment | Research, not promised | Research, not promised | Tools augment host without taking ownership |

Source basis: [S01–S07](../research/SOURCES.md). Do not implement against guessed JSON-RPC method schemas; generate/read types from the pinned runtime and keep protocol fixtures under version control.

## Common adapter contract

Proposed operations: `probeCapabilities`, `startSession`, `resumeSession`, `submitTurn`, `interruptTurn`, `respondToApproval`, `subscribeEvents`, and `closeSession`. Capability fields include image input, resumability, steering, cancellation acknowledgement, approvals, usage reporting, and connection exclusivity.

Capability discovery returns `supported`, `unsupported`, or `unknown`, together with provider/runtime version and test evidence. An unknown capability stays disabled until established. A successful login is not proof of image input or plan entitlement.

Normalize events into: session-ready, text-delta, tool-started, tool-completed, approval-requested, usage-reported, turn-completed, cancelled, disconnected, and error. Preserve original provider IDs for debugging without persisting raw private payloads by default.

## Session binding and concurrency

Bind `(project_id, provider, provider_session_id, owner_instance_id)` before accepting a voice turn. Show the project basename and an expandable full path. Session switching cancels pending speech playback and invalidates unsent evidence; require a new capture lease for a new session binding.

Do not use fuzzy window titles to decide which agent receives a message. Do not blindly type into whichever terminal has focus. Do not mutate provider transcript files to manufacture session attachment. Resumption must use a supported provider interface and the user-selected session ID.

One active managed turn per session is the initial rule. If busy, keep an explicit follow-up queue of one user turn or offer interruption. Coalesce visual updates; never enqueue dozens of stale image turns. Unsolicited visual changes do not authorize new code-editing tasks.

## Authentication and billing

Embedded Claude uses supported API authentication unless the publisher has explicit approval for another route. The provider's SDK guidance restricts third-party use of subscription login/limits [S03]. This is a product integration constraint, not a claim about every personal CLI workflow. Use “Claude” in the embedded agent selector; avoid presenting the app as Anthropic's Claude Code product.

For Codex, use a supported user-authorized runtime route and verify what is permitted for distribution. Do not read/copy session tokens from unrelated applications. Do not claim voice costs are included in a coding plan. UI settings must separate coding provider, transcription provider, and speech output.

## Conversation truth

The coding session owns code conclusions and tool results. A-Eye may emit deterministic status speech (“Waiting for your approval”) but must not invent agent conclusions. If a future realtime narrator summarizes a result, label it as a summary and ground it in a completed result ID.

If an image fails to deliver, say so. A transcript plus missing image is not a successful visual turn. Verify attachment acceptance, then show a sent timestamp. “Sent” does not imply that the model has finished interpreting it.

## Compatibility record

For every released adapter record OS/build, runtime version, model capability, authentication route, transport, image fixture result, resumed-turn result, approval denial result, interrupted-turn result, and known limitations. Re-run on upgrades. Never turn a documentation observation into a green support badge.

---

## Linux and OAuth addendum

See [OAUTH.md](OAUTH.md) for the full OAuth implementation plan.
See [LOCAL-MODEL.md](LOCAL-MODEL.md) for the local Ollama layer.

### Updated capability matrix (Linux / OAuth / local model)

| Capability | Codex (OAuth) | Claude (OAuth) | Grok (API key) | Ollama (local) |
|---|---|---|---|---|
| Auth method | OAuth PKCE (ChatGPT sub) | OAuth PKCE (claude.ai sub) | API key | None |
| Linux support | Yes (CLI is cross-platform) | Yes (SDK is cross-platform) | Yes (HTTP API) | Yes (native) |
| Image input | Documented; validate version | Documented; validate version | Supported (grok-2-vision) | Yes (minicpm-v, moondream2) |
| Role in A-Eye | Coding agent (managed) | Coding agent (managed) | Coding agent (managed / API) | Internal evidence intelligence only |
| Conversation continuity | Socket session | SDK session binding | Stateless (per request) | N/A |
| Code actions | Yes (approval-gated) | Yes (approval-gated) | Yes (approval-gated) | No — read-only |
| Cost | Subscription (included) | Subscription (included) | Per-token API cost | Free (local compute) |

### Two-tier model architecture (ADR-017)

A-Eye separates two model roles that must never be conflated:

**Tier 1 — Local model (Ollama):** Runs on-device inside A-Eye's core pipeline.
Handles frame change detection, evidence scoring, and initial visual description.
Fast, free, private. Cannot write code, approve actions, or access the repo.

**Tier 2 — Coding agent (Claude / Codex / Grok):** Receives the curated
evidence bundle that Tier 1 selected. Does repository-aware investigation,
code editing, and approval-gated actions. Authenticated via OAuth or API key.

```
Screen frames → [Ollama: select + describe] → Evidence bundle → [Claude/Codex/Grok: code work]
```

The two tiers are configured independently in Settings and can be changed
without affecting each other.
