# Runtime flows

These diagrams specify proposed behavior, including failure paths.

## Point and ask

```mermaid
sequenceDiagram
    participant U as Developer
    participant C as Coordinator
    participant N as Native capture
    participant A as Agent adapter
    participant V as Voice output
    U->>C: Press hotkey and speak
    C->>N: Anchor pointer and validate scope
    N-->>C: Authorized crop and timestamp
    U->>C: Release hotkey
    C->>C: Finalize transcript and bind session
    C->>A: Submit request with evidence
    A-->>C: Accepted input and response events
    C-->>U: Show evidence age and response
    C->>V: Speak eligible response text
    U->>C: Interrupt speech
    C->>V: Cancel playback immediately
```

Anchoring does not itself authorize capture. With no active lease, request local scope approval before acquisition. In single-shot mode the border appears for capture, then disappears when acquisition ends; the evidence drawer continues to identify the captured artifact.

## Watch-region lifecycle

```mermaid
stateDiagram-v2
    [*] --> Off
    Off --> Selecting: User requests watch
    Selecting --> Active: Scope approved and indicator ready
    Selecting --> Off: Cancel
    Active --> Paused: Budget or provider disconnected
    Paused --> Active: Explicit resume after validation
    Active --> Stopping: Stop or lease expiry
    Paused --> Stopping: Stop
    Active --> Stopping: Display change or overlay failure
    Stopping --> Off: Acquisition and dispatch stopped
    Off --> [*]
```

The native core owns this transition, not the model. Permission revocation, screen lock, sleep, topology invalidation, and unexpected child-process failure stop or pause according to policy. Resume never silently selects a different monitor.

## Live evidence selection

```mermaid
flowchart TD
    F[Native frame] --> S{Lease valid?}
    S -->|No| X[Discard]
    S -->|Yes| C[Crop and mask]
    C --> D{Meaningful change?}
    D -->|No| H[Heartbeat or discard]
    D -->|Yes| B[Bounded buffer]
    H --> B
    B --> G{Adapter ready and budget available?}
    G -->|No| R[Coalesce or pause]
    G -->|Yes| V[Revalidate epoch and scope]
    V --> O[Send ordered evidence]
```

Thresholds must be evaluated against tiny text changes and brief animation. Cursor motion alone should not flood evidence when the selected region is pinned.

## Stop and cancellation semantics

1. User activates stop by button or configurable global hotkey; this path never depends on cloud speech recognition.
2. Core revokes the capture lease and increments its epoch.
3. Stop acquisition, reject new requests, discard queued crops and unsent voice-associated evidence.
4. Cancel in-flight transmissions where possible. Already sent data cannot be retracted by a local stop.
5. Remove the active indicator after acquisition stops; show a local completion state.
6. Ignore stale callbacks carrying the old epoch. Keep the provider's ongoing code task separate unless the user also selects stop work.

“Stop everything” explicitly combines capture stop, microphone stop, playback cancellation, and a best-effort agent interrupt. Report any unacknowledged agent cancellation rather than claiming work stopped.

## Fix and verify

Observe → explain uncertainty → inspect source → propose action → provider approval → agent edit/test → user reproduces UI → acquire fresh evidence → compare.

Before/after comparison requires the same scope or a documented transform, matching application state, and capture timestamps. A visual improvement is not proof of all code correctness. Do not silently click or deploy to complete this loop.
