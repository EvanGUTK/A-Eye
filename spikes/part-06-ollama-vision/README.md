# Part 06: local Ollama vision

Standalone Rust experiment for ADR015/017 **evidence intelligence**: synthetic
UI pixels become a text description through local Ollama. No coding agent,
OAuth, repository access, desktop capture, tools, or automatic actions are involved.

**Tested 2026-10-18:** 9/9 completed descriptions across three synthetic images
on Windows, Ollama 0.31.2, `qwen3-vl:2b` (1,889,519,687 bytes). Warm request p50
was 1.195 s (dialog), 0.779 s (form), 1.579 s (workspace). The initial unloaded
request took 50.59 s and truncated with the original 180-token budget; the final
run used 512. See the [results and quality review](results/README.md).

## Reproduce on Windows

Ollama was already installed. Its Windows server was stopped; a separate WSL
server on port 11434 was left untouched. This experiment uses Windows port 11435.
No installer, admin rights, global settings, or paid API were needed.

From Windows PowerShell, set the worktree path for your checkout:

```powershell
$spike = '\\wsl$\Ubuntu\home\usaas\projects\code\A-Eye-worktrees\vision\spikes\part-06-ollama-vision'
$env:OLLAMA_HOST = '127.0.0.1:11435'
$env:OLLAMA_NO_CLOUD = '1'
ollama --version
# Run in a separate PowerShell window with the same two environment variables:
ollama serve
```

In the original window (skip the pull if the exact model is already installed):

```powershell
ollama list
ollama pull qwen3-vl:2b
& "$spike\generate-fixtures.ps1"
$env:CARGO_BUILD_JOBS = '1'
$env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'aeye-vision-target'
cargo run --locked --manifest-path "$spike\Cargo.toml" -- --runs 3
```

The Windows cargo target stays off UNC because incremental locks can fail there.
On Linux with a local Ollama server, the same standalone crate works with
`cargo run --locked -- --port 11434` from this directory; Linux inference was
not benchmarked. PNGs are embedded at compilation, so regenerate **before** building.
Fixture rendering needs Windows PowerShell/System.Drawing and Segoe UI/Consolas;
different Windows/font versions may change hashes and pixels together.

Output is `out/evidence.json` (gitignored). The binary fails on request errors,
truncated/empty descriptions, missing vision capability, or invalid fixture hashes.
It preserves successful samples if the final memory-metadata request fails.
A successful exit proves completed inference, **not semantic accuracy**.

## Method and boundaries

- Three hand-drawn synthetic UI PNGs, generated with System.Drawing; no screen
  reading. Their [manifest](fixtures/manifest.json) records dimensions, SHA-256
  and visible ground truth. Rendering twice produced identical bytes here.
- Each request has one image and the same neutral prompt. Ground truth and
  filenames are never put into the model prompt. No conversation history or tools.
- Native `POST /api/chat`, raw base64 image, `stream:false`, `think:false`,
  temperature 0, seed 42, 512 output tokens, 8192 context tokens, keep-alive 5 min.
  Native REST was chosen instead of the proposed architecture's OpenAI-compatible
  endpoint to obtain load/prefill/generation timing fields directly.
  This tested model emitted reasoning despite `think:false`; `thinking_chars`
  records its presence without retaining reasoning text. Token counts and latency
  include that work. Disabling reasoning is **not proven**.
- Three round-robin passes (9 calls); client latency includes request serialization,
  transport, inference and JSON parsing, excludes PNG rendering/base64 encoding.
  Raw Ollama durations are retained in nanoseconds. First-request load and repeat
  requests are distinguished; repeated identical images may benefit from caches.
- Nearest-rank p50/p95: with only 3 samples per fixture, p95 is the maximum.
  These are exploratory measurements, not a population latency estimate.
- Endpoint is fixed to numeric loopback; HTTP proxy use and redirects are disabled.
  Cloud-tag names and remote-model metadata are rejected; local weight size and
  digest are required. Server cloud support was disabled for this run. Network
  isolation was not enforced or packet-traced.
- Model descriptions are untrusted evidence, saved as JSON strings. They are
  never executed or interpreted as permission. The binary accepts no image path
  or capture input, so it cannot accidentally read a private desktop image.
- No cancellation, scope-lease handling, frame selection/change detection,
  concurrent capture, provider handoff, or application integration is proven.
  The broader L0.6 two-model comparison and pixel-diff comparison remain untested.

## Verification

Portable checks from the repository root:

```bash
cargo test --locked --manifest-path spikes/part-06-ollama-vision/Cargo.toml
cargo clippy --locked --manifest-path spikes/part-06-ollama-vision/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path spikes/part-06-ollama-vision/Cargo.toml --check
python3 scripts/check_docs.py
```

Use the same cargo commands in Windows PowerShell with `CARGO_TARGET_DIR` above.
Tests use synthetic loopback HTTP fixtures, never a live model. They cover
malformed/oversized/error responses, redirects, cloud aliases, absent/text-only
models, empty/incomplete/truncated output, argument bounds, fixture integrity,
distinct fixture coverage and percentile arithmetic.

Actual local verification: 12 tests passed on WSL and Windows; clippy with
`-D warnings` and formatting checks passed. Documentation validation passed.
CodeRabbit CLI 0.9.0 was invoked on the local diff before the PR, but returned
a rate limit (all 10 included reviews used); no review findings were produced
and paid credits were not enabled. An independent local agent review was used
to inspect correctness and evidence boundaries; it does not replace CodeRabbit.

Dependencies reuse pinned repository versions (`reqwest`, `serde`, `serde_json`,
`base64`, `sha2`), each MIT/Apache-2.0 compatible; no Ollama SDK is needed.
`reqwest` provides bounded blocking HTTP/JSON; TLS features are unnecessary for
this loopback-only experiment. No dependency versions elsewhere were changed.

## Primary compatibility sources

Reviewed 2026-10-18; exact tested runtime and model identity are recorded with
the results. Model weights are downloaded separately, not redistributed here.

- [Ollama vision input](https://docs.ollama.com/capabilities/vision)
- [Native chat and timing fields](https://docs.ollama.com/api/chat)
- [Installed model sizes/digests](https://docs.ollama.com/api/tags)
- [Model capabilities](https://docs.ollama.com/api-reference/show-model-details)
- [Loaded models and VRAM](https://docs.ollama.com/api/ps)
- [Generation options](https://docs.ollama.com/modelfile)
- [Windows per-user installation](https://docs.ollama.com/windows)
- [qwen3-vl:2b model, Apache-2.0](https://ollama.com/library/qwen3-vl:2b)
- [Cloud-model behavior](https://registry.ollama.com/blog/cloud-models)
