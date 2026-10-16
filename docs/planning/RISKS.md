# Risk register

Likelihood and impact are planning judgments, not measured probabilities.

| Risk | Likelihood / impact | Mitigation and exit evidence |
| --- | --- | --- |
| Users expect arbitrary live-terminal attachment | High / high | Separate companion/managed modes; prove supported attachment before advertising |
| Provider protocol changes | High / high | Pin tested versions, capability probes, adapter fixtures |
| Subscription billing assumption fails | High / high | Verify auth route; separate embedded API mode and user's CLI |
| Agent slower than screen changes | High / high | Bound/coalesce queue, show freshness, event bundles |
| Mixed-DPI mis-targets capture | Medium / critical | Physical coordinate model, property tests, native matrix |
| Stop races allow queued evidence out | Medium / critical | Epoch revocation and final dispatch check |
| Overlay hides or captures itself | Medium / high | Native spike, watchdog, self-capture detection |
| Voice misrecognizes identifiers | High / medium | Editable transcript, project vocabulary experiment, text fallback |
| TTS echo causes repeated input | Medium / high | Push-to-talk first, cancel-before-record, later AEC testing |
| Screen text prompt injection | High / high | Treat pixels/OCR as untrusted data; preserve provider permissions |
| Unintended secrets in images | Medium / critical | Narrow scopes, explicit preview/masks, no automatic expansion |
| Resource use overwhelms host | Medium / high | Crop early, byte caps, measured acquisition throttling |
| Rust/sidecar complexity stalls delivery | Medium / high | Time-box shell choice; Electron fallback; avoid extra runtimes |
| Second narrator contradicts coding agent | Medium / high | Defer; one coding authority; correlate completed results |
| Generic-looking UI obscures purpose | High / medium | Dock-first design, all state compositions, task review |
| Name is hard to search or conflicts | Unknown / medium | Name research before product launch; no clearance assumed |

## Decisions still needing evidence

- Tauri versus Electron after native/packaging spikes.
- Exact supported Windows baseline and GPU requirements.
- Which managed adapter ships first, based on authentication and working image/session proofs.
- Hosted versus local transcription baseline and output voice.
- Image-size/rate defaults after provider and readability tests.
- Whether overlay capture exclusion is reliable enough on supported configurations.
- Whether users need optional retained evidence or prefer ephemeral operation.
- Whether a realtime narrator improves completion time enough to justify complexity.

Resolve these with short experiments and update the [decision record](../decisions/README.md). Do not bury unresolved assumptions in code comments.
