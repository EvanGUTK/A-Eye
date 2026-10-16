# Local model layer

Status: proposed. Supplements [AGENT-INTEGRATIONS.md](AGENT-INTEGRATIONS.md).

## Purpose

A-Eye uses a **local vision model** for its own internal processing — not as the
coding agent, but as the intelligence layer that decides what visual evidence is
worth sending to one. This keeps latency low, eliminates API cost for frame
analysis, and allows fully offline operation of A-Eye's capture pipeline
regardless of whether the coding agent is reachable.

The local model does not write code, approve actions, or have access to the
repository. It is a read-only visual reasoning tool owned entirely by A-Eye's
core.

## Responsibility split

```
Screen frames (raw)
        ↓
  Local model (Ollama)
  ├── Is this frame meaningfully different from the last one?
  ├── Does this region contain readable text / a UI element of interest?
  ├── Initial visual description for low-latency spoken feedback
  └── Frame scoring for evidence selection (keep / discard / burst)
        ↓
  Curated evidence bundle (selected frames + transcript)
        ↓
  Coding agent (Claude via OAuth / Codex via OAuth / Grok via API key)
  ├── Repository-aware investigation
  ├── Code inspection and editing
  └── Approval-gated actions
```

The coding agent receives only the frames the local model judged worth sending.
This reduces token usage, latency, and the risk of flooding the agent with
redundant visual updates.

## Runtime: Ollama

[Ollama](https://ollama.com) runs on Linux, exposes an OpenAI-compatible
`/v1/chat/completions` endpoint at `http://localhost:11434`, and manages model
downloads and GPU memory automatically.

Install on Arch:

```bash
# AUR
yay -S ollama
# or official install script
curl -fsSL https://ollama.com/install.sh | sh
```

A-Eye assumes Ollama is running as a local service. It does not bundle or manage
Ollama itself. First-run setup detects whether Ollama is reachable and offers
to open the install page if not.

## Model candidates

All values below are approximate and hardware-dependent. Benchmark on target
hardware before committing to defaults.

| Model | Size (VRAM) | Strengths | Weaknesses |
|---|---|---|---|
| `minicpm-v:8b` | ~6 GB | Strong on UI text, small elements, dense crops | Slower than moondream |
| `moondream2` | ~2 GB | Very fast, good for change detection | Shallow reasoning |
| `llava-phi3` | ~3 GB | Good balance | Less reliable on small UI text |
| `qwen2.5vl:7b` | ~6 GB | Best overall vision quality in class | Newest, verify support |

Recommended default for frame-change detection: `moondream2` (speed).
Recommended default for evidence description: `minicpm-v:8b` (quality).

These are starting points. Phase 0 benchmarks on Evan's GPU (RTX 4080) will
establish actual latency/quality numbers.

## Integration in the capture pipeline

The local model integrates at two points in [CAPTURE.md](CAPTURE.md)'s live
evidence selection flow:

**1. Change detector (replaces pure pixel diff)**

Instead of only comparing downscaled luminance images, the local model provides
a semantic change signal:

```
Native frame → downscale to 224×224 → moondream2 embed
                                           ↓
                               cosine similarity vs last embed
                                           ↓
                         score > threshold → meaningful change
```

This catches text changes and UI state transitions that pixel diff misses while
ignoring cursor movement and irrelevant animation.

**2. Evidence descriptor**

When a frame is selected for sending to the coding agent, the local model
generates a brief description that is prepended to the observation envelope:

```json
{
  "local_description": "Dropdown menu open; last two items clipped below visible area.",
  "local_model": "minicpm-v:8b",
  "local_model_ms": 340
}
```

This gives the coding agent immediate context without requiring it to re-reason
the image from scratch, reducing time-to-first-useful-response.

## API contract with Ollama

Use the OpenAI-compatible endpoint. No special SDK required.

```
POST http://localhost:11434/v1/chat/completions
{
  "model": "minicpm-v:8b",
  "messages": [
    {
      "role": "user",
      "content": [
        {"type": "image_url", "image_url": {"url": "data:image/png;base64,<crop>"}},
        {"type": "text", "text": "Describe what UI issue is visible in this screenshot. Be concise."}
      ]
    }
  ],
  "max_tokens": 150,
  "stream": false
}
```

The bridge calls this from the TypeScript sidecar. Keep a timeout (default 2s
for change detection, 5s for description). On timeout, treat the frame as
undescribed and send it anyway — the local model is a quality enhancement, not
a gate.

## Privacy properties

- All frames processed by the local model stay on-device
- Ollama does not phone home during inference
- The local model has no code-execution capability
- Its output is treated as untrusted data (same as OCR — see [SECURITY.md](../../SECURITY.md))
- Model weights are downloaded once from Ollama's registry; verify hash after download

## Phase 0 spike: L0.6

Capture one crop of a running UI (synthetic demo app).
Send to `minicpm-v:8b` and `moondream2` via Ollama.
Record: inference latency p50/p95, description quality on UI text,
false-positive rate on "no meaningful change" frames.
Establish GPU utilization during concurrent capture + inference.

A failed spike (too slow, too inaccurate) falls back to pure pixel diff for
change detection and omits the description field — the product still works,
just with lower evidence quality.
