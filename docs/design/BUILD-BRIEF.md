# UI implementation brief

Use this brief when building the interface. It defines a product-specific layout rather than asking a coding model to “make a modern AI dashboard.”

## Assignment

Build a compact desktop inspection companion named A-Eye. The user's application remains visible and usable. Create the dock first, then scope selection, then the evidence drawer. Use the [experience spec](EXPERIENCE.md) as the source of truth.

The dock's visual hierarchy is: **receiving agent/project → capture state → talk action → stop**. The evidence drawer's hierarchy is: **last sent image → age and scope → user request → agent response**. The drawer is not a stream of unrelated cards.

## Required compositions

1. **Idle:** graphite dock, explicit project, Screen off, microphone shortcut hint.
2. **Listening:** actual audio activity indicator, editable transcript expansion, no fake waveform.
3. **Watching:** solid orange monitor border and matching crop outline; dock labels display and region; sent age visible.
4. **Agent busy:** subtle textual progress and queued-evidence state; no perpetual thinking orb.
5. **Approval:** clearly separated provider action with approve/deny controls and target project.
6. **Disconnected:** capture paused, retry action, unsent text recoverable.

## Constraints

Use the token palette, one type family plus monospace metadata, and a consistent spacing scale. No purple/blue gradient hero, floating glass cards, fake stats, sidebar with six empty navigation categories, chat avatar parade, or decorative “AI sparkle” buttons. Do not put developer implementation terminology into normal controls.

Orange is functional. The product can look distinctive without more effects. Keep the outer monitor line thin and solid. Avoid darkening the entire user's screen. Do not animate the capture border.

## Build sequence

First implement deterministic UI state fixtures, including missing permissions and stale evidence. Review keyboard focus and 200% scaling. Then connect to mocked typed core events. Only after the states are coherent, attach the real core. A mock must be visibly labeled and must never request screen/microphone permissions.

Do not confuse a screenshot of a beautiful idle state with a validated product. Review all six compositions, multiple monitors, long project names, translated labels, and errors.

## Acceptance

The interface should answer “who receives this?”, “what is shared?”, “how fresh is it?”, and “how do I stop?” without opening settings. A reviewer should be able to perform the primary workflow with only keyboard and text. The screen under inspection should remain the dominant visual surface.
