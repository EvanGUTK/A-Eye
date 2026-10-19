# Spike 08: local voice (whisper.cpp STT + piper TTS) on Windows

Proves one thing: on this Windows desktop, a local speech-to-text engine and a local text-to-speech
engine run offline on synthetic audio and text, and what their per-call latency is. It does **not**
touch a microphone or speaker, and it is not a voice pipeline.

## Pinned versions

| Piece | Version | Notes |
|---|---|---|
| STT engine | whisper.cpp **v1.9.5**, CI build `b5454` (commit `d1be6fde11ac6e0407606b4e42fe72d34add8037`), `whisper-bin-x64.zip`, CPU only | `whisper-cli.exe --version` reports `1.9.5`. MIT. The `v1.9.5` release has no assets; the binaries are on the `b5454` pre-release built from the same commit. |
| STT models | `ggml-base.en.bin` (primary), `ggml-tiny.en.bin` (comparison), from `ggerganov/whisper.cpp` @ `5359861c739e955e79d9a303bcbc70fb988958b1` | SHA-256 pinned in `setup.ps1`. |
| TTS engine | `rhasspy/piper` **2023.11.14-2**, `piper_windows_amd64.zip` | `piper.exe --version` reports `1.2.0`. MIT, but the repository is **archived**. |
| TTS voice | `en_US-lessac-medium` from `rhasspy/piper-voices` @ `c10ece1aade47bb51c153c893d14e5bf8e5b7117` | 22050 Hz. See the licensing note below. |

Both engines ship native Windows x64 binaries; no compiler or CMake was needed. The only Rust is the
test runner in this directory.

## Reproduce

Windows PowerShell (downloads are the only network use, each checked against a pinned SHA-256):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File spikes\part-08-voice\setup.ps1
```

This installs to `%LOCALAPPDATA%\aeye-voice-tools` (outside the repo). Then build and run with the
Windows toolchain, using the UNC-path + `CARGO_TARGET_DIR` pattern from `spikes/README.md`:

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\aeye-target"
cd \\wsl$\Ubuntu\home\usaas\projects\code\A-Eye-worktrees\voice\spikes\part-08-voice
cargo test
cargo run --release            # optional: --tools-dir PATH, --runs N (default 5)
```

Output is a list of PASS/FAIL/UNVERIFIED checks and a `SPIKE STATUS` line; evidence goes to the
gitignored `out/` (`record.json`, plus piper's WAVs). Exit code is 0 whenever a record was produced.

## Fixtures

`fixtures/*.wav` (16 kHz, 16-bit, mono, ~100-125 KB each) are **synthetic**: spoken by the offline
Windows SAPI voice "Microsoft David Desktop", with the spoken text in the sibling `.txt`.
`generate-fixtures.ps1` regenerates them. No human speech, no microphone. Negative controls (2 s of
digital silence, a 2 s 440 Hz tone) are generated in memory at run time. The sentences contain no
digits, so transcripts compare without number normalisation.

## What it checks

- **STT**: each fixture through `whisper-cli` (base.en); pass if word error rate (WER) <= 0.10. The
  8-9 word sentences therefore need zero word errors; the 11-word ones would still pass with one
  (1/11 = 0.091). `tiny.en` is run as a comparison.
- **STT latency**: wall time from process spawn to transcript, 5 runs per fixture.
- **TTS**: 5 short strings through piper to WAV; pass if valid 16-bit mono WAV, duration plausible for
  the word count, and not silent.
- **TTS latency**: spawn to first raw PCM byte on stdout, and spawn to WAV file ready, 5 runs each.
- **Round trip**: piper's audio fed back through whisper base.en; pass if WER <= 0.10 against the
  input text. This is the evidence that piper's output is intelligible speech.

"End of input" here means the whole input is available when the process starts (a complete WAV, or
text written to stdin and closed at once). Both numbers therefore include process start and model load
on every call; a resident engine would not pay that. Engine-reported load and compute times are
recorded next to the wall time.

## Results (this machine: AMD Ryzen 7 9800X3D, 16 logical CPUs, CPU inference, Windows)

The spike was run three times back to back. Latencies below are from the last run; the other two were
within about 15% (run 1's base.en p50s were 793-884 ms). Each p50 is over 5 launches per case, so the sample is small and **p95 is just the
slowest run**.

| STT, wall per call (spawn to transcript) | base.en p50 | tiny.en p50 |
|---|---|---|
| 3.2-3.9 s of speech, 4 fixtures | 726-777 ms | 399-425 ms |

Of the base.en wall time, whisper reports ~675-710 ms engine total, of which ~98 ms is model load
(so ~580-610 ms is mel/encode/decode); the rest is process start. Real-time factor ~0.2.

| TTS, 2.6-3.3 s of audio, 5 texts | p50 |
|---|---|
| spawn to first audio byte (raw stdout) | 280-336 ms |
| spawn to WAV file ready | 321-336 ms |

Of that, ~184-192 ms is piper loading the voice and ~74-92 ms is inference.

Accuracy: base.en and tiny.en transcribed all 4 fixtures with WER 0.00 in all 3 runs, including the
code-flavoured one ("Run cargo test and open the stack trace in the terminal."). Transcripts were
identical across the 5 launches of each case in runs 1 and 3 (run 2's record was overwritten; its
console output shows WER 0.00 throughout).

## Findings (read these)

1. **Round trip is not fully stable.** Piper's output varies from run to run (default noise
   parameters). In run 1, `second-screen` came back as "Open the Settings panel **in** the second
   monitor" (WER 0.125, one substitution), so that run's required round-trip check FAILED and its
   status line said "NOT proven". Runs 2 and 3 passed 5/5. The 0.10 threshold was set before the
   first run and was not changed afterwards. The pass rate is therefore 2 of 3 full runs, not 100%.
2. **whisper base.en invents words on silence.** Two seconds of digital silence transcribed as
   `you` in all 3 runs (the 440 Hz tone produced `(eerie music)`, which is a non-speech tag and passed).
   This control check failed every time. A real pipeline needs voice-activity detection or an energy
   gate before STT (see `docs/architecture/VOICE.md`); this spike does not build one.
3. **First launch is slow.** The first whisper launch of run 1 took 2900 ms against ~880 ms for the
   rest, consistent with a cold file cache / first load of the binaries and model. Later launches did
   not repeat it.
4. **Per-call model load matters for TTS.** More than half of piper's wall time is loading the voice.

## Not proven / not attempted

- **Live microphone and speaker: not attempted.** This machine routes audio through Voicemeeter and
  virtual cables, so the default devices are ambiguous, and a capture would record the user's mic.
  Real mic capture is a later concern per the task.
- **Playback and cancelling playback midway:** nothing is played; audio is only written to files.
- **Network isolation:** offline by construction (no URLs passed, nothing downloaded at run time), but
  not enforced with a firewall or observed with a packet capture.
- **Streaming/partial STT**, GPU or other hardware, other voices, noise, accents, real human speech,
  and code identifiers beyond the single "cargo test" sentence.
- **Fixture speech is synthetic and clean.** Accuracy on real microphone audio is unknown.
- The `p95` figures are not a distribution (5 samples).
- This is one machine and one run set; no claim is made for other hardware.

## Licensing and maintenance flags (not resolved here)

- `rhasspy/piper` is archived; its maintained successor `OHF-voice/piper1-gpl` is **GPL-3.0** and
  ships as a Python wheel, not a standalone CLI. Choosing between them is a product decision.
- Piper voice licences follow the voice's training dataset, not the engine's MIT licence. The lessac
  dataset is under the Blizzard 2013 licence; `ryan` and `hfc_female` are CC BY-NC-SA 4.0. Review before
  shipping any voice.
- The whisper.cpp release page's `v1.9.5` has no binaries; the pinned build is from a pre-release
  tag `b5454`. Pin the SHA-256 (done in `setup.ps1`) rather than the tag name.
