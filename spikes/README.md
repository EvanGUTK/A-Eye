# A-Eye Spikes

Each part is a standalone Rust binary proving exactly one capability.
No part depends on another being merged — integration happens after all spikes pass.

Status values: `todo`, `paused`, `done on branch <name>, not merged (<what is proven>)`, `merged (<what is proven>)`. A spike lists what was **not** proven;
a merged spike is not a finished capability. Details are in each spike's capability record
(`cargo run --manifest-path spikes/part-XX-<name>/Cargo.toml`) and in its PR.

| Part | Spike | Status | Issues |
|---|---|---|---|
| 01 | Credential store (keyring + fallback) | merged (#17): Secret Service and encrypted-file fallback proven; default keyring on the test machine is unencrypted (SS-05 FAIL) | #3 #4 #5 #6 closed |
| 02 | Codex OAuth PKCE | merged (#16): PKCE, loopback callback, token exchange and keyring store proven against a **mock** provider; live Codex flow unverified. On branch `agents/sessions`, not merged: `--live-plan-usage` for OpenAI's documented open-source route, synthetic-mock tested; real binary reached the real token endpoint with a fake code (see [runbook](../docs/planning/LIVE-AUTH-RUNBOOK.md)) | #7 #8 closed; #9 #10 open |
| 03 | Claude OAuth PKCE | merged (#19): PKCE, token exchange and env-only SDK token handoff proven against a **mock** provider; live flow and real SDK session unverified; Anthropic policy risk recorded (F01, re-read 2026-10-18, unchanged); no live run is prepared, see [runbook](../docs/planning/LIVE-AUTH-RUNBOOK.md) | #11 #12 open |
| 04 | XDG ScreenCast + PipeWire frame | merged (#18): real frame captured via portal and saved as PNG on Hyprland; monitor id/scale (M1) and two-monitor different-scale (M2) unverified | #14 closed; #13 #15 open |
| 05 | wlr-layer-shell orange border | paused (Linux-only overlay; see platform pivot below) | |
| 06 | Ollama vision (local model) | done on branch `agents/vision`, not merged: Windows Ollama 0.31.2 + `qwen3-vl:2b` (1,889,519,687 bytes), 9/9 completed descriptions of 3 generated synthetic UI frames; warm p50 0.779–1.579 s, initial unloaded request 50.59 s and truncated at original token cap; reasoning emitted despite `think:false`; [results](part-06-ollama-vision/results/README.md). Real capture integration, change detection, model comparison, cancellation and enforced offline isolation **unverified** | #36 open |
| 07 | [Windows global hotkey](part-07-windows-hotkey/README.md) (Linux evdev L0.7 paused) | done on branch `agents/hotkey`, not merged (draft PR #39; CodeRabbit rate-limited): native Windows Ctrl+Alt+A and custom Ctrl+Alt+7 toggle part-18's orange overlay while a separate PowerShell/WinForms application keeps foreground focus (injected keyboard events, 3 composited pixel probes); second instance reports Win32 error 1409 and exits 3; exit-hotkey/timeout cleanup and hotkey reuse proven. Physical keyboard, arbitrary apps, elevated/fullscreen apps, other layouts, mixed DPI, Ctrl+C and external WM_CLOSE **unverified** | #37 open |
| 08 | Voice: whisper.cpp STT + piper TTS | done on branch `agents/voice`, not merged (#27): on Windows, offline, 4 synthetic SAPI-speech fixtures transcribed with 0 word errors by whisper.cpp 1.9.5 (base.en wall p50 ~0.73-0.88 s per ~3.5 s clip, tiny.en ~0.40-0.48 s) and 5 strings synthesised by piper 1.2.0 (first audio ~0.28-0.39 s, WAV ready ~0.32-0.38 s; both include per-call model load); piper-to-whisper round trip passed 5/5 in 2 of 3 runs (1 word substituted in the other); **whisper base.en invented "you" on 2 s of silence in all 3 runs**; live mic/speaker, playback cancel, enforced network isolation, real human speech **unverified**; piper is archived and voice dataset licences need review | #27 open |
| 09 | MCP stdio server (5 tools over Windows capture + overlay) | done on branch `agents/mcp-bridge`, not merged (#28): hand-written MCP 2025-11-25 server over stdio only (no network listener; zero TCP/UDP endpoints on the process), tools list/capture/overlay start+stop/cursor; capture is off unless started with `--allow-capture` (no scope leases yet); live run through the real binary: capture size matches, orange border seen in a capture of **all 4 monitors** (one portrait), colour param, expiry, injected Ctrl+Alt+Q, stop on stdin close, second server refuses while hotkey held; Claude Code 2.1.303 completed the handshake and called `aeye_list_monitors`. **Unverified:** a host rendering the image result, Codex CLI as host, mixed-DPI (all monitors 100%), borders not inspected by eye, external WM_CLOSE path | #28 open |
| 10 | Scope leases + epoch revocation | todo | |
| 11 | Coordinate transforms + crop pipeline | todo | |
| 12 | Observation envelope validation | todo | |
| 13 | Session registry + binding | todo | |
| 14 | Codex app-server managed session | todo | |
| 15 | Claude SDK managed session | todo | |
| 16 | Tauri 2 shell as a Windows exe (was Linux AppImage; see [part-16-windows-shell](part-16-windows-shell/README.md)) | done on branch `agents/packaging`, not merged (#26): Tauri 2.12.4 shell bundled to unsigned NSIS and MSI installers; standalone and NSIS-installed exe launch as a real window (non-zero `MainWindowHandle`, title set, page inspected by eye) loading only bundled assets, with 0 TCP sockets in the process tree and no new system-wide TCP listener; NSIS install and uninstall work; an intermittent WebView2 wildcard UDP socket was seen in 2 launches and is **unexplained**; MSI install, signing, clean-machine (WebView2 absent), AppImage/Linux untested | #26 open |
| 17 | Windows.Graphics.Capture monitor frame | done on branch `feat/spike-17-18-windows`, not merged (#23): real frame of each of 4 monitors (one portrait) captured via windows-rs and saved as PNG (size matches the monitor, not uniform); monitor handle, device, DPI scale and cursor position recorded; two-monitor different-scale (M2) **unverified** (all 4 monitors are at 100%); PNG contents not inspected by eye | #23 open |
| 18 | Win32 layered-window orange border overlay | done on branch `feat/spike-17-18-windows`, not merged (#24): layered, click-through, non-activating, topmost window drew a border over the primary monitor (composited pixel read back as the orange); foreground window unchanged, with a control showing an ordinary window does change it; click-through and local close (timeout and injected Ctrl+Alt+Q) proven; Ctrl+C and external WM_CLOSE stop paths **unverified**; the `--region` argument, non-primary monitors and non-100% scales untested; border not inspected by eye | #24 open |

## Platform pivot (2026-10-18)

Development moved from a Linux/WSL machine to a real Windows desktop. Per the proposed stack
("Windows Graphics Capture first"), native capture/overlay work now targets Windows first.
Parts 05-16 above are Linux/cross-platform work and are **paused, not deleted** — they resume
once the Windows capture+overlay pair is proven. Parts 17-18 are the Windows equivalents of the
already-merged part 04 (capture) and the paused part 05 (border overlay): Windows.Graphics.Capture
API for frame capture, and a Win32 layered/transparent, non-activating topmost window for the
orange border, in place of XDG portal/PipeWire and wlr-layer-shell.

Build note: the repo lives in the WSL filesystem (`~/projects/code/A-Eye`), but native Windows
binaries must be compiled with the Windows-side `cargo`/`rustc` (confirmed present: cargo 1.99.0).
Build via `powershell.exe` against the UNC path `\\wsl$\Ubuntu\home\usaas\projects\code\A-Eye\...`.
Cargo's default incremental-compilation lock fails over a UNC path ("could not create session
directory lock file"); set `CARGO_TARGET_DIR` to a local Windows path (e.g. `$env:TEMP\aeye-target`)
to avoid it. Confirmed working with `cargo check --target x86_64-pc-windows-msvc`.

## What is still unverified (parts 01-04)

- Live OAuth against Codex (part 02) and Claude (part 03): needs interactive browser consent on a real
  subscription. Codex is one command once a human is ready (`docs/planning/LIVE-AUTH-RUNBOOK.md`); the
  parameters the real authorize endpoint accepts can only be checked in a browser (Cloudflare challenge). Part 03's live use should wait for Anthropic's approval or a switch to the permitted path
  (see the F01 finding in its capability record and PR #19).
- Real Claude Agent SDK session turn and whether the SDK accepts a subscription token (part 03).
- Selected-stream monitor id/scale and the two-monitor different-scale capture (part 04).
- Part 01 against the real login keyring once it is password-protected (it is unencrypted on the test machine), kwallet,
  and the locked-keyring GUI unlock prompt. An encrypted, unlocked private gnome-keyring did pass 17/17 checks.
- Captures from part 04 are written to a gitignored `out/` folder and may contain private screen content.

## Rules for spike code
- Each spike is `cargo run` from its own directory
- No shared crates until a spike graduates to the real app
- Output is evidence: print what you proved, print what failed
- Each spike has its own Cargo.toml (workspace member later)
