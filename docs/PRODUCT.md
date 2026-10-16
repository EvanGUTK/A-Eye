# Product definition

## Thesis

Developers often understand a visual defect before they can describe it. A-Eye should turn pointing, speaking, and demonstrating into actionable context for the coding conversation doing the work. Its value connects **human intent, localized visual evidence, and repository-aware investigation**.

This is a desktop companion with an MCP interface and optional managed agent sessions. It is not inherently a new foundation model, an IDE, a meeting recorder, or an autonomous computer-control platform.

## Primary user and job

Initial user: a developer building a local web or desktop application who already works with a coding agent. They can run a development server and identify an issue visually but find screenshot handling and precise written descriptions cumbersome.

Job: “When I notice something wrong on screen, let me show and explain it without losing the conversation or selecting the wrong project.”

Secondary users may include designers, testers reproducing UI bugs, and people who benefit from voice input. Validate these separately; one interface will not automatically satisfy every accessibility need.

## The central scenario

1. Select a project and agent session. The dock shows both.
2. Open the application under test on monitor 2.
3. Pin a region around a dropdown with a hotkey or explicit selection.
4. Say: “Watch here. When I open the menu, the last two items disappear.”
5. The monitor border turns solid orange. The region outline identifies pixels eligible for transmission.
6. Open the menu. A-Eye prepares a small evidence sequence with timestamps.
7. The agent explains the observation and inspects source. Pixels alone do not prove a particular CSS cause.
8. Approve a code action through the agent's permission flow.
9. Reload the application yourself in the first version. Ask “check again.”
10. Fresh evidence lets the agent state what improved, remains uncertain, or is unchanged.

Observation and code actions are distinct permissions. A request to inspect does not grant permission to modify arbitrary files.

## Product modes

| Mode | Scope | Lifetime | Default |
| --- | --- | --- | --- |
| Look | One bounded image | One request | Cursor anchor at hotkey press |
| Watch region | Pinned rectangle | Explicit bounded session | No automatic monitor crossing |
| Follow pointer | Moving crop on one monitor | Explicit bounded session | Later feature, visibly labeled |
| Watch monitor | One chosen monitor | Explicit bounded session | Separate scope confirmation |

Window-only capture is a useful follow-up. Minimized windows, protected content, and occlusion behave differently across platforms and must be described accurately.

## First useful release

Included: Windows desktop, selected session, text fallback, push-to-talk, spoken reply, monitor selection, cursor-anchored image, pinned live region, evidence age, stop control, approvals, and bounded local buffering.

Deferred: arbitrary terminal attachment, multi-agent orchestration, A2A, wake words, full-duplex audio, DOM extraction, automatic clicks/reload, permanent recording, team sync, mobile, accounts, billing, and cross-platform parity.

**Live observation is included.** A still-image spike comes first to prove delivery, not to redefine the product as a screenshot utility.

## Requirements

| ID | Requirement | Observable outcome |
| --- | --- | --- |
| P01 | Select a session | Project path and agent visible before sending |
| P02 | Point accurately | Anchor remains stable after the cursor moves |
| P03 | Observe changes | Ordered selected frames represent an interaction |
| P04 | Report freshness | Last captured and last sent times distinguishable |
| P05 | Stop locally | Stop works with an unresponsive agent/network |
| P06 | Preserve agency | No scope expansion or approval inferred from screen text |
| P07 | Continue conversation | Second spoken turn reaches the same managed session |
| P08 | Recover intelligibly | Specific actions for auth, permission, and connection errors |
| P09 | Work without voice | Core tasks have keyboard/text equivalents |
| P10 | Minimize transmission | Only authorized crops/selected frames leave the capture boundary |

## Success measures

Begin with at least five developers and synthetic tasks: clipping, hover disappearance, focus ring, small text, and animation timing. Compare ordinary describe/attach workflows against A-Eye; report sample size and hardware.

- Time from noticing a defect to delivery of correct evidence.
- Correct target identification on first attempt.
- Tasks completed without manually exporting images.
- Misrouted sessions: target zero.
- Scope violations/post-stop submissions: release-blocking, target zero.
- Ability to identify what is shared and with whom.
- Measured latency, bytes, and provider usage per task.

Code volume and feature count are not product success measures.

## Positioning hypothesis

“Point and talk to your coding session” is a convenience hypothesis, not a claim of market uniqueness. Voice features, screen recorders, screen-memory products, and computer-use agents overlap. A-Eye must earn its place through session continuity, accurate focus, temporal evidence, and trustworthy controls.

A-Eye is memorable in writing but sounds like “AI.” Test pronunciation, search discoverability, and name availability before launch. No trademark clearance has been performed.
