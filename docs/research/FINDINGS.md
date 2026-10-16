# Research findings and feasibility

The concept is plausible. Build around **local capture and a session coordinator**, with MCP as an interoperability surface. Same-session integration, latency, platform behavior, and authentication expectations are the largest risks. None has been experimentally closed in this documentation foundation.

References S01–S18 resolve in the [source register](SOURCES.md).

## Findings

1. **MCP is not the whole voice layer.** Image-returning tools fit screen inspection [S05]. Tool availability does not automatically create a channel for injecting spoken user turns into the host. The coordinator and MCP server have separate jobs.
2. **Managed sessions are the controllable path.** Use Codex app-server [S01] and Claude SDK streaming input [S04] behind separate adapters. Resuming transcript history is not proof of safe concurrent control of a live terminal. Start with one owner per session.
3. **Authentication changes the promise.** Anthropic limits third-party use of claude.ai login/subscription capacity without approval [S03]. Do not market embedded mode as free use of an existing subscription. Existing CLI + MCP and embedded API sessions are different routes. Confirm Codex auth and distribution requirements separately. Voice billing is also separate.
4. **Realtime perception is optional.** Realtime audio/image/tool interaction is documented [S08]. A separate narrator adds another context, bill, and synchronization problem. Introduce it only if measured latency justifies it.
5. **Windows supports the capture building block.** Windows Graphics Capture is the initial candidate [S09]. System-border removal has separate permission requirements [S10]. Keep the first build compatible with the OS border; test overlay exclusion [S11].
6. **Desktop frameworks do not remove native work.** Tauri provides scoped IPC [S12], while sidecars need packaging [S13]. Electron is an alternative [S14]. Neither removes monitor, DPI, audio, hotkey, or signing work. Time-box the choice.
7. **Live has three rates.** Acquisition, evidence delivery, and reasoning are separate. Expose freshness and handle backpressure. A buffer reduces some misses but cannot guarantee capturing arbitrarily short glitches.
8. **Differentiation remains a hypothesis.** Screen-memory products such as Screenpipe are adjacent [S18]. Validate intentional pointing plus coding-session continuity in actual debugging tasks.

## Feasibility matrix

| Capability | Assessment | Missing proof |
| --- | --- | --- |
| Cursor crop through MCP | Strong documentation basis | Image fidelity in both target hosts |
| Live region capture | Strong platform basis | DPI, overlays, resource use |
| Speech and spoken reply | Feasible composition | Latency, noise handling, cancellation |
| Same managed conversation | Documented primitives | Adapter tests and auth |
| Attach to any running terminal | Not established | Supported ownership mechanism per provider |
| Understanding every video frame | Not promised | Model/transport and workload evaluation |
| Selected-monitor orange border | Feasible UI goal | OS coexistence, fullscreen, HDR, topology |
| Fully offline product | Not initial scope | Local vision/coding and audio stack evaluation |
| macOS/Linux parity | Deferred | Dedicated platform spikes |

## Research limits

This review did not reproduce Codex native voice internals, validate undocumented realtime CLI interfaces, measure hardware latency, test API keys, or establish multi-client safety. Architecture diagrams are proposals, not implementation evidence. Apple documentation was surveyed only at its framework entry point.
