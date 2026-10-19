# Full-stack proposal and tradeoffs

This is a desktop-first stack. A web UI framework does not require a hosted website, backend API, or user-account service.

## Selected direction

| Layer | Proposed choice | Why | Cost or validation |
| --- | --- | --- | --- |
| Shell | Tauri 2 | Native core with web-based interface | Rust/build tooling, WebView variation |
| UI | React + strict TypeScript + Vite | Familiar component model and typed contracts | Avoid recreating app state in every component |
| Styling | CSS custom properties + CSS modules | Small coherent design language | Maintain tokens and accessible states deliberately |
| UI primitives | Accessible headless primitives, selected only as needed | Keyboard/focus behavior without a template aesthetic | Verify licenses and bundle impact |
| Native runtime | Rust | Resource ownership and OS integration | Real learning/implementation overhead |
| Windows bindings | Rust `windows` crate candidate | Direct WinRT/Win32 integration | Pin against tested OS SDK/build |
| Capture | Windows.Graphics.Capture + GPU crop where feasible | Native frame acquisition | HDR, GPU device-loss, DPI, consent tests |
| Agent bridge | TypeScript in a packaged Node LTS sidecar | Official SDK ecosystem, isolated protocol churn | Ship/update runtime; signed sidecar distribution |
| Codex | app-server | Managed rich-client integration | Protocol-version compatibility |
| Claude | Agent SDK streaming input | Persistent embedded agent integration | API authentication, packaging, event differences |
| MCP | Official TypeScript SDK candidate | Tool interoperability | Negotiate supported protocol/version |
| Audio input | Native capture abstraction; CPAL candidate | Avoid tying core capture to renderer lifetime | Device changes, resampling, echo behavior |
| STT | Provider interface; hosted baseline and whisper.cpp experiment | Compare latency/quality with local option | Separate usage costs and model packaging |
| TTS | Provider interface; native Windows voice baseline | Audible first workflow without another LLM | Voice quality and streaming/cancellation behavior |
| Metadata | SQLite in native core | Simple local transactional state | Migration and sensitive-path handling |
| Secrets | Windows Credential Manager abstraction | Keep keys out of renderer/files | OS integration and account switching |
| Tests | Rust tests; Vitest candidate; Playwright for web UI only | Unit/contract/UI layers | Native behavior needs real Windows testing |
| Distribution | Windows installer, signed artifacts/update manifest | Normal desktop lifecycle | Signing cost, clean-machine tests, rollback |
| CI | GitHub Actions; Linux docs checks now, Windows app jobs later | Repeatable validation | Hosted runners do not replace interactive hardware tests |

The first [desktop integration](../../apps/desktop/README.md) now pins Tauri and packages the existing Rust MCP server over stdio, with bundled static JavaScript controls. Its lockfiles and README record tested versions. The wider React/TypeScript, provider, storage, voice, and policy stack above remains proposed; clean-machine reproduction is still a release gate.

## Why Tauri, and when to change the decision

Tauri is a reasonable fit for native capture ownership and a small companion interface. Its scoped command surface is useful [S12 in the source register](../research/SOURCES.md). It also increases implementation complexity when Rust is unfamiliar. A small installer is not worth months of stalled integration.

Compare a minimal Tauri and Electron capture/overlay prototype in Phase 0, without building two complete apps. Record development effort, idle memory, startup, capture correctness, packaging effort, and developer comfort. Choose Electron if it materially reduces delivery risk and meets the product's measured resource budget. Electron does not automatically make a UI look generic; that is a design decision.

Do not add Python as another mandatory runtime solely because it is familiar. Python can be used for research/benchmarks without being distributed in the application. Avoid a Python+Node+Rust production combination unless a specific local model dependency justifies it.

## Frontend structure

Use feature areas for dock, scope selection, sessions, evidence, approvals, and settings. Maintain shared primitives for buttons, labeled toggles, status labels, dialogs, and focus management. Provider-specific details stay behind normalized selectors/events.

Core-owned state is authoritative. The UI renders snapshots and sends commands; it does not infer whether capture stopped from a missing animation. Prefer a reducer and typed events initially. Add a dedicated state-machine library only if it simplifies actual transition complexity.

## Backend and hosting

The Rust core is the application backend. SQLite is the local data layer. Private IPC is the transport. External model/voice services are configurable integrations.

Do not add PostgreSQL, Redis, Kubernetes, a vector database, or a FastAPI server to this version. Future paid/team features could justify an authenticated hosted service, but that requires a new design for identity, retention, tenancy, and credential delegation. Those concerns are not prerequisites for visual debugging on one computer.

## Packaging contract

- Bundle exact sidecar versions per target architecture; do not download mutable executable code at launch.
- Detect missing provider CLI prerequisites and explain how to install supported versions.
- Never silently replace a user's global CLI installation.
- Verify sidecar signatures/hashes through the chosen release mechanism.
- Keep optional speech models separate, with source, size, hash, license, and explicit download.
- Preserve existing credentials and settings during updates; test metadata migration failures.
- Uninstall must stop children/overlays and explain what user data remains.
- A signing decision and release-channel design are beta gates, not something the README pretends already exists.
