# Capture, pointer focus, and live evidence

## Scope is an enforceable boundary

The native core creates a lease containing a session binding, monitor identity, authorized rectangle, expiry, and monotonically increasing epoch. Tools may request narrower regions. A model cannot expand the lease, switch monitors, or renew it without local user action. The visible border and every outgoing image derive from the same authoritative scope.

An OS backend may acquire a whole monitor surface before cropping. The product must describe this accurately: **the native layer acquires the selected display; only the selected crop is passed onward**. Never claim excluded pixels were never acquired if the backend cannot guarantee that. Mask/crop before compression, retention, logs, and provider dispatch.

## Coordinate model

Maintain three explicit spaces: virtual desktop physical pixels, monitor-local physical pixels, and renderer logical coordinates. Never mix them implicitly. Monitors can have negative origins, rotation, fractional scaling, and unequal DPI.

For an unrotated monitor with physical origin `(mx, my)`, an anchored desktop point `(px, py)` maps to `(px-mx, py-my)` locally. For crop origin `(cx, cy)` in that monitor, the local pointer is `(px-mx-cx, py-my-cy)`. Apply the declared capture rotation and image resize transform afterward. Store transforms rather than inferring them from an image's displayed size.

Use outward rounding when mapping user selections to pixels, then clamp to the approved monitor bounds. Recompute or revoke on DPI/resolution/rotation changes. Never silently stretch an old crop onto a replacement display.

## Pointing behavior

- Pin the pointer at hotkey-down, not at the end of transcription or delayed tool execution.
- Initial crop candidate: 640 × 480 physical pixels, user-adjustable and clamped at edges. This is a starting value, not a universal ergonomic optimum.
- Show a preview so “look here” is not ambiguous.
- Keep unannotated evidence plus explicit pointer metadata; a small annotated companion image may help, but must not cover the defect.
- Pinned region remains fixed while the user opens menus or moves the cursor.
- Follow-pointer is a separate later mode with hysteresis; do not enable it automatically.
- Crossing onto an unauthorized monitor pauses pointer-follow capture and asks for a scope change.
- Offer keyboard region selection and a center-of-active-window fallback.

## Orange border specification

Use a solid orange line, nominally 2 logical pixels, adjusted for DPI. It belongs only to the selected monitor. Add a rectangular focus outline and a short scope label. Avoid a large blurred glow, continuous pulse, or spotlight dimming by default.

Create overlays that do not activate the window or intercept application clicks. The stop control is a separate interactive surface. Investigate excluding the overlays from capture [S11](../research/SOURCES.md); if unreliable, use a backend strategy that prevents self-capture without hiding the indicator while capture is active. Do not let overlay pixels drive change detection.

Windows can draw a separate system capture border [S09–S10]. Preserve it in the initial build. Custom orange-only appearance is conditional on supported, user-consented OS behavior. Fullscreen, exclusive graphics modes, secure desktop, and protected content may be unsupported; display a clear failure instead of a fabricated image.

## Live sampling proposal

All values below are experiment defaults, not measured performance claims.

| Stage | Initial experiment | Bound |
| --- | --- | --- |
| Local acquisition | Up to 10 FPS while watch is active | Reduce under resource pressure |
| Change detector | Small downscaled luminance image plus region comparison | Include text-change evaluation fixtures |
| Normal outward updates | Up to 1 selected frame/second | Budget and adapter readiness can reduce it |
| Explicit interaction burst | Up to 3 selected frames/second for 3 seconds | User-triggered; not a permanent setting |
| Bounded memory | Up to 3 seconds or 32 MiB of cropped evidence | Whichever limit occurs first |
| Evidence request | At most 4 selected frames | Total payload cap negotiated per adapter |
| Watch lease | 5 minutes, visible remaining time | User renews; configurable within policy |

Keep an initial frame, meaningful changed frames, and an ending frame for a short demonstration. Use monotonic sequence numbers and timestamps. For a slow agent, coalesce routine updates to the newest frame while preserving a user-marked event sequence. Do not continuously start new agent turns just because the screen changed.

The buffer only starts after consent. No hidden pre-roll. It is not a full-fidelity video recorder. It may miss short events; surface that limitation and offer a user-triggered higher-rate diagnostic mode later.

## Resource model

A 1920 × 1080 four-byte frame is 8,294,400 bytes, about 7.91 MiB. Thirty raw frames require about 237 MiB. A 640 × 480 crop is about 1.17 MiB; even thirty of those exceed 32 MiB. Therefore enforce a byte budget in addition to time/frame counts. GPU surfaces, temporary encodes, and native frame pools also consume memory outside the retained evidence budget.

Approximate outbound bandwidth as `selected_frames_per_second × mean_encoded_bytes`. At one 150 kB frame/second, that is about 9 MB/minute; this is an illustrative assumption, not a measured codec result. Provider cost depends on its image/token accounting, not merely bytes.

## Freshness and quality

Display “captured 0.2s ago” separately from “last sent 3.1s ago.” “Sent” means accepted by the adapter transport, not reasoned over. Show “waiting for agent” when delivery is deferred.

Prefer lossless or high-quality crops for small UI text. Downscale only after verifying readability. Detect blank frames and permission loss. HDR-to-SDR conversion, color accuracy, cursor inclusion, and rotated monitors require native tests. OCR may assist selection later; it is not an authority for hidden application structure.

## Failure handling

Monitor removal, lock, sleep, overlay failure, or lease expiry stops capture. Adapter failure pauses it by default. If the native core crashes, child processes lose their pipe and terminate; a watchdog must prevent a detached capture process continuing without UI. Stop works independently of provider availability. Already transmitted images are outside local revocation control.
