# Delivery roadmap

This is an evolving product, but releases still need a bounded definition of done. Estimates are **planning ranges in focused engineering weeks for one experienced developer**, not calendar commitments or measured project velocity. Learning Rust/native capture, part-time availability, provider changes, and packaging can extend them substantially.

## Milestone summary

| Phase | Planning range | User-visible result | Exit gate |
| --- | --- | --- | --- |
| 0 · Feasibility | 1–2 weeks | Recorded technical proofs | Session, image, capture, auth, packaging routes established |
| 1 · Visual companion | 2–3 weeks | Point → image → useful answer | Correct target, permission, stop, host image fidelity |
| 2 · Voice session | 2–4 weeks | Talk and receive spoken reply | Same session, cancellation, approvals, text fallback |
| 3 · Live region | 2–4 weeks | Demonstrate an interaction live | Freshness, backpressure, multi-frame evidence, scoped stop |
| 4 · Private alpha | 2–3 weeks | Reliable installable workflow | Usability and Windows acceptance matrix |
| 5 · Public beta | 2–4 weeks | Signed distribution and second managed adapter | Compatibility evidence and release operations |

Total indicative effort through beta: **11–20 focused weeks**, with uncertainty concentrated in Phase 0, native behavior, and provider integrations. Do not promise this as a solo beginner's three-month delivery date. After Phase 0, replace ranges with estimates based on actual completed work.

## Current status (2026-10-18) — read this first

Development pivoted Windows-first on 2026-10-18 (ADR018, `docs/decisions/README.md`), superseding ADR013's Linux-first call below. Phase 0 is substantially done on Windows:

| Item | Status |
| --- | --- |
| S0.1 Codex session proof | **Done, live-verified.** 4 PASS/0 FAIL/1 UNVERIFIED (id_token signature). See `docs/planning/LIVE-AUTH-RUNBOOK.md`, PR #42. |
| S0.2 Claude route proof | **Intentionally blocked.** Anthropic policy (F01) still disallows third-party claude.ai login; see issue #11, #22 item 3 (owner decision still open: pursue written approval, or drop A-Eye-owned Claude OAuth). |
| S0.3 Native capture proof | **Done on Windows** (parts 17/18, PR #25). Mixed-DPI/negative-origin unverified — this dev machine's 4 monitors are all 100% scale. |
| S0.4 Voice proof | **Done**, with a caveat: whisper.cpp + archived piper work (part-08, PR #33), but piper's voice-model license needs confirming before shipping (issue #34, ADR019). |
| S0.5 Shell/packaging proof | **Done.** Tauri 2 → native Windows exe/installer (part-16, PR #31), now hosting the real MCP sidecar in `apps/desktop` (PR #43) — the first actually-runnable product, not just a spike. |

Beyond Phase 0: part-06 (Ollama vision), part-09 (MCP stdio server, 5 tools), part-10 (scope leases — built but **not yet wired into part-09**, which still only gates capture behind a blunt `--allow-capture` flag) are also merged. See `spikes/README.md` for the full per-spike evidence table and issue #22 for the living follow-up tracker.

**Immediate next steps, roughly in priority order:**
1. Wire part-10 (scope leases) into part-09's MCP server, replacing `--allow-capture`.
2. Store Codex tokens via the part-01 credential store (issue #10) — token exchange works but nothing persists yet.
3. Resolve the Claude OAuth release-gate decision (issue #22 item 3) before any Claude-dependent SaaS work.
4. Confirm a shippable (MIT/CC0) voice model license (issue #34) before voice graduates from spike to feature.
5. Wire vision (part-06) into `apps/desktop` so captured frames actually get described, not just previewed.

The Linux-first section immediately below (L0.x items) is **paused, not deleted** — resume it if/when Linux becomes a real target again.

## Phase 0 — Prove the interfaces

Deliver short, reproducible spikes rather than a large scaffold.

**S0.1 Codex session proof:** start a supported managed session, send text and a synthetic image, receive structured events, deny an approval, cancel, resume, and confirm the second turn references the first. Record runtime/version/auth. Confirm image input and session ownership behavior independently.

**S0.2 Claude route proof:** validate MCP image tools in the user's CLI and test an API-authenticated SDK session separately. Capture input, queue, approval, interruption, and resume behavior. Do not use unsupported subscription-token extraction.

**S0.3 Native capture proof:** capture one selected monitor, draw an orange non-activating overlay, crop around a pinned cursor, and stop locally. Test a second monitor with a different scale and negative desktop origin. Record any system border and overlay capture artifacts.

**S0.4 Voice proof:** recognize code-oriented speech and speak a reply. Measure end-of-speech to usable transcript; cancel output midway. Compare a local STT option if hardware permits.

**S0.5 Shell/packaging proof:** produce a minimal installable Tauri prototype with one sidecar. Compare a small Electron alternative if native integration stalls. No full duplicate implementation.

**Exit:** written capability matrix, exact tested versions, synthetic fixture outputs, selected shell, auth requirements, and a minimal clean-machine reproduction. A failed spike narrows the product honestly; it does not disappear under a compatibility badge.

## Phase 1 — Visual companion

The first integration slice is [apps/desktop](../../apps/desktop/README.md): a native Windows
Tauri host for the existing stdio MCP server, monitor selection, timed border controls, and
one-frame local preview. This does not complete the Phase 1 exit demo: scope leases, agent
delivery, cursor crops, and the observation envelope are still absent from the app.

Build scope policy, display enumeration, cursor anchor, crop preview, orange border, stop control, and a basic MCP tool server. Add the observation envelope, explicit errors, and image-size bounds. Test with both target CLI hosts when available; state any host that remains unverified.

**Exit demo:** in a synthetic app, point at a clipped menu, request inspection, get a relevant answer, and stop. No out-of-scope pixels reach the bridge; stop works with the host disconnected.

## Phase 2 — Managed voice session

Implement the first managed adapter selected by Phase 0, not both simultaneously. Add project/session selection, push-to-talk, transcript display, image+text submission, response narration, provider approval surface, and explicit cancellation semantics. Maintain the companion path.

**Exit demo:** two spoken turns in one conversation, one visual inspection, one denied approval, and an interrupted spoken response. The second turn must retain context and remain in the selected project.

## Phase 3 — Live region

Add bounded region acquisition, frame selection, memory budgets, evidence age, finite watch leases, coalescing, explicit burst requests, and current capture/sent distinction. The coding agent receives ordered useful evidence without endless user turns. Include “watch while I open this” and “check again.”

**Exit demo:** demonstrate a menu interaction, identify an observed change, inspect source, then recapture after a user-applied/reloaded change. Under simulated provider slowdown, the UI stays responsive and stale frames do not accumulate. A hotkey stops capture during an active request.

**This phase completes the first useful live product**, subject to the quality gates. It does not imply continuous comprehension of every video frame.

## Phase 4 — Private alpha

Recruit a small group of developers. Run task-based usability comparisons. Fix target-selection errors, permission confusion, hotkey conflicts, unreadable evidence, and session-routing surprises. Test startup, sleep, screen lock, monitor removal, device changes, and child-process crashes. Add basic diagnostics without recording private content.

**Exit:** no known scope/routing violations; usable without voice; transparent privacy behavior; clean-machine installer; documented compatibility and remaining limitations. Alpha telemetry is opt-in.

## Phase 5 — Public beta

Add the second managed adapter with its own conformance evidence. Implement signing/update verification, reproducible release packaging, rollback/recovery instructions, dependency/license inventory, and support documentation. Decide whether a realtime narrator earns its cost from measured feedback; it is not an automatic beta requirement.

**Exit:** published release checklist completed, supported version matrix, clear billing/auth expectations, and a trustworthy uninstall/update path. A public repository alone is not a beta release.

## Later tracks

Follow-pointer mode, window capture, macOS, Wayland/Linux, local-only vision, browser DOM evidence, annotation, recorded reproduction clips, realtime narration, and team features are separate proposals. Prioritize by observed failures and user value, not by the number of integrations competitors advertise.

## First ten working days

| Day range | Focus | Concrete output |
| --- | --- | --- |
| 1–2 | Provider/session/auth research spike | Compatibility record and text/image round trip |
| 3–4 | Native monitor/cursor/overlay | Mixed-DPI capture recording and failure notes |
| 5 | Small MCP bridge | Synthetic image reaches selected host |
| 6–7 | Voice/cancellation | Latency log and interrupted playback proof |
| 8 | Packaging | Clean-machine launch with sidecar |
| 9 | Threat/failure review | Scope/stop tests and resolved architecture decisions |
| 10 | Scope reset | Re-estimated backlog based on evidence |

This is a suggested sequence, not a claim that all experiments will fit the time box. If blocked, record the result and revisit the scope.

---

## Phase 0 — Linux spikes (paused 2026-10-18, see "Current status" above)

ADR-013 superseded the Windows-first assumption when this was written; ADR018 has since superseded ADR013 back to Windows-first. This section is kept for when Linux work resumes, not deleted. The spikes
below replace or supplement the original Phase 0 items for Arch Linux / Wayland.
See [LINUX.md](../architecture/LINUX.md), [OAUTH.md](../architecture/OAUTH.md),
and [LOCAL-MODEL.md](../architecture/LOCAL-MODEL.md) for full context.

**L0.1 XDG ScreenCast portal + PipeWire frame capture**
Use `ashpd` to open a portal session, select one monitor, consume the PipeWire
stream, extract one frame, save as PNG. Test on two monitors with different
scale factors. Record: portal dialog behavior, frame format (DMA-BUF vs SHM),
latency from portal confirm to first frame, any compositor-specific quirks.

**L0.2 wlr-layer-shell orange border overlay**
Using `smithay-client-toolkit`, draw a 2-logical-pixel orange border on the
selected monitor. Verify: no focus steal, no pointer interception, survives
monitor hotplug, survives compositor restart. Test on Hyprland and at least
one other compositor. Record: which compositors support `zwlr_layer_shell_v1`.

**L0.3 libsecret credential storage**
Write and read a test credential via `keyring` crate (Secret Service D-Bus).
Verify it does not appear in plaintext on disk. Test the encrypted-file fallback
path on a system without a keyring daemon (bare WM). Record setup steps for
the `input` group requirement if applicable.

**L0.4 OAuth PKCE flow — Codex (ChatGPT subscription)**
Complete the full PKCE flow against OpenAI auth in a standalone Rust binary.
Store token via L0.3 credential store. Start Codex app-server daemon. Send one
synthetic text + image turn via the Unix socket. Receive and parse the response.
Record: exact client_id, token endpoint, socket protocol version, image input
format, session resumability. See [OAUTH.md](../architecture/OAUTH.md).

**L0.5 OAuth PKCE flow — Claude (claude.ai subscription)**
Same as L0.4 but against Claude auth. Use the Claude Agent SDK with the stored
token. Send one text + image turn in managed mode. Record: auth endpoint,
token lifetime, SDK version, managed session binding behavior.

**L0.6 Local model — Ollama vision spike**
Install Ollama on Arch. Pull `moondream2` and `minicpm-v:8b`. Capture one crop
from a running synthetic UI (a simple demo web page). Send to each model via
the `/v1/chat/completions` endpoint. Record: inference latency p50/p95 on target
GPU, description quality on UI text, GPU memory usage during concurrent
capture + inference. Establish whether local model adds meaningful evidence
quality over pure pixel diff. See [LOCAL-MODEL.md](../architecture/LOCAL-MODEL.md).

**L0.7 evdev global hotkey on Wayland**
Read a key event from `/dev/input/eventN` without window focus using `evdev-rs`.
Verify the `input` group membership requirement. Test push-to-talk hotkey
detection while another application has focus. Document the polkit alternative
for systems where group membership is undesirable.

**L0.8 Tauri 2 + AppImage packaging on Arch**
Produce a minimal Tauri 2 app (just the dock shell, no real capture) as an
AppImage. Install on a second clean Arch VM. Verify launch, WebKitGTK version,
Wayland surface rendering. Record: binary size, startup time, WebKitGTK
dependency chain.

### Linux Phase 0 exit gate

Written capability matrix covering all L0.x items; exact tested versions for
Rust toolchain, Node LTS, Tauri, compositor, GPU driver, Ollama, model weights;
synthetic fixture outputs for capture, auth, and local model; selected packaging
path; resolved ADR-003 (Tauri vs Electron) based on L0.8 evidence.

### Linux first ten working days (revised)

| Day range | Focus | Concrete output |
|---|---|---|
| 1 | Portal + PipeWire frame (L0.1) | PNG saved from live monitor |
| 2 | Layer-shell overlay (L0.2) | Orange border on selected monitor |
| 3 | Credential store (L0.3) | Token written + read; fallback tested |
| 4 | Codex OAuth + app-server (L0.4) | Text + image round trip recorded |
| 5 | Claude OAuth + SDK (L0.5) | Managed session turn recorded |
| 6 | Ollama vision (L0.6) | Latency + quality numbers on GPU |
| 7 | evdev hotkey (L0.7) | Push-to-talk fires without window focus |
| 8 | Tauri + AppImage (L0.8) | Installs and launches on clean Arch VM |
| 9 | Threat + failure review | Scope/stop tests; epoch revocation proved |
| 10 | Scope reset | Re-estimated backlog from evidence |
