//! A-Eye spike 08: local speech-to-text (whisper.cpp) and text-to-speech (piper) on synthetic
//! fixtures, with latency measured per call.
//!
//! Everything runs locally: the spike only spawns the two installed binaries with local paths.
//! Prints a capability record and writes evidence to the gitignored `out/`.
//! Exit code is 0 whenever a record was produced, including when a check FAILs or stays
//! UNVERIFIED; read the `SPIKE STATUS` line, not the exit code.
//!
//! Usage: spike-08-voice [--tools-dir PATH] [--runs N]
//!   Tools are installed by `setup.ps1`; the default location is `%LOCALAPPDATA%\aeye-voice-tools`
//!   (override with `AEYE_VOICE_TOOLS` or `--tools-dir`).

mod run;
mod stt;
mod text;
mod tts;
mod wav;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use wav::Wav;

const DEFAULT_RUNS: usize = 5;
/// Word error rate at or below which a transcript counts as correct.
const WER_PASS_MAX: f64 = 0.10;
/// RMS (full scale = 1.0) below which generated audio is treated as silence.
const SILENCE_RMS_FLOOR: f64 = 0.01;
/// Plausible spoken duration per word, to catch truncated or runaway synthesis.
const SECS_PER_WORD_MIN: f64 = 0.10;
const SECS_PER_WORD_MAX: f64 = 1.00;
const CONTROL_RATE_HZ: u32 = 16_000;
const CONTROL_SECS: f64 = 2.0;
const TONE_HZ: f64 = 440.0;
const TONE_AMPLITUDE: f64 = 0.3;
const TOOLS_DIR_ENV: &str = "AEYE_VOICE_TOOLS";
const TOOLS_DIR_NAME: &str = "aeye-voice-tools";
const TRANSCRIPT_SHOWN_CHARS: usize = 120;

const PIN_WHISPER: &str = "whisper.cpp v1.9.5 (CI build b5454, commit d1be6fde11ac6e0407606b4e42fe72d34add8037), whisper-bin-x64.zip, CPU";
const PIN_PIPER: &str = "rhasspy/piper 2023.11.14-2 (piper_windows_amd64.zip; archived upstream)";
const PIN_VOICE: &str =
    "en_US-lessac-medium (rhasspy/piper-voices @ c10ece1aade47bb51c153c893d14e5bf8e5b7117)";

/// Sizes pinned by `setup.ps1` (which also verifies SHA-256); re-checked here as a cheap guard.
struct PinnedFile {
    label: &'static str,
    bytes: u64,
}
const MODEL_BASE_FILE: &str = "ggml-base.en.bin";
const MODEL_TINY_FILE: &str = "ggml-tiny.en.bin";
const VOICE_FILE: &str = "en_US-lessac-medium.onnx";
const VOICE_CONFIG_FILE: &str = "en_US-lessac-medium.onnx.json";
const PINNED: [PinnedFile; 4] = [
    PinnedFile {
        label: MODEL_BASE_FILE,
        bytes: 147_964_211,
    },
    PinnedFile {
        label: MODEL_TINY_FILE,
        bytes: 77_704_715,
    },
    PinnedFile {
        label: VOICE_FILE,
        bytes: 63_201_294,
    },
    PinnedFile {
        label: VOICE_CONFIG_FILE,
        bytes: 4_885,
    },
];

/// Extra TTS input with two sentences, beyond the fixture sentences.
const EXTRA_TTS: (&str, &str) = (
    "two-sentences",
    "Done. The overlay is now on the primary monitor.",
);

const USAGE: &str = "usage: spike-08-voice [--tools-dir PATH] [--runs N]";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Fail,
    Unverified,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Unverified => "UNVERIFIED",
        }
    }
}

struct Check {
    id: String,
    name: String,
    status: Status,
    detail: String,
    /// Required checks decide whether the spike's claim holds; the others are reported alongside.
    required: bool,
}

#[derive(Default)]
struct Report {
    checks: Vec<Check>,
    stt: Vec<Value>,
    controls: Vec<Value>,
    tts: Vec<Value>,
    roundtrip: Vec<Value>,
}

impl Report {
    fn add(&mut self, id: String, name: String, required: bool, status: Status, detail: String) {
        println!("  [{}] {id} {name}: {detail}", status.label());
        self.checks.push(Check {
            id,
            name,
            status,
            detail,
            required,
        });
    }

    fn judge(&mut self, id: String, name: String, required: bool, ok: bool, detail: String) {
        let status = if ok { Status::Pass } else { Status::Fail };
        self.add(id, name, required, status, detail);
    }
}

struct Stats {
    n: usize,
    min: f64,
    p50: f64,
    p95: f64,
    max: f64,
}

/// Nearest-rank percentiles. With few runs p95 is simply the slowest run.
fn stats(values: &[f64]) -> Option<Stats> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let rank = |q: f64| sorted[((q * n as f64).ceil() as usize).clamp(1, n) - 1];
    Some(Stats {
        n,
        min: sorted[0],
        p50: rank(0.50),
        p95: rank(0.95),
        max: sorted[n - 1],
    })
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn stats_json(values: &[f64]) -> Value {
    stats(values).map_or(Value::Null, |s| {
        json!({ "n": s.n, "min": r1(s.min), "p50": r1(s.p50), "p95": r1(s.p95), "max": r1(s.max) })
    })
}

fn p50_text(values: &[f64]) -> String {
    stats(values).map_or("n/a".to_string(), |s| format!("{:.0}", s.p50))
}

struct Args {
    tools_dir: PathBuf,
    runs: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut tools_dir = std::env::var_os(TOOLS_DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(TOOLS_DIR_NAME))
        });
    let mut runs = DEFAULT_RUNS;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tools-dir" => {
                tools_dir = Some(PathBuf::from(it.next().ok_or("--tools-dir needs a value")?));
            }
            "--runs" => {
                runs = it
                    .next()
                    .ok_or("--runs needs a value")?
                    .parse()
                    .map_err(|e| format!("--runs: {e}"))?;
                if runs == 0 {
                    return Err("--runs must be at least 1".into());
                }
            }
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let tools_dir = tools_dir.ok_or("no tools dir: set AEYE_VOICE_TOOLS or pass --tools-dir")?;
    Ok(Args { tools_dir, runs })
}

struct Tools {
    dir: PathBuf,
    whisper_cli: PathBuf,
    piper: PathBuf,
    model_base: PathBuf,
    model_tiny: PathBuf,
    voice: PathBuf,
}

impl Tools {
    fn locate(dir: &Path) -> Tools {
        let exe = |stem: &str| format!("{stem}{}", std::env::consts::EXE_SUFFIX);
        let models = dir.join("models");
        Tools {
            dir: dir.to_path_buf(),
            whisper_cli: dir.join("whisper").join("Release").join(exe("whisper-cli")),
            piper: dir.join("piper").join(exe("piper")),
            model_base: models.join(MODEL_BASE_FILE),
            model_tiny: models.join(MODEL_TINY_FILE),
            voice: models.join(VOICE_FILE),
        }
    }
}

/// T0: both binaries and all pinned model files are present with the pinned sizes.
fn check_inventory(report: &mut Report, tools: &Tools) -> bool {
    let mut problems = Vec::new();
    for exe in [&tools.whisper_cli, &tools.piper] {
        if !exe.is_file() {
            problems.push(format!("missing {}", exe.display()));
        }
    }
    let models = tools.dir.join("models");
    for pinned in &PINNED {
        let path = models.join(pinned.label);
        match fs::metadata(&path) {
            Ok(m) if m.len() == pinned.bytes => {}
            Ok(m) => problems.push(format!(
                "{} is {} bytes, expected {}",
                pinned.label,
                m.len(),
                pinned.bytes
            )),
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    let ok = problems.is_empty();
    let detail = if ok {
        format!("tools and models present under {}", tools.dir.display())
    } else {
        format!("{} (run setup.ps1)", problems.join("; "))
    };
    report.judge("T0".into(), "tool inventory".into(), true, ok, detail);
    ok
}

fn tool_version(exe: &Path, arg: &str) -> String {
    let mut cmd = Command::new(exe);
    cmd.arg(arg);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    match cmd.output() {
        Ok(out) => {
            let mut all = String::from_utf8_lossy(&out.stdout).into_owned();
            all.push_str(&String::from_utf8_lossy(&out.stderr));
            all.lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("(no output)")
                .to_string()
        }
        Err(e) => format!("(could not run: {e})"),
    }
}

struct Fixture {
    name: String,
    wav: PathBuf,
    expected: String,
    audio_secs: f64,
}

fn load_fixtures(dir: &Path) -> Result<Vec<Fixture>, String> {
    let mut wavs: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "wav"))
        .collect();
    wavs.sort();
    let mut fixtures = Vec::new();
    for wav_path in wavs {
        let name = wav_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let expected = fs::read_to_string(wav_path.with_extension("txt"))
            .map_err(|e| format!("{name}.txt: {e}"))?
            .trim()
            .to_string();
        let bytes = fs::read(&wav_path).map_err(|e| format!("{name}.wav: {e}"))?;
        let audio_secs = Wav::parse(&bytes)
            .map_err(|e| format!("{name}.wav: {e}"))?
            .duration_secs();
        fixtures.push(Fixture {
            name,
            wav: wav_path,
            expected,
            audio_secs,
        });
    }
    if fixtures.is_empty() {
        return Err(format!("no .wav fixtures in {}", dir.display()));
    }
    Ok(fixtures)
}

struct SttMeasure {
    transcript: String,
    identical: bool,
    wall: Vec<f64>,
    engine_load: Vec<f64>,
    engine_total: Vec<f64>,
}

fn measure_stt(
    whisper: &Path,
    model: &Path,
    wav: &Path,
    runs: usize,
) -> Result<SttMeasure, String> {
    let mut m = SttMeasure {
        transcript: String::new(),
        identical: true,
        wall: Vec::new(),
        engine_load: Vec::new(),
        engine_total: Vec::new(),
    };
    for i in 0..runs {
        let r = stt::transcribe(whisper, model, wav)?;
        if i == 0 {
            m.transcript = r.transcript;
        } else if r.transcript != m.transcript {
            m.identical = false;
        }
        m.wall.push(r.wall_ms);
        m.engine_load.extend(r.engine_load_ms);
        m.engine_total.extend(r.engine_total_ms);
    }
    Ok(m)
}

fn shorten(s: &str) -> String {
    let mut out: String = s.chars().take(TRANSCRIPT_SHOWN_CHARS).collect();
    if s.chars().count() > TRANSCRIPT_SHOWN_CHARS {
        out.push_str("...");
    }
    out
}

fn stt_section(report: &mut Report, tools: &Tools, runs: usize, fixtures: &[Fixture], work: &Path) {
    let models = [
        ("base.en", &tools.model_base, true),
        ("tiny.en", &tools.model_tiny, false),
    ];
    for (label, model, required) in models {
        println!("\n== STT latency, whisper.cpp model {label}, {runs} runs per fixture (ms) ==");
        println!(
            "  wall = spawn to transcript (input file complete at spawn; model load included)"
        );
        let mut all_wall = Vec::new();
        let mut measured = 0usize;
        let mut all_correct = true;
        for f in fixtures {
            let id = format!("S:{}:{label}", f.name);
            let name = format!("STT {} ({label})", f.name);
            match measure_stt(&tools.whisper_cli, model, &f.wav, runs) {
                Ok(m) => {
                    measured += 1;
                    all_wall.extend(&m.wall);
                    let wer = text::word_error_rate(&f.expected, &m.transcript).unwrap_or(1.0);
                    all_correct &= wer <= WER_PASS_MAX;
                    println!(
                        "  {:<14} {:>4.1}s audio  wall p50 {:>5} p95 {:>5}  engine load p50 {:>4} total p50 {:>5}  WER {wer:.2}",
                        f.name,
                        f.audio_secs,
                        p50_text(&m.wall),
                        stats(&m.wall).map_or(0.0, |s| s.p95).round(),
                        p50_text(&m.engine_load),
                        p50_text(&m.engine_total),
                    );
                    report.stt.push(json!({
                        "model": label,
                        "fixture": f.name,
                        "audio_secs": r1(f.audio_secs),
                        "expected": f.expected,
                        "transcript": m.transcript,
                        "wer": wer,
                        "transcripts_identical_across_runs": m.identical,
                        "wall_ms": stats_json(&m.wall),
                        "wall_ms_runs": m.wall.iter().map(|x| r1(*x)).collect::<Vec<_>>(),
                        "engine_load_ms": stats_json(&m.engine_load),
                        "engine_total_ms": stats_json(&m.engine_total),
                    }));
                    if required {
                        report.judge(
                            id,
                            name,
                            true,
                            wer <= WER_PASS_MAX,
                            format!(
                                "WER {wer:.2} (max {WER_PASS_MAX}); transcript \"{}\"{}",
                                shorten(&m.transcript),
                                if m.identical {
                                    ""
                                } else {
                                    "; transcript varied across runs"
                                }
                            ),
                        );
                    }
                }
                Err(e) => {
                    all_correct = false;
                    if required {
                        report.judge(id, name, true, false, e);
                    } else {
                        println!("  {:<14} FAILED: {e}", f.name);
                    }
                }
            }
        }
        if required {
            let ok = measured == fixtures.len() && !all_wall.is_empty();
            let p = stats(&all_wall);
            report.judge(
                "L1".into(),
                "STT latency recorded (base.en)".into(),
                true,
                ok,
                match p {
                    Some(s) if ok => format!(
                        "{measured} fixtures x {runs} runs: wall p50 {:.0} ms, p95 {:.0} ms, max {:.0} ms (small N: p95 is the slowest run)",
                        s.p50, s.p95, s.max
                    ),
                    _ => format!("only {measured}/{} fixtures measured", fixtures.len()),
                },
            );
        } else {
            report.judge(
                format!("S:all:{label}"),
                format!("STT all fixtures ({label}, comparison model)"),
                false,
                all_correct && measured == fixtures.len(),
                format!(
                    "{measured}/{} fixtures measured, WER <= {WER_PASS_MAX} on all: {all_correct}",
                    fixtures.len()
                ),
            );
        }
    }

    println!("\n== STT negative controls (base.en, 1 run each) ==");
    let controls = [
        ("silence", wav::silence(CONTROL_RATE_HZ, CONTROL_SECS)),
        (
            "tone",
            wav::tone(CONTROL_RATE_HZ, CONTROL_SECS, TONE_HZ, TONE_AMPLITUDE),
        ),
    ];
    for (name, audio) in controls {
        let id = format!("C:{name}");
        let label = format!("STT {name} control: no words invented");
        let path = work.join(format!("control-{name}.wav"));
        let result = fs::write(&path, audio.to_bytes())
            .map_err(|e| format!("write {}: {e}", path.display()))
            .and_then(|()| stt::transcribe(&tools.whisper_cli, &tools.model_base, &path));
        match result {
            Ok(r) => {
                let invented = text::words(&text::strip_non_speech_tags(&r.transcript));
                report.controls.push(json!({
                    "control": name, "transcript": r.transcript, "words_invented": invented.len(),
                    "wall_ms": r1(r.wall_ms),
                }));
                report.judge(
                    id,
                    label,
                    false,
                    invented.is_empty(),
                    format!(
                        "transcript \"{}\" ({} words)",
                        shorten(&r.transcript),
                        invented.len()
                    ),
                );
            }
            Err(e) => report.judge(id, label, false, false, e),
        }
    }
}

struct TtsOutput {
    id: String,
    text: String,
    wav: PathBuf,
}

struct TtsMeasure {
    file_wall: Vec<f64>,
    raw_first_audio: Vec<f64>,
    raw_wall: Vec<f64>,
    raw_bytes: usize,
    engine_load: Vec<f64>,
    engine_infer: Vec<f64>,
    engine_audio_secs: Option<f64>,
}

fn measure_tts(tools: &Tools, text: &str, out: &Path, runs: usize) -> Result<TtsMeasure, String> {
    let mut m = TtsMeasure {
        file_wall: Vec::new(),
        raw_first_audio: Vec::new(),
        raw_wall: Vec::new(),
        raw_bytes: 0,
        engine_load: Vec::new(),
        engine_infer: Vec::new(),
        engine_audio_secs: None,
    };
    for _ in 0..runs {
        let r = tts::synth_to_file(&tools.piper, &tools.voice, text, out)?;
        m.file_wall.push(r.wall_ms);
        m.engine_load.extend(r.engine_load_ms);
        m.engine_infer.extend(r.engine_infer_ms);
        m.engine_audio_secs = r.engine_audio_secs.or(m.engine_audio_secs);
    }
    for _ in 0..runs {
        let r = tts::synth_raw(&tools.piper, &tools.voice, text)?;
        m.raw_first_audio
            .push(r.first_audio_ms.ok_or("no audio byte arrived on stdout")?);
        m.raw_wall.push(r.wall_ms);
        m.raw_bytes = r.stdout_bytes;
    }
    Ok(m)
}

fn tts_section(
    report: &mut Report,
    tools: &Tools,
    runs: usize,
    fixtures: &[Fixture],
    work: &Path,
) -> Vec<TtsOutput> {
    println!("\n== TTS latency, piper voice en_US-lessac-medium, {runs} runs per text (ms) ==");
    println!(
        "  first-audio = spawn to first PCM byte on stdout; file-ready = spawn to WAV written"
    );
    println!("  (text is fully available at spawn; process start and voice load included)");
    let mut texts: Vec<(String, String)> = fixtures
        .iter()
        .map(|f| (f.name.clone(), f.expected.clone()))
        .collect();
    texts.push((EXTRA_TTS.0.into(), EXTRA_TTS.1.into()));

    let mut outputs = Vec::new();
    let mut measured = 0usize;
    let mut all_first = Vec::new();
    let mut all_file = Vec::new();
    for (id, text) in &texts {
        let check_id = format!("P:{id}");
        let name = format!("TTS {id}");
        let out = work.join(format!("tts-{id}.wav"));
        let m = match measure_tts(tools, text, &out, runs) {
            Ok(m) => m,
            Err(e) => {
                report.judge(check_id, name, true, false, e);
                continue;
            }
        };
        let wav = match fs::read(&out)
            .map_err(|e| format!("read {}: {e}", out.display()))
            .and_then(|b| Wav::parse(&b))
        {
            Ok(w) => w,
            Err(e) => {
                report.judge(
                    check_id,
                    name,
                    true,
                    false,
                    format!("output is not a valid WAV: {e}"),
                );
                continue;
            }
        };
        measured += 1;
        all_first.extend(&m.raw_first_audio);
        all_file.extend(&m.file_wall);

        let word_count = text::words(text).len() as f64;
        let secs = wav.duration_secs();
        let plausible =
            secs >= word_count * SECS_PER_WORD_MIN && secs <= word_count * SECS_PER_WORD_MAX;
        let audible = wav.rms() >= SILENCE_RMS_FLOOR;
        let ok = wav.channels == 1 && plausible && audible && m.raw_bytes > 0;
        let raw_secs = m.raw_bytes as f64 / 2.0 / wav.sample_rate as f64;
        println!(
            "  {id:<14} {secs:>4.1}s audio  first-audio p50 {:>4} p95 {:>4}  file-ready p50 {:>4} p95 {:>4}  engine infer p50 {:>4}",
            p50_text(&m.raw_first_audio),
            stats(&m.raw_first_audio).map_or(0.0, |s| s.p95).round(),
            p50_text(&m.file_wall),
            stats(&m.file_wall).map_or(0.0, |s| s.p95).round(),
            p50_text(&m.engine_infer),
        );
        report.tts.push(json!({
            "id": id,
            "text": text,
            "audio_secs": r1(secs),
            "sample_rate_hz": wav.sample_rate,
            "channels": wav.channels,
            "rms": wav.rms(),
            "peak": wav.peak(),
            "raw_stdout_secs": r1(raw_secs),
            "first_audio_ms": stats_json(&m.raw_first_audio),
            "raw_wall_ms": stats_json(&m.raw_wall),
            "file_ready_ms": stats_json(&m.file_wall),
            "file_ready_ms_runs": m.file_wall.iter().map(|x| r1(*x)).collect::<Vec<_>>(),
            "engine_load_ms": stats_json(&m.engine_load),
            "engine_infer_ms": stats_json(&m.engine_infer),
            "engine_audio_secs": m.engine_audio_secs,
        }));
        report.judge(
            check_id,
            name,
            true,
            ok,
            format!(
                "{} Hz {}ch, {secs:.2}s for {word_count} words (expect {:.1}-{:.1}s), rms {:.3} (floor {SILENCE_RMS_FLOOR}), {} raw bytes",
                wav.sample_rate,
                wav.channels,
                word_count * SECS_PER_WORD_MIN,
                word_count * SECS_PER_WORD_MAX,
                wav.rms(),
                m.raw_bytes
            ),
        );
        outputs.push(TtsOutput {
            id: id.clone(),
            text: text.clone(),
            wav: out,
        });
    }

    let ok = measured == texts.len() && !all_first.is_empty();
    let detail = match (stats(&all_first), stats(&all_file)) {
        (Some(first), Some(file)) if ok => format!(
            "{measured} texts x {runs} runs: first-audio p50 {:.0} ms, p95 {:.0} ms; file-ready p50 {:.0} ms, p95 {:.0} ms (small N: p95 is the slowest run)",
            first.p50, first.p95, file.p50, file.p95
        ),
        _ => format!("only {measured}/{} texts measured", texts.len()),
    };
    report.judge("L2".into(), "TTS latency recorded".into(), true, ok, detail);
    outputs
}

/// Feeds piper's own audio back through whisper: proof the output is intelligible speech.
fn roundtrip_section(report: &mut Report, tools: &Tools, outputs: &[TtsOutput]) {
    println!("\n== Round trip: piper audio -> whisper.cpp base.en ==");
    for o in outputs {
        let id = format!("R:{}", o.id);
        let name = format!("round trip {}", o.id);
        match stt::transcribe(&tools.whisper_cli, &tools.model_base, &o.wav) {
            Ok(r) => {
                let wer = text::word_error_rate(&o.text, &r.transcript).unwrap_or(1.0);
                report.roundtrip.push(json!({
                    "id": o.id, "text": o.text, "transcript": r.transcript, "wer": wer,
                    "stt_wall_ms": r1(r.wall_ms),
                }));
                report.judge(
                    id,
                    name,
                    true,
                    wer <= WER_PASS_MAX,
                    format!(
                        "WER {wer:.2} (max {WER_PASS_MAX}); transcript \"{}\"",
                        shorten(&r.transcript)
                    ),
                );
            }
            Err(e) => report.judge(id, name, true, false, e),
        }
    }
}

fn add_unverified(report: &mut Report) {
    let items = [
        (
            "X1",
            "live microphone and speaker",
            "not attempted: needs a real mic/speaker and a listener; this machine routes audio through Voicemeeter/virtual cables, so the default devices are ambiguous, and a live capture would record the user's mic",
        ),
        (
            "X2",
            "cancel TTS playback midway",
            "not attempted: the spike writes audio to files and never plays it",
        ),
        (
            "X3",
            "network isolation",
            "offline by construction only (no URLs are passed, nothing is downloaded at run time); not enforced or observed with a firewall or packet capture",
        ),
    ];
    for (id, name, detail) in items {
        report.add(
            id.into(),
            name.into(),
            false,
            Status::Unverified,
            detail.into(),
        );
    }
}

fn status_line(checks: &[Check]) -> String {
    if let Some(c) = checks
        .iter()
        .find(|c| c.required && c.status != Status::Pass)
    {
        return format!(
            "SPIKE STATUS: local voice NOT proven (reason: {} {} is {}: {})",
            c.id,
            c.name,
            c.status.label(),
            c.detail
        );
    }
    let passed = |prefix: &str| {
        checks
            .iter()
            .filter(|c| c.required && c.id.starts_with(prefix))
            .count()
    };
    let list = |status: Status| -> Vec<String> {
        checks
            .iter()
            .filter(|c| !c.required && c.status == status)
            .map(|c| format!("{} {}", c.id, c.name))
            .collect()
    };
    let mut line = format!(
        "SPIKE STATUS: local voice proven on synthetic fixtures (STT {} fixtures, TTS {} texts, round trip {})",
        passed("S:"),
        passed("P:"),
        passed("R:"),
    );
    let failed = list(Status::Fail);
    if !failed.is_empty() {
        line.push_str(&format!("; optional checks FAILED: {}", failed.join(", ")));
    }
    let unverified = list(Status::Unverified);
    if !unverified.is_empty() {
        line.push_str(&format!("; NOT proven: {}", unverified.join(", ")));
    }
    line
}

fn write_evidence(
    report: &Report,
    versions: Value,
    line: &str,
    outputs: &[TtsOutput],
    runs: usize,
) {
    let record = json!({
        "spike": 8,
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "processor": std::env::var("PROCESSOR_IDENTIFIER").ok(),
            "logical_cpus": std::env::var("NUMBER_OF_PROCESSORS").ok(),
        },
        "tools": versions,
        "config": {
            "runs_per_case": runs,
            "wer_pass_max": WER_PASS_MAX,
            "whisper_args": stt::ARGS_NOTE,
            "piper_args": "-m <voice> --espeak_data <dir> with -f <wav> or --output_raw",
        },
        "checks": report.checks.iter().map(|c| json!({
            "id": c.id, "name": c.name, "status": c.status.label(),
            "required": c.required, "detail": c.detail,
        })).collect::<Vec<_>>(),
        "stt": report.stt,
        "stt_controls": report.controls,
        "tts": report.tts,
        "roundtrip": report.roundtrip,
        "status_line": line,
    });
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out");
    let bytes = serde_json::to_vec_pretty(&record).unwrap_or_default();
    if let Err(e) =
        fs::create_dir_all(&out_dir).and_then(|()| fs::write(out_dir.join("record.json"), bytes))
    {
        eprintln!("could not write out/record.json: {e}");
    }
    for o in outputs {
        if let Err(e) = fs::copy(&o.wav, out_dir.join(format!("tts-{}.wav", o.id))) {
            eprintln!("could not copy {}: {e}", o.wav.display());
        }
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}");
            }
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    let tools = Tools::locate(&args.tools_dir);
    let mut report = Report::default();
    let mut outputs = Vec::new();
    let mut versions = json!({
        "whisper": PIN_WHISPER, "piper": PIN_PIPER, "voice": PIN_VOICE,
        "stt_models": [MODEL_BASE_FILE, MODEL_TINY_FILE],
    });

    println!("A-Eye spike 08: local STT (whisper.cpp) + TTS (piper), synthetic fixtures only\n");
    println!("== checks ==");
    if check_inventory(&mut report, &tools) {
        versions["whisper_reported"] = tool_version(&tools.whisper_cli, "--version").into();
        versions["piper_reported"] = tool_version(&tools.piper, "--version").into();
        println!(
            "  tool versions: {} | piper {}",
            versions["whisper_reported"].as_str().unwrap_or(""),
            versions["piper_reported"].as_str().unwrap_or("")
        );
        let work = std::env::temp_dir().join("aeye-spike-08");
        let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        match fs::create_dir_all(&work)
            .map_err(|e| format!("create {}: {e}", work.display()))
            .and_then(|()| load_fixtures(&fixtures_dir))
        {
            Ok(fixtures) => {
                stt_section(&mut report, &tools, args.runs, &fixtures, &work);
                outputs = tts_section(&mut report, &tools, args.runs, &fixtures, &work);
                roundtrip_section(&mut report, &tools, &outputs);
            }
            Err(e) => report.judge("F0".into(), "fixtures".into(), true, false, e),
        }
    }
    add_unverified(&mut report);

    let line = status_line(&report.checks);
    println!("\n{line}");
    write_evidence(&report, versions, &line, &outputs, args.runs);
}
