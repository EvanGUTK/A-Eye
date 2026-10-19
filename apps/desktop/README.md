# A-Eye desktop

First runnable Windows app milestone, issue [#35](https://github.com/EvanGUTK/A-Eye/issues/35).
The Tauri window hosts the existing [part-09 server](../../spikes/part-09-mcp-stdio/README.md)
as a local child over stdin/stdout. Choose a monitor, request/remove its orange border,
and capture one PNG into the window. Connection and display status are refreshed every two seconds.
This is the first desktop integration, not another spike or the complete visual companion.

`apps/desktop/` follows the [intended repository layout](../../docs/architecture/REPOSITORY.md).
`src-tauri/` is the composition root and small MCP host; `src/` contains embedded static assets.
It reuses [part 16](../../spikes/part-16-windows-shell/README.md)'s Tauri/bundler pins and icons.
Static HTML/CSS/JavaScript avoids a new frontend toolchain for this slice. React/TypeScript,
shared capture crates, and the wider proposed package hierarchy remain future work.

## Build and try it

Use **native Windows** PowerShell, cargo/rustc and Node. From WSL, invoke `powershell.exe`:

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\aeye-target-desktop"
Set-Location '\\wsl$\Ubuntu\home\usaas\projects\code\A-Eye-worktrees\integration\apps\desktop'
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\build.ps1
Start-Process "$env:CARGO_TARGET_DIR\release\aeye-desktop.exe"
```

`build.ps1` builds the unchanged part-09 release executable, stages it as
`src-tauri/binaries/aeye-mcp-x86_64-pc-windows-msvc.exe`, installs the pinned local Tauri CLI
if absent, then calls `node node_modules\@tauri-apps\cli\tauri.js build` directly.
`npm install` works over UNC; `npm run` and `npx` go through cmd.exe and fail over UNC.
The local Windows target directory avoids Cargo's UNC incremental-lock error. Builds default
to two jobs to limit memory pressure. PowerShell `Resolve-Path` must use `.ProviderPath`
when passing UNC paths to Cargo; `.Path` can include a provider prefix Cargo cannot resolve.
Use `-NoBundle` to skip installers.

Outputs under `$env:CARGO_TARGET_DIR\release`:

- `aeye-desktop.exe` **and adjacent `aeye-mcp.exe`**: both are required for standalone launch.
- `bundle\nsis\A-Eye_0.1.0_x64-setup.exe` and `bundle\msi\A-Eye_0.1.0_x64_en-US.msi`.

The sidecar is packaged by Tauri `externalBin`; no mutable executable is downloaded at launch.
Installers are unsigned and skip WebView2 installation, as in part 16: an existing WebView2
runtime is required. No global package install is needed. Do not commit generated executables.

In the window, wait for **Connected · local stdio**, select a display, click **Show orange
overlay**, then **Capture one frame**. The image appears below with its decoded dimensions
and capture timestamp. **Hide overlay** removes the requested border; **Disconnect** closes
the child and clears the preview. Closing the window shuts down the sidecar. To stop the app,
close its window. Ctrl+Alt+Q removes borders locally, independently of UI/protocol responsiveness.

There is **no browser URL, TCP listener, dev server, or LAN web server**. Assets use Tauri's
in-process virtual origin `http://tauri.localhost/`; that is not a hosted site. The CSP permits
only bundled scripts/styles, PNG data previews, and native IPC. The renderer receives no shell
or filesystem plugin permissions; its sole command accepts a closed set of desktop actions.

## Controls and limits

- The child is started with `--allow-capture`. This is part 09's coarse process-lifetime gate,
  not a scope lease. Only the explicit **Capture one frame** action calls the capture tool;
  polling and borders do not capture pixels. Each image covers the whole selected display,
  downscaled to at most 1568 pixels on its longest side, with no cursor drawn.
- Captures remain in memory in the app, and are never sent to a provider or deliberately saved.
  Disconnect, reconnect, display changes, another capture, or Hide clear the preview. Screen
  pixels and sidecar output are untrusted data. This is not a claim about OS paging or WebView caches.
- Borders are independent visual controls, expire after 60 seconds, and require the existing
  native hotkey registration. Another server holding Ctrl+Alt+Q can prevent overlay creation.
  The UI says **Overlay requested**: part 09 has no notification when the hotkey closes it early.
  Hide resolves/clears that handle; there is no fabricated live border indicator.
- One native worker serializes actions through a bounded queue. Replies have a 12-second
  deadline and a 16 MiB framing cap; image bytes are capped at 8 MiB. A protocol failure
  disconnects and closes the child; reconnect is explicit. No capture is automatically retried.
  Closing stdin allows native overlay cleanup, with a two-second exit grace and kill fallback.
- Display IDs belong to one child lifetime. Status refreshes topology; capture/overlay actions
  re-enumerate and reject changed geometry, and previews validate the returned monitor/PNG size.
  Removed/replaced displays require selection again. Hide the current border before switching.
- Scope leases/epoch revocation, cropping, agent/session integration, continuous observation,
  voice, persistence, authentication, and provider calls are not implemented in this app.

## Local verification

After the build has staged the sidecar (needed by Tauri's build script):

```powershell
cargo test --locked --manifest-path src-tauri\Cargo.toml
cargo clippy --locked --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path src-tauri\Cargo.toml -- --check
node --check src\app.js
node --test tests\app.test.mjs
$env:AEYE_TEST_SIDECAR = "$env:CARGO_TARGET_DIR\release\aeye-mcp.exe"
cargo test --locked --manifest-path src-tauri\Cargo.toml --test windows_host -- --ignored --nocapture --test-threads=1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\verify.ps1 -Exe "$env:CARGO_TARGET_DIR\release\aeye-desktop.exe" -KeepRunning
```

The ignored test is deliberately opt-in: it captures real displays into memory and requests
borders. `verify.ps1` uses Windows UI Automation to click the actual bundled window, assert
image decode, exercise disconnect/crash/reconnect/window-close, and inspect process-tree TCP/UDP
endpoints. It closes its test instance; `-KeepRunning` then launches a fresh demo window.
Optional `-Screenshot <local.png>` records **private app-window pixels**; do not commit or upload
them. From the repository root also run `python3 scripts/check_docs.py`.

### Compatibility and evidence (2026-10-18)

Pins reused from the proven spikes: tauri 2.12.4, tauri-build 2.7.3, Tauri CLI 2.12.3,
base64 0.22.1; serde 1.0.229 and serde_json 1.0.154 are locked/pinned. Existing dependency
families only (MIT/Apache-2.0); no shell plugin or JS runtime dependency was added.
Build environment: Windows cargo/rustc 1.99.0, Node 26.1.0, npm 11.13.0. The exact resolved
dependency graph is in the two lockfiles.

Verified on the Windows desktop (four monitors, one portrait; all 100% scale):

- `build.ps1` run over this worktree's UNC path, including recreation of the missing staging
  directory: release executable plus unsigned NSIS (1,743,755 bytes) and MSI (2,543,616 bytes).
  These builds used `%TEMP%\aeye-target-16` to reuse the proven packaging cache.
- 12 native tests, 22 frontend tests, native clippy with `-D warnings`, Rust formatting, and
  JavaScript syntax checks passed. Synthetic tests cover changed geometry before capture,
  sidecar exit/timeout, protocol bounds/errors, command validation, FIFO, and stale previews.
- The opt-in real-host test passed capture and overlay show/hide on **all four monitors**,
  matching original image dimensions, refresh, disconnect and reconnect. It did not inspect
  PNG contents visually or simulate mixed DPI.
- Windows UI Automation drove the actual release window: connection, selected primary display,
  orange-border request, **decoded 1568 × 882 preview**, hide, disconnect, forced child crash,
  explicit reconnect, and close with an overlay requested all passed. The child exited on
  Disconnect and window close. The app-window preview was also inspected locally by eye;
  private evidence was not committed.
- A standalone copy of only the two executables in a fresh local Windows directory passed
  the same UI checks, including immediate Show/Hide and Connect/Disconnect ordering.
- CodeRabbit CLI 0.9.0 was invoked twice on the local diff with a process-scoped Git
  `safe.directory` override. Both attempts were rate-limited (all ten included reviews used);
  **no CodeRabbit findings or completed review were obtained**, and paid reviews were not enabled.
  A focused local code review found and resolved staging, topology, and action-ordering issues.
- The tested app/sidecar/WebView2 process tree (nine processes) had **zero TCP endpoints and
  zero UDP endpoints** during three release-window checks. This is a point-in-time observation,
  not proof that WebView2 never opens UDP.

Remaining release gates:
mixed DPI, monitor hotplug/lock/sleep acceptance, installed/clean-machine execution, signing,
MSI install, forced-parent-crash cleanup, and the full visual-companion exit demo. Part 16's
unexplained intermittent WebView2 UDP endpoint is not assumed fixed; verification reports UDP
separately and gates on TCP endpoints.

Primary references checked 2026-10-18: [Tauri external binaries](https://v2.tauri.app/develop/sidecar/),
[Tauri native commands](https://v2.tauri.app/develop/calling-rust/),
[MCP lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle),
[stdio transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports).
Only MCP 2025-11-25 is supported by this small host. No provider API was implemented.
