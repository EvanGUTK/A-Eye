# Quality gates and verification plan

## Current foundation

Run `python scripts/check_docs.py`. It checks local documentation targets, JSON parsing, draft observation consistency, and SVG XML. It does not validate provider APIs, render Mermaid, test native capture, or establish runtime security. No application test results are claimed by this repository yet.

## Boundary-focused tests

| Layer | Meaningful tests |
| --- | --- |
| Scope policy | Rectangle containment, expired leases, wrong session, stale epoch, unauthorized monitor |
| Coordinate transforms | Negative desktop origin, mixed DPI, rotated display, resize transform, crop edge clamping |
| Live scheduler | Queue cap, byte cap, slow consumer, ordered event burst, capture stop during dispatch |
| Agent adapters | Synthetic text+image turn, resume, denied approval, unknown event, disconnect after send, cancellation |
| Voice | Silence, code identifiers, device removal, cancellation, late chunks, TTS echo scenario |
| UI | Focus order, text input, disabled states, long project names, contrast, 200% scaling |
| Packaging | Clean-machine install, sidecar launch, update verification, failed migration, uninstall |

Property-based tests are especially useful for coordinate and scope math. Synthetic fixtures should include a secret-colored region outside the approved crop; verify its pixels never enter an encoded/exported image.

## Native Windows acceptance matrix

Test at least: single display; dual displays at 100%/150%; negative-origin secondary display; 200% scaling; rotated monitor; HDR on/off where available; docking/undocking; display disconnect; sleep/wake; lock/unlock; fullscreen app; protected/blank capture; microphone change; output-device change; provider outage; sidecar crash; native-core crash.

Each report records OS build, GPU/driver, monitor topology, app version, provider version, observed result, and a sanitized reproduction. CI runners cannot establish real multi-monitor overlay correctness by themselves.

## Performance experiments

| Metric | Initial target or policy | Measurement |
| --- | --- | --- |
| Local control feedback | p95 ≤ 100 ms target | Input timestamp to rendered state |
| Playback stop | p95 ≤ 150 ms target | Interrupt to last locally played sample |
| Capture stop | p95 ≤ 250 ms target | Stop event to no new acquisitions/dispatch |
| Evidence freshness | Display actual age; no false “live” promise | Captured/sent timestamps |
| Evidence buffer | ≤ 32 MiB proposed retention bound | Byte accounting, separate from GPU pools |
| Outward update rate | Default cap 1 selected frame/s | Adapter submissions over time |
| Agent answer latency | Measure, no universal SLA | End-of-speech to useful response p50/p95 |
| CPU/GPU/RAM | Establish baseline in Phase 0 | Idle versus active; hardware named |

Targets are not benchmark results. Reducing latency must not bypass scope checks, approvals, or frame readability. Record distributions and failure counts rather than one favorable demo.

## Release-blocking invariants

1. No image is dispatched after its capture epoch is revoked, except bytes already handed to the transport before revocation; document the race boundary precisely.
2. No image crosses the approved crop/monitor boundary.
3. No utterance goes to a different selected project/session.
4. Indicator failure cannot leave an invisible active capture session.
5. No secret/raw evidence content appears in default diagnostics.
6. An approval denial is not converted into an automatic approval or hidden retry.
7. Unknown adapter support is never displayed as verified compatibility.

## Human acceptance script

Choose a synthetic demo project, connect a managed session, select display 2, pin a dropdown, speak a request, open the dropdown, inspect sent evidence, deny a proposed action, ask a follow-up, interrupt speech, stop capture, then disconnect/reconnect. Confirm every state is understandable without looking at implementation logs.

## Operational checklist

Before beta: exact dependency lockfiles; provider compatibility record; license inventory for code/models/voices; signed installer/update process; rollback instructions; metadata migration backup strategy; opt-in diagnostics; documented limitations; security report route; no bundled credentials; no claimed OS support without native test evidence.
