# Windows results, 2026-10-18

**Proven:** local Ollama returned completed UI descriptions for all three
generated synthetic frames, in three round-robin passes (9/9). A manual
comparison against each PNG found the main visible error/state correctly
described in every final sample. This is a small feasibility experiment, not
an accuracy benchmark or production capture pipeline.

## Exact environment

- Windows native executable, Rust/cargo 1.99.0; portable checks also ran on WSL
  Rust/cargo 1.93.1. Build jobs limited to 1; Windows target directory under
  `%LOCALAPPDATA%\aeye-vision-target` (outside the UNC worktree).
- GPU: NVIDIA GeForce RTX 4080 SUPER, 16,376 MiB; NVIDIA driver 617.42.
- Windows Ollama client/server 0.31.2, dedicated `127.0.0.1:11435`, server started
  with process-scoped `OLLAMA_NO_CLOUD=1`. Windows client initially found no
  server; `ollama list` after starting it showed no installed vision model.
  WSL's separate Ollama 0.31.2 instance had vision models and was left untouched.
- Explicitly downloaded official `qwen3-vl:2b`, Apache-2.0, Q4_K_M, 2.1B parameters
  according to the installed metadata. Artifact **1,889,519,687 bytes** (1.89 GB,
  1.76 GiB); digest
  `0635d9d857d497aeadba3d7d27485746c50554446f9f6ec01ef39788221adbe8`.
- `/api/ps` reported **2,521,238,076 bytes VRAM** (2.35 GiB), context 8192.
  This is Ollama's reported loaded-model allocation, not independently measured
  peak usage or total GPU memory. No concurrent-capture measurement was made.
- Existing Ollama installation reused; no install/admin/global configuration
  changes. Model weights remain in Ollama's Windows model store.

## Latency and descriptions

[Final raw evidence](windows-2026-10-18.json) retains all samples, descriptions,
model identity, PNG hashes, timing counters and memory metadata. All final
requests started with the model loaded. Client request durations:

| Synthetic frame | Dimensions | n | p50 | p95/max | Description review |
| --- | --- | --- | --- | --- | --- |
| [Error dialog](../fixtures/01-dialog.png) | 640×360 | 3 | 1195.0 ms | 1364.9 ms | Correct Upload failed, Network unavailable, Retry/Cancel |
| [Validation form](../fixtures/02-form.png) | 960×640 | 3 | 779.0 ms | 795.9 ms | Correct signup, demo.invalid, Invalid email address, masked password and gray button |
| [Build workspace](../fixtures/03-workspace.png) | 1280×800 | 3 | 1578.8 ms | 1623.6 ms | Correct sidebar, files, build-duration graph, E0425/cannot find value total, Build failed |

These p95s are nearest-rank maxima of only three observations. Repeated images
and prompts can benefit from caches; they do not establish latency for fresh
desktop frames. Descriptions were identical across the final three passes per
fixture here, which does not establish deterministic behavior generally.

Example model output for the form:

> The UI shows a "Create account" dialog. The Email field contains "demo.invalid", displaying "Invalid email address" in red. The Password field has masked characters. A gray "Create account" button is present.

Review limits: the form description omits the synthetic label and does not
explicitly say the gray button looks disabled. The workspace calls its file
list/graph "controls"; interactivity cannot be established from these pixels.
No false-cause claim was observed. Review was manual against the same fixtures,
with one reviewer; OCR completeness, robustness and general accuracy are untested.

## Failed first attempt and diagnosed adjustment

[Attempt 01](attempt-01.json) used the same prompt/images, a 180-token cap,
and `think:false`. Only 3/9 calls (the form) completed. The dialog reached
`done_reason:length`; the workspace returned empty content. The first dialog
request began with no loaded model and took **50,590.6 ms**, including cold
startup and computation. The original runner discarded rejected responses,
so that request's server timing breakdown is unavailable; the overhead cannot
be attributed to model loading alone. Failed-call wall times are retained.

A [diagnostic replay](diagnostic-dialog.json) showed the model emitting a
reasoning field despite `think:false`, consuming the output budget before its
description finished. Reasoning text is omitted; its length is recorded.
The final cap was raised to **512 tokens**, leaving headroom for the observed
reasoning plus the same requested 90-word description. Empty/truncated outputs
still fail validation. The runner now retains rejected descriptions and counts
reasoning characters; the final run emitted 220/121/312 tokens per dialog/form/
workspace, including reasoning. No fixture/prompt was changed to reveal answers.

## Decision and unverified work

ADR015/017's narrow description path is feasible on this hardware: local pixels
can produce a useful error description independently of the coding-agent layer.
This experiment does **not** establish a default production model or satisfy all
of the older L0.6 roadmap: `minicpm-v`/`moondream` comparison, semantic change
detection, false positives, pixel-diff comparison and concurrent capture remain
unverified. Also unverified: real frame acquisition/lease integration, stop and
cancellation, fresh-frame continuous latency, enforced offline networking,
other hardware and OAuth coding-agent handoff. No private screen was captured.
