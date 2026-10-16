# Draft contracts

Status: design only, version `0.1.0-draft`. Names here are **A-Eye's proposed interfaces**, not assertions about provider APIs. The [observation schema](../../schemas/observation.schema.json) and [synthetic example](../../examples/observation.json) specify an initial portable evidence envelope.

## Observation envelope

| Field | Meaning |
| --- | --- |
| schema_version | Contract version; reject unknown major versions |
| observation_id | Unique evidence identifier |
| session_id | A-Eye session, mapped privately to a provider session |
| capture_epoch | Generation of the active capture lease |
| sequence | Monotonic sequence within that epoch |
| captured_at | UTC wall-clock timestamp |
| monotonic_ms | Relative local monotonic time, not comparable across machines |
| monitor_id | Instance-scoped monitor identity, never assumed stable forever |
| scope | Mode and monitor-local physical-pixel rectangle |
| pointer | Position relative to the original crop, or null |
| image | MIME, transmitted dimensions, content hash, ephemeral buffer ID |
| transform | Scale from original crop pixels to transmitted image pixels |
| redacted | Whether masks were applied before export |

The image buffer ID is a private handle, not a public URL or an arbitrary filesystem path. When an MCP host needs an image, the bridge returns encoded image content plus metadata. A schema-valid envelope is not proof of authorization; the native core must check scope containment and epoch at use time.

## MCP tool proposals

| Tool | Input concept | Output | Consent |
| --- | --- | --- | --- |
| `aeye_status` | Bound session | Current capture state and freshness, without secrets | Connection binding |
| `aeye_list_displays` | None | Display IDs, dimensions, scale; no pixels | Local user-approved client |
| `aeye_capture_focus` | Lease reference, freshness bound | One image + observation metadata | Existing active scope only |
| `aeye_capture_monitor` | Monitor lease reference | One selected-monitor image | Explicit whole-monitor lease |
| `aeye_get_recent_observations` | Lease, after-sequence, count ≤ 4 | Ordered image bundle or empty result | Active, matching lease |
| `aeye_request_watch` | Requested scope and duration | Pending request ID | Local user must approve activation |
| `aeye_stop_watch` | Bound session | Revocation result | May stop own session, never expand access |

Tools cannot start microphone recording, approve edits, capture other sessions, or receive generic filesystem/shell commands. A watch request is not an authorization token. MCP annotations are descriptive hints; enforce policy in the core.

## Agent input proposal

`UserTurn` contains turn ID, session ID, transcript text, evidence IDs, requested operation (`explain`, `investigate`, or explicit user action), and cancellation epoch. These operation labels are intent hints; actual code permissions remain in the provider's supported enforcement layer.

Submit transcript and evidence atomically at the adapter boundary where possible. If a provider requires staged upload, do not start the turn until attachments are accepted or the user explicitly chooses text-only. Fail visibly when the image cannot be processed.

## Event envelope

Every normalized event has version, event ID, session ID, turn ID if applicable, monotonic sequence, timestamp, type, and payload. Events referencing evidence include observation IDs and the capture epoch. Unknown event types can be logged as metadata and ignored safely; unknown authorization-related events fail closed.

Example categories: `capture.started`, `capture.stopped`, `evidence.ready`, `evidence.sent`, `agent.text_delta`, `agent.approval_requested`, `agent.turn_completed`, `audio.playback_stopped`, and `session.disconnected`.

## Error semantics

| Code | Meaning | Recovery |
| --- | --- | --- |
| `NO_ACTIVE_SCOPE` | Capture not authorized | Select/approve scope locally |
| `SCOPE_MISMATCH` | Request crosses lease boundary | Narrow request or ask user to change scope |
| `STALE_EPOCH` | Evidence predates revoke/switch | Discard, never retry automatically |
| `DISPLAY_CHANGED` | Topology invalidated coordinates | Stop and reselect |
| `FRAME_UNAVAILABLE` | No usable image | Show backend/permission status |
| `ADAPTER_BUSY` | Turn/queue bound reached | Coalesce evidence or explicit interruption |
| `CAPABILITY_UNSUPPORTED` | Adapter cannot honor input | Explain limitation and offer supported mode |
| `AUTH_REQUIRED` | Provider authentication unavailable | User follows supported auth flow |
| `BUDGET_REACHED` | Outward usage cap reached | Pause and show budget controls |
| `DELIVERY_UNKNOWN` | Disconnect before acknowledgement | Do not blindly replay state-changing turns |

## Bounds and versioning

Bound image dimensions, encoded bytes, message length, rate, and number of frames at every boundary. Phase 0 records provider-specific limits; use the strictest active constraint. Default recent-evidence query count is four, not an unbounded history dump.

Use idempotency IDs for local requests and retries only where the downstream operation is safely repeatable. A timeout does not prove the provider did nothing. Keep transport receipts distinct from inference completion. Store protocol fixtures free of personal or proprietary content.
