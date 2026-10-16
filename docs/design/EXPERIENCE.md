# Experience and visual system

## Direction: a compact inspection instrument

The application under inspection is the main canvas. A-Eye should occupy little space and make three facts obvious: **which agent is connected, what it can see, and whether evidence is fresh**.

Avoid an obligatory full-screen dashboard. A beautiful landing page is not a usable desktop companion. The product identity comes from a consistent focus frame, precise typography, and useful state changes.

See the [concept board](../../assets/experience-concept.svg). It is a static design reference with synthetic application content, not a working capture demonstration.

## Surfaces

| Surface | Initial size target | Content | Behavior |
| --- | --- | --- | --- |
| Compact dock | 420–520 × 64 logical px | Agent/project, microphone, scope, stop | User-positioned; never covers pointer target by default |
| Expanded conversation | 440 × 420 logical px | Recent turns, text composer, agent status | Opens on demand; does not take focus during narration |
| Evidence drawer | 360–440 px wide | Last sent crop, age, target, image sequence | Explicitly opened; compare captured versus sent |
| Scope selector | Full selected monitor overlay | Crop rectangle, coordinates, confirm/cancel | Keyboard adjustable; exits without capture on cancel |
| Session picker | Compact dialog | Project path, provider, managed/companion mode | No hidden auto-routing |
| Settings | Ordinary resizable window | Audio, providers, capture, privacy, shortcuts | Plain task labels, no analytics dashboard |

Sizes are starting constraints. Verify 100–200% scaling and keyboard enlargement before freezing layout.

## Visual tokens

| Token | Proposed value | Role |
| --- | --- | --- |
| `surface.canvas` | `#101214` | Main graphite surface |
| `surface.raised` | `#1B1E22` | Dock and drawer |
| `surface.input` | `#24282D` | Interactive input backgrounds |
| `text.primary` | `#F3F0EA` | Main text |
| `text.secondary` | `#B5B6B9` | Explanatory labels |
| `line.default` | `#3D4248` | Dividers |
| `accent.capture` | `#FF8A3D` | Active observation and focus frame |
| `status.error` | `#FF7474` | Errors; always accompanied by text |
| `status.ready` | `#9BC8AD` | Ready state; not a capture indicator |

Use a system sans-serif initially (Segoe UI on Windows) with a restrained weight scale: 400 body, 600 controls, 700 headings. Use a monospace system stack only for paths, shortcuts, durations, and coordinates. No external font download is required.

Spacing scale: 4, 8, 12, 16, 24, 32. Radius: 6 on inputs, 10 on the dock, 12 on dialogs. Thin separators carry structure. Avoid excessive rounded cards, neon gradients, glossy effects, giant avatars, or decorative gauges.

Orange is reserved for capture-related state and selected focus. Do not make every primary button orange, or the color stops communicating capture. Capture indicator is solid, not pulsing. Optional waveform motion appears only when actual audio samples are arriving and respects reduced-motion settings.

## State language

| Condition | Main label | Supporting text |
| --- | --- | --- |
| No session | Choose a coding session | Pick the project and agent to receive your requests. |
| Ready, no capture | Ready · Screen off | Hold your shortcut to speak. |
| Mic active | Listening | Release to send. |
| Scope selecting | Select what to share | Drag a region. Enter confirms; Escape cancels. |
| Region active | Watching region · Display 2 | Last sent 1.2s ago |
| Agent busy | Agent working | New visual updates are waiting. |
| Observation paused | Watching paused | No new images are being sent. |
| Scope stopped | Screen off | Previously sent images remain in the conversation. |
| Permission needed | Allow screen capture | Open the system permission flow. |
| Image failure | Image was not sent | Retry capture or send text only. |
| Provider failure | Agent disconnected | Capture paused. Reconnect to continue. |

Do not say “I can see everything,” “secure,” “private,” or “fixed” without the behavior to support it. The UI should distinguish native capture from provider transmission.

## Interaction details

**First run:** explain the workflow in one sentence; choose agent/project; explain whether managed mode requires a provider account/API key; test audio; select scope; perform a synthetic “look here” task. Ask OS permissions when the related feature is first used, not in an unexplained wall of prompts.

**Pointing:** hotkey-down anchors location. During scope selection, arrow keys move the rectangle, modified arrows resize it, Enter confirms, Escape cancels. Show the exact shortcut in the interface and make it remappable.

**Speaking:** provide push-to-talk and click-to-record alternatives. A held key cannot be the only access method. Let users inspect/edit a transcript before sending through an optional review setting. Destructive approvals never depend on an unqualified “yes” detected in background audio.

**Approvals:** show the provider's proposed action, target project, and meaningful consequence. Keep approval ownership in the provider adapter. Distinguish asking the agent to explain from approving execution. A-Eye must not silently choose an approval option.

**Capture scopes:** monitor border identifies the device; region outline identifies transmitted pixels. A small badge says “Region” or “Whole display.” Widening requires a user action. Show evidence age even when the orange line is visible.

**Error recovery:** preserve the user's unsent words when safe, explain the failure, and offer one primary recovery action. Do not restart capture or resend a possibly executed task without checking state.

## Accessibility and ergonomics

- Validate normal-text contrast ≥ 4.5:1 and control/focus contrast ≥ 3:1 for shipped color combinations.
- Keep controls at least 32 × 32 logical px in the desktop dock; offer an enlarged 44 px control mode.
- Provide names, roles, visible focus, keyboard navigation, and text alternatives to speech.
- Never rely on orange alone; include a state label and accessible status announcement.
- Avoid repeatedly announcing every frame to screen readers; announce meaningful state transitions.
- Support reduced motion, high contrast, text scaling, microphone-off usage, and remappable hotkeys.
- Keep focus in the application under inspection unless the user explicitly opens A-Eye controls.
- Do not use a centered toast to cover the bug being investigated.

## Design review questions

Can a new user name the receiving project within two seconds? Can they identify the exact capture scope? Can they stop without finding a settings page? Does the interface remain understandable without color, audio, or animation? Is the last sent image distinguishable from a local preview? Does the UI communicate the agent's actual state rather than decorative activity?

Use task recordings and observation, not a vote on whether the screen looks “modern.”
