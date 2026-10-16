# Primary-source register

Reviewed **2026-10-15**. Documentation observations are not integration test results. Recheck versions and provider requirements in Phase 0 and before release.

| ID | Primary source | Evidence used | Consequence |
| --- | --- | --- | --- |
| S01 | [Codex app-server](https://learn.chatgpt.com/docs/app-server) | Sessions, streaming, approvals, images, active-turn steering | Managed Codex candidate; probe installed protocol |
| S02 | [Claude programmatic operation](https://code.claude.com/docs/en/headless) | Structured streaming and continuation | Subprocess option, not arbitrary live-terminal attachment |
| S03 | [Claude Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview) | Embedded runtime, third-party subscription restrictions, branding | Supported API auth unless separately approved; embedded label “Claude” |
| S04 | [Claude streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode) | Persistent input, images, queuing, interruption | Managed Claude candidate |
| S05 | [MCP tools, 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | Image content and schemas | Screenshots through MCP; verify each host |
| S06 | [MCP transports, 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | stdio and Streamable HTTP | Local stdio initially |
| S07 | [Claude Code MCP](https://code.claude.com/docs/en/mcp) | External tool integration | Companion tools for existing CLI |
| S08 | [OpenAI Realtime conversations](https://developers.openai.com/api/docs/guides/realtime-conversations) | Audio, image messages, function calls, interruption | Optional later voice/perception layer |
| S09 | [Windows screen capture](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture) | User-selected capture and system indicator | Initial capture backend |
| S10 | [IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired) | Border removal needs additional consent/capability | Orange may coexist with system border |
| S11 | [SetWindowDisplayAffinity](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity) | Capture exclusion and limitations | Test overlay exclusion; no security guarantee |
| S12 | [Tauri security](https://v2.tauri.app/security/) and [capabilities](https://v2.tauri.app/security/capabilities/) | Renderer/native boundary and IPC permissions | Narrow renderer commands |
| S13 | [Tauri sidecars](https://v2.tauri.app/develop/sidecar/) | Platform-specific external binaries | Packaging is a separate work item |
| S14 | [Electron desktopCapturer](https://www.electronjs.org/docs/latest/api/desktop-capturer) | Alternative capture surface and platform caveats | Fallback shell candidate |
| S15 | [whisper.cpp](https://github.com/ggml-org/whisper.cpp) | Local Whisper implementation and acceleration options | Local STT candidate; benchmark and check model licensing |
| S16 | [Apple ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit) | Official framework entry point | Future discovery only; detailed behavior not established |
| S17 | [XDG ScreenCast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html) | User-selected sessions and PipeWire streams | Linux needs portal/compositor-specific work |
| S18 | [Screenpipe docs](https://docs.screenpipe.com/home) | Adjacent screen/audio memory product | Screen-aware AI itself is not a novelty claim |

## Evidence discipline

**Documented:** primary source describes a capability. **Proposed:** A-Eye engineering choice. **Unverified:** no working A-Eye experiment established it. Record provider version, OS, auth route, capability results, and reproduction before marking an adapter supported.

This register does not certify dependency versions, entitlement, pricing, model speed, trademark availability, or performance. Pin verified dependencies during implementation.
