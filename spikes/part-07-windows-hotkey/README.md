# Spike 07: Windows global hotkey

Standalone Windows equivalent of paused Linux **L0.7 evdev global hotkey on Wayland**
([original scope](../../docs/planning/ROADMAP.md#phase-0--linux-spikes-paused-2026-10-18-see-current-status-above)). Issue: #37.
This demonstrates an overlay action only: **no screen capture, microphone, agent, or provider**.
Linux `/dev/input/eventN`, input-group/polkit permissions, and push-to-talk remain paused.

## Build and try

From WSL, use native Windows cargo/rustc through PowerShell. Build directly against this
worktree's UNC path, with output on a local Windows drive to avoid the UNC incremental-lock
failure (see [build note](../README.md#platform-pivot-2026-10-18)):

```bash
powershell.exe -NoProfile -Command '$env:CARGO_TARGET_DIR = "$env:TEMP\aeye07-target"; $env:CARGO_BUILD_JOBS = "1"; Set-Location "\\wsl$\Ubuntu\home\usaas\projects\code\A-Eye-worktrees\hotkey\spikes\part-07-windows-hotkey"; cargo build --locked'
```

In a Windows PowerShell terminal:

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\aeye07-target"
Set-Location \\wsl$\Ubuntu\home\usaas\projects\code\A-Eye-worktrees\hotkey\spikes\part-07-windows-hotkey
& "$env:CARGO_TARGET_DIR\debug\spike-07-windows-hotkey.exe" --hotkey Ctrl+Alt+A --hold-secs 120
```

Wait for the JSON `ready` line, then switch to another application. Press **Ctrl+Alt+A** to
show the primary-monitor border; press it again to hide it. The other application should keep
focus. **Ctrl+Alt+Q exits**, including while the border is hidden. Ctrl+C in the launching
console also exits; the entire process expires after 120 seconds by default (range 1–600).
The timeout starts at readiness and is not renewed by toggling. Nothing is initially visible.

Start the same executable in a second terminal while the first is running. It should print
`ERROR_HOTKEY_ALREADY_REGISTERED (1409)` on stderr and exit **3**, without a `ready` line or an
overlay. It checks both the toggle and fixed exit chord before creating a window, so part-18
or part-09 holding Ctrl+Alt+Q also prevents startup. A startup failure releases any registration
already acquired by this process. Stop the conflicting process before trying again.

`--hotkey` accepts Ctrl, Alt and/or Shift plus one ASCII letter or digit, case-insensitively,
for example `Ctrl+Alt+7` or `Ctrl+Shift+A`. Duplicated modifiers, unmodified keys, unsupported
keys, and a toggle matching the fixed exit chord are rejected with exit **2**. Other runtime
errors exit **1**; clean local exits return **0**. stdout is line-delimited JSON evidence;
stderr carries errors. There is no network service or browser URL.

## Verification

Run native Windows commands from the same PowerShell directory/environment:

```powershell
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
# Deliberate desktop test: changes foreground focus and injects keyboard events.
# Run with the desktop unlocked and hands off the keyboard.
cargo test --locked --test live_hotkey -- --ignored --nocapture --test-threads=1
```

The six portable unit tests cover defaults, chord normalization, unsupported/ambiguous input,
the reserved exit chord, missing/invalid arguments, bounded timeout and help. The live test is
ignored by default because it needs an interactive Windows desktop. It runs the real binary,
launches a separate PowerShell/WinForms process with synthetic text, checks that the fixture's
PID owns the foreground HWND, and injects balanced keyboard events with `SendInput` only
after the child reports successful registration. It does **not** post `WM_HOTKEY` directly.
It samples three composited border pixels using part-18's `BitBlt`/`CAPTUREBLT` technique,
checks the HWND both immediately and every 10 ms during a 350 ms settling interval, and
checks window visibility. No screenshot or window title is retained in evidence.

Tested on 2026-10-18: Windows 11 Pro **10.0.26200**, primary monitor **2560×1440**,
Windows cargo **1.99.0 (5f94df478 2026-08-27)** / rustc **1.99.0 (b940084d7 2026-09-28)**,
pinned `windows` **0.62.2**, dependencies fixed in Cargo.lock.

Local CodeRabbit CLI review was attempted before the PR; it returned a rate-limit error
(all 10 included reviews used, next slot reported in 10 minutes). No review findings were
obtained and paid usage was not enabled. The PR remains a draft pending that review.

Observed in the native live test:

- Border initially OFF; default chord ON → OFF → ON with three exact orange probes per ON
  state and non-orange probes per OFF state. The foreign foreground HWND stayed unchanged.
- A second real process failed promptly with exit 3 and the specific Win32 error 1409;
  the first instance continued toggling normally.
- A different-toggle instance was refused because the fixed exit chord was held; its
  partially acquired toggle registration was released and could be registered immediately.
- Ctrl+Alt+Q removed an active overlay, destroyed its HWND and preserved foreground focus.
- Custom Ctrl+Alt+7 toggled ON → OFF with the same pixel and focus checks.
- A new instance could register Ctrl+Alt+A after exit; a three-second timeout destroyed its
  active overlay. Both toggle and exit chords could then be registered directly again.

**Unverified:** physical keyboard presses (all presses above were injected), arbitrary other
applications, elevated apps/UIPI, games/exclusive fullscreen, secure desktop, lock/sleep,
other keyboard layouts/AltGr, physical autorepeat, Ctrl+C and external WM_CLOSE at runtime,
non-primary monitors and mixed DPI. The border was checked by pixel values, not by eye.
The first console-based fixture could not expose a separate HWND under the WSL pseudoconsole;
the successful fixture uses a separate PowerShell GUI process instead. The hotkey proof does
not establish capture scope leases, recording, push-to-talk, or product integration.

## Reuse and design

`src/win/overlay.rs` adapts only monitor bounds, window creation/styles and alpha border painting
from [part-18](../part-18-windows-overlay/src/win.rs), following
[part-09's documented copy rationale](../part-09-mcp-stdio/README.md#decisions). Spikes are
bin-only and cannot depend on shared crates before graduation. Part-18 is untouched; its
self-test and capability runner are not copied. Differences: a unique window class, hidden
initial window, show/hide toggles, checked bitmap allocation/GDI handles and RAII destruction.
The six-physical-pixel `rgb(255,140,0)` border deliberately preserves the proven spike renderer;
it is not the final DPI-adjusted product design. Consolidate these primitives at graduation.

The new runner registers both chords against **NULL HWND**, binding `WM_HOTKEY` to the
calling thread's message queue, independent of overlay visibility or focus. `MOD_NOREPEAT`
requests repeat suppression. Painting/showing uses `WS_EX_NOACTIVATE`, `SW_SHOWNOACTIVATE`
and `SWP_NOACTIVATE`; it never calls `SetForegroundWindow` or restores focus to conceal a
change. Hotkey/window guards clean up on normal exit and startup failure. The console handler
only latches a flag; the owning thread handles windows and unregisters hotkeys.

Primary API references checked 2026-10-18:
[RegisterHotKey](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey),
[WM_HOTKEY](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-hotkey),
[UnregisterHotKey](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-unregisterhotkey),
[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput).
