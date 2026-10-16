# A-Eye Spikes

Each part is a standalone Rust binary proving exactly one capability.
No part depends on another being merged — integration happens after all spikes pass.

Status values: `todo`, `merged (<what is proven>)`. A merged spike lists what was **not** proven;
a merged spike is not a finished capability. Details are in each spike's capability record
(`cargo run --manifest-path spikes/part-XX-<name>/Cargo.toml`) and in its PR.

| Part | Spike | Status | Issues |
|---|---|---|---|
| 01 | Credential store (keyring + fallback) | merged (#17): Secret Service and encrypted-file fallback proven; default keyring on the test machine is unencrypted (SS-05 FAIL) | #3 #4 #5 #6 closed |
| 02 | Codex OAuth PKCE | merged (#16): PKCE, loopback callback, token exchange and keyring store proven against a **mock** provider; live Codex flow unverified | #7 #8 closed; #9 #10 open |
| 03 | Claude OAuth PKCE | merged (#19): PKCE, token exchange and env-only SDK token handoff proven against a **mock** provider; live flow and real SDK session unverified; Anthropic policy risk recorded (F01) | #11 #12 open |
| 04 | XDG ScreenCast + PipeWire frame | merged (#18): real frame captured via portal and saved as PNG on Hyprland; monitor id/scale (M1) and two-monitor different-scale (M2) unverified | #14 closed; #13 #15 open |
| 05 | wlr-layer-shell orange border | todo | |
| 06 | Ollama vision (local model) | todo | |
| 07 | evdev global hotkey | todo | |
| 08 | Voice: whisper.cpp STT + piper TTS | todo | |
| 09 | MCP stdio server (7 tools) | todo | |
| 10 | Scope leases + epoch revocation | todo | |
| 11 | Coordinate transforms + crop pipeline | todo | |
| 12 | Observation envelope validation | todo | |
| 13 | Session registry + binding | todo | |
| 14 | Codex app-server managed session | todo | |
| 15 | Claude SDK managed session | todo | |
| 16 | Tauri shell + AppImage | todo | |

## What is still unverified (parts 01-04)

- Live OAuth against Codex (part 02) and Claude (part 03): needs interactive browser consent on a real
  subscription. Part 03's live use also needs Anthropic approval or a switch to the permitted path
  (see the F01 finding in its capability record and PR #19).
- Real Claude Agent SDK session turn and whether the SDK accepts a subscription token (part 03).
- Selected-stream monitor id/scale and the two-monitor different-scale capture (part 04).
- Secret Service with a password-protected keyring, kwallet, and the locked-keyring GUI unlock prompt (part 01).
- Captures from part 04 are written to a gitignored `out/` folder and may contain private screen content.

## Rules for spike code
- Each spike is `cargo run` from its own directory
- No shared crates until a spike graduates to the real app
- Output is evidence: print what you proved, print what failed
- Each spike has its own Cargo.toml (workspace member later)
