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

## Phase 0 — Prove the interfaces

Deliver short, reproducible spikes rather than a large scaffold.

**S0.1 Codex session proof:** start a supported managed session, send text and a synthetic image, receive structured events, deny an approval, cancel, resume, and confirm the second turn references the first. Record runtime/version/auth. Confirm image input and session ownership behavior independently.

**S0.2 Claude route proof:** validate MCP image tools in the user's CLI and test an API-authenticated SDK session separately. Capture input, queue, approval, interruption, and resume behavior. Do not use unsupported subscription-token extraction.

**S0.3 Native capture proof:** capture one selected monitor, draw an orange non-activating overlay, crop around a pinned cursor, and stop locally. Test a second monitor with a different scale and negative desktop origin. Record any system border and overlay capture artifacts.

**S0.4 Voice proof:** recognize code-oriented speech and speak a reply. Measure end-of-speech to usable transcript; cancel output midway. Compare a local STT option if hardware permits.

**S0.5 Shell/packaging proof:** produce a minimal installable Tauri prototype with one sidecar. Compare a small Electron alternative if native integration stalls. No full duplicate implementation.

**Exit:** written capability matrix, exact tested versions, synthetic fixture outputs, selected shell, auth requirements, and a minimal clean-machine reproduction. A failed spike narrows the product honestly; it does not disappear under a compatibility badge.

## Phase 1 — Visual companion

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
