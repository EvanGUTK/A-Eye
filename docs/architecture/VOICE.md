# Voice architecture

## Stage one: conversational turn-taking

Implement push-to-talk, transcript finalization, agent submission, and spoken responses. This provides two-way spoken interaction without claiming native speech-to-speech or simultaneous uninterrupted full duplex.

The audio pipeline is replaceable: input device → bounded PCM buffer → resampling/VAD as needed → STT → user-visible transcript → session coordinator → agent → eligible text → TTS → selected output device.

VAD means voice activity detection; STT means speech-to-text; TTS means text-to-speech. A-Eye must never force users to learn these acronyms to use settings.

## Provider selection

Begin with one supported hosted transcription provider or a measured local alternative. Keep an interface for whisper.cpp [S15](../research/SOURCES.md), but do not promise fast local inference on every computer. Native Windows speech output is a useful baseline; optional hosted speech may improve voice quality at an explicit cost.

Choose exact models only after testing latency, recognition of code identifiers, noise handling, cancellation, licensing, and hardware needs. Do not hardcode a model advertised as “latest” into the architecture.

## Interaction rules

| Event | Behavior |
| --- | --- |
| Push-to-talk down | Anchor cursor; stop current speech playback; begin authorized microphone capture |
| Push-to-talk up | Finalize utterance; show editable transcript; prepare evidence within valid scope |
| Empty/noisy recognition | Do not submit a fabricated prompt; offer repeat or type |
| Agent streams text | Display text immediately; speak only eligible complete phrases |
| Approval requested | Stop narration and present the provider's exact action in an approval surface |
| User interrupts narration | Cancel playback and queued audio, not automatically the code task |
| Stop work | Request provider interruption and report acknowledgement separately |
| Input device disappears | Stop recording and show device recovery |
| Provider unavailable | Preserve unsent transcript for explicit retry; avoid duplicate requests |

Streaming speech must not read tool logs, chain-of-thought, code blocks, raw JSON, or secrets aloud. Prefer concise user-facing agent text. Deterministic status messages are allowed; invented progress is not. Keep the full visible response available.

## Cancellation and echoes

Each utterance and audio playback job has an ID and cancellation epoch. Discard late chunks after cancellation. A speech interruption should take effect locally before waiting for a network acknowledgement.

Initial push-to-talk avoids always-open listening. While TTS is playing, a press stops playback before recording. Hands-free listening needs acoustic echo cancellation, room/noise testing, turn detection, and clear microphone state; it is a separate milestone. A wake phrase is not a security boundary or proof that a request came from the user.

“Stop watching” can be recognized through the voice pipeline, but a global stop hotkey and visible stop button must always work without STT. Do not claim a cloud-dependent voice command provides a guaranteed emergency stop.

## Latency budget

Record hotkey-down, speech-end, transcript-final, evidence-ready, request-accepted, first-response-text, and first-audio-played. Use monotonic clocks for local durations and preserve wall-clock timestamps for human evidence review.

Provisional acceptance targets: local UI feedback within 100 ms; local playback stop within 150 ms; transcript-to-dispatch overhead under 250 ms excluding STT/provider time. These are goals, not current benchmarks. Report end-of-speech to first useful spoken answer as a measured p50/p95 distribution, with provider, network, hardware, and task noted. No guaranteed instant code diagnosis.

## Stage two: realtime voice/perception experiment

Evaluate a realtime audio model with image inputs and tool handoffs [S08]. The voice layer can remain responsive while an agent performs longer work. It must distinguish observations, hypotheses, and completed coding results.

Use explicit `task_id` and `evidence_ids` to hand work to the coding adapter. Keep one authority for code changes. A narrator must not report a successful fix before the agent result and verification arrive. A-Eye remains the owner of capture consent and routing; a realtime model cannot expand access. Measure added cost and whether the second context actually improves the workflow.
