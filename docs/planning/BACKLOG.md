# Prioritized implementation backlog

IDs are planning identifiers, not GitHub issue numbers. Create focused implementation issues from these once the first spikes establish scope.

| ID | Priority | Work item | Depends on | Acceptance |
| --- | --- | --- | --- | --- |
| AE-001 | P0 | Codex protocol/auth spike | None | Text+image, resume, denial, interrupt recorded |
| AE-002 | P0 | Claude companion/SDK spike | None | Routes/auth distinguished; image delivery proved |
| AE-003 | P0 | Windows capture/overlay spike | None | Correct target on two mixed-DPI monitors |
| AE-004 | P0 | Shell/sidecar packaging decision | AE-003 | Clean-machine minimal installer |
| AE-005 | P0 | Audio recognition/playback spike | None | Recognition timing and local interruption measured |
| AE-006 | P0 | Scope leases and cancellation epochs | AE-003 | Old epoch cannot dispatch after revocation |
| AE-007 | P0 | Native display/cursor transforms | AE-003 | Negative origins, rotation, scale fixtures pass |
| AE-008 | P0 | Crop pipeline and metadata | AE-006, AE-007 | Bridge receives only authorized crop |
| AE-009 | P0 | Orange overlays and stop control | AE-004, AE-006 | Indicator tied to active capture; no click interception |
| AE-010 | P0 | MCP tools over stdio | AE-008 | Host receives image; scope denied correctly |
| AE-011 | P0 | Session registry and binding | AE-001 or AE-002 | No routing by terminal focus; explicit target shown |
| AE-012 | P0 | First managed adapter | AE-011 | Conformance tests and approval flow pass |
| AE-013 | P1 | Compact dock and scope selector | AE-009, AE-011 | Keyboard/text-only primary flow usable |
| AE-014 | P1 | Push-to-talk and transcript | AE-005, AE-012 | Anchored pointer and second turn preserve context |
| AE-015 | P1 | Spoken response policy | AE-014 | No code/log narration; late audio discarded |
| AE-016 | P1 | Live region scheduler | AE-008, AE-012 | Bounded queue and adaptive delivery under slowdown |
| AE-017 | P1 | Evidence drawer and age | AE-013, AE-016 | Captured vs sent image/time visibly distinct |
| AE-018 | P1 | Event-sequence verification flow | AE-016, AE-017 | Ordered before/after evidence with scope metadata |
| AE-019 | P1 | Local metadata/credential storage | AE-011 | No keys in renderer/logs; migration recovery |
| AE-020 | P1 | Native failure acceptance suite | AE-009, AE-016 | Lock, sleep, hotplug, crash, expiry stop safely |
| AE-021 | P1 | Alpha usability sessions | AE-013–AE-020 | Task measurements and prioritized findings |
| AE-022 | P1 | Second managed adapter | AE-012, AE-021 | Same contract suite, documented differences |
| AE-023 | P1 | Signed release/update pipeline | AE-004, AE-020 | Signature verification and rollback tested |
| AE-024 | P2 | Realtime voice/perception experiment | AE-021 | Measured benefit versus complexity/cost |
| AE-025 | P2 | Follow-pointer mode | AE-020, AE-021 | No unauthorized monitor crossing; stable crop motion |
| AE-026 | P2 | macOS/Linux feasibility | Stable Windows slice | Platform-specific proof before support claim |

## Definition of ready

An issue names one user-visible outcome, relevant contracts, evidence needed, dependencies, and what is out of scope. A spike ends in a decision or measured uncertainty, not an unreviewable pile of prototype code.

## Definition of done

Behavior implemented; meaningful tests at the relevant boundary; unhappy path demonstrated; docs and compatibility record updated; no new raw-content logging; scope/approval behavior reviewed. Design-only work must remain explicitly labeled.

Avoid opening 26 empty issues as a substitute for implementation planning. Start with the five feasibility items; expand after their results.
