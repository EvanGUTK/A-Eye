# Linux platform architecture

Status: proposed. Primary development target is Arch Linux on Wayland.
This document supplements [SYSTEM.md](SYSTEM.md) and [STACK.md](STACK.md) with Linux-specific
platform decisions. Windows-first references in those documents are superseded here for the
active development target.

## Platform scope

Initial target: **Arch Linux, Wayland compositor** (Hyprland, Sway, KDE Plasma, GNOME).
X11 is a secondary path with a simpler capture story but no active compositor features.
macOS and Windows remain in the long-term roadmap but are not current targets.

## Screen capture

### Wayland: XDG ScreenCast portal + PipeWire

The correct capture path on Wayland is the XDG ScreenCast portal
(`org.freedesktop.portal.ScreenCast`). The portal:

- Presents a system-native consent dialog to the user (compositor-owned)
- Returns a PipeWire stream node ID for the selected monitor or window
- Enforces the user's selection boundary — A-Eye cannot silently expand it
- Works across all compliant Wayland compositors

The native capture crate uses `ashpd` (Rust D-Bus portal bindings) to request
the session, then `pipewire-rs` to consume the stream. Raw frames arrive as
DMA-BUF or shared memory buffers depending on the compositor and GPU driver.

```
User selects monitor
       ↓
ashpd::desktop::screencast::ScreenCastProxy::create_session()
       ↓
PipeWire stream (DMA-BUF / SHM frames)
       ↓
crates/capture-linux: crop, encode, hand to policy layer
```

Exact crate candidates: `ashpd 0.x`, `pipewire 0.x`. Pin tested versions in Phase 0.

### X11 fallback

On X11, use `x11rb` with `XShmGetImage` or the MIT-SHM extension for
low-overhead frame acquisition. The scope boundary is enforced by A-Eye policy
(no OS-level portal), which increases the responsibility of the scope lease
layer.

### What changes from the Windows design

| Windows | Linux equivalent |
|---|---|
| `Windows.Graphics.Capture` | XDG ScreenCast portal + PipeWire |
| `SetWindowDisplayAffinity` (overlay exclusion) | Compositor-dependent; no universal equivalent — design overlay to be visually distinct |
| System-drawn capture border | No equivalent; A-Eye draws its own indicator entirely |
| `IsBorderRequired` permission | Portal consent dialog serves this role |

The portal consent dialog is the authoritative user-approval event. A-Eye must
not start frame acquisition before the portal session is confirmed active.

## Overlay and orange border

Wayland does not allow arbitrary windows to position themselves above others
without compositor cooperation. The correct surface type for a persistent
non-interactive border overlay is **`wlr-layer-shell`** (`zwlr_layer_shell_v1`),
which is supported by wlroots-based compositors (Hyprland, Sway, river) and has
partial support on KDE and GNOME.

For compositors without layer-shell support, fall back to a borderless always-on-top
window via the `xdg_toplevel` surface with `set_maximized` + transparency hack,
clearly labeled as a best-effort fallback.

Rust crate candidate: `smithay-client-toolkit` (`sctk`) provides layer-shell
surface management. The overlay window:

- Covers the selected monitor's full extent
- Is transparent except for the 2-logical-pixel orange border line
- Does not steal focus or intercept pointer events
  (`zwlr_layer_surface_v1::set_keyboard_interactivity(none)`)
- Is excluded from capture by passing its surface ID to the portal session
  exclusion list where supported

## Credential storage

Linux equivalent of Windows Credential Manager is the **Secret Service D-Bus API**
(`org.freedesktop.secrets`), implemented by gnome-keyring and kwallet.

Rust crate: `secret-service` (sync) or `keyring` (cross-platform, async-friendly).

On a minimal Arch install without a keyring daemon (e.g. a bare WM setup),
fall back to an AES-256-GCM encrypted file at `~/.config/a-eye/credentials.enc`,
key derived via Argon2id from a user-chosen unlock passphrase.
Detect the daemon's availability at startup and clearly tell the user which
storage path is active.

Never store credentials in:
- SQLite metadata database
- Tauri/WebView local storage
- Log files or crash dumps
- Environment variables persisted to disk

## Audio

`cpal` already supports PipeWire/ALSA on Linux — no platform-specific change
required in the audio crate. Push-to-talk hotkey registration uses
`evdev`/`udev` on Wayland (no global hotkey API without compositor cooperation)
or `XGrabKey` on X11.

Global hotkey on Wayland: the only reliable cross-compositor path is reading
from the `/dev/input/eventN` device directly via `evdev-rs`, which requires
the user to be in the `input` group or a polkit rule. Surface this requirement
clearly during first-run setup. An alternative is an app-local shortcut that
only works when the A-Eye dock is focused.

## D-Bus dependencies summary

| Purpose | Interface | Crate |
|---|---|---|
| Screen capture portal | `org.freedesktop.portal.ScreenCast` | `ashpd` |
| Credential storage | `org.freedesktop.secrets` | `keyring` / `secret-service` |
| Notifications | `org.freedesktop.Notifications` | `notify-rust` |
| Power/sleep detection | `org.freedesktop.login1` | direct D-Bus via `zbus` |

## Packaging

Target: **AUR PKGBUILD** for Arch as the primary distribution channel.
Secondary: AppImage for distribution-agnostic installs.
Flatpak is a future option but adds sandboxing complexity around `/dev/input`
access for hotkeys and PipeWire portal negotiation.

The Tauri bundler supports AppImage and `.deb`; the PKGBUILD wraps the AppImage
or builds from source with Arch's Rust/Node toolchain.

No installer signing requirement before alpha (unlike Windows). Alpha gate is a
working PKGBUILD that installs and runs cleanly on a fresh Arch system.

## Phase 0 Linux spikes

See [ROADMAP.md](../planning/ROADMAP.md) for the day-by-day sequence (Linux Phase 0 section). Short summary:

1. **L0.1** XDG ScreenCast portal → PipeWire frame → PNG. Test on two monitors
   with different scales.
2. **L0.2** `wlr-layer-shell` overlay → orange border on selected monitor, no
   focus steal, no pointer interception.
3. **L0.3** libsecret / keyring storage → write + read a test credential, verify
   it does not touch disk plaintext.
4. **L0.4** `evdev` global hotkey → detect key on Wayland without window focus.
5. **L0.5** Tauri 2 + AppImage packaging → clean install on bare Arch VM.
