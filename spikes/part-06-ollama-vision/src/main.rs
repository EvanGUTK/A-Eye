//! Standalone evidence-intelligence experiment; no capture, tools or coding agent.
mod ollama;

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::STANDARD};
use ollama::{Ollama, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const DEFAULT_PORT: u16 = 11435;
const DEFAULT_MODEL: &str = "qwen3-vl:2b";
const DEFAULT_RUNS: usize = 3;
const MAX_RUNS: usize = 20;
const USAGE: &str =
    "spike-06-ollama-vision [--port 11435] [--model qwen3-vl:2b] [--runs 3] [--out PATH]";
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const FIXTURES: [(&str, &[u8]); 3] = [
    ("01-dialog.png", include_bytes!("../fixtures/01-dialog.png")),
    ("02-form.png", include_bytes!("../fixtures/02-form.png")),
    (
        "03-workspace.png",
        include_bytes!("../fixtures/03-workspace.png"),
    ),
];

struct Config {
    port: u16,
    model: String,
    runs: usize,
    out: PathBuf,
}

impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut config = Self {
            port: DEFAULT_PORT,
            model: DEFAULT_MODEL.into(),
            runs: DEFAULT_RUNS,
            out: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out"),
        };
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("missing value: {flag}"))?;
            match flag.as_str() {
                "--port" => config.port = value.parse()?,
                "--model" => config.model = value,
                "--runs" => config.runs = value.parse()?,
                "--out" => config.out = value.into(),
                _ => return Err(format!("unknown option {flag}; {USAGE}").into()),
            }
        }
        if config.runs == 0 || config.runs > MAX_RUNS || config.port == 0 {
            return Err("runs must be 1..20 and port must be 1..65535".into());
        }
        if config.model.is_empty()
            || config.model.ends_with(":cloud")
            || config.model.ends_with("-cloud")
        {
            return Err("select an explicitly installed local model".into());
        }
        Ok(config)
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Fixture {
    id: String,
    file: String,
    width: u32,
    height: u32,
    sha256: String,
    ground_truth: Vec<String>,
}

fn verify_fixture(fixture: &Fixture, image: &[u8]) -> Result<()> {
    if image.len() < 24 || !image.starts_with(PNG_SIGNATURE) {
        return Err("fixture is not a PNG".into());
    }
    let width = u32::from_be_bytes(image[16..20].try_into()?);
    let height = u32::from_be_bytes(image[20..24].try_into()?);
    if (width, height) != (fixture.width, fixture.height) {
        return Err("fixture dimensions differ from manifest".into());
    }
    let digest: String = Sha256::digest(image)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if digest != fixture.sha256.to_lowercase() {
        return Err("fixture SHA-256 differs from manifest".into());
    }
    Ok(())
}

#[derive(Serialize)]
struct Sample {
    fixture: String,
    run: usize,
    client_ms: f64,
    validation_error: Option<String>,
    thinking_chars: usize,
    response: ollama::Chat,
}

fn stats(mut times: Vec<f64>) -> Value {
    if times.is_empty() {
        return Value::Null;
    }
    times.sort_by(f64::total_cmp);
    let rank = |percent: usize| times[(times.len() * percent).div_ceil(100) - 1];
    json!({"n":times.len(), "min_ms":times[0], "p50_ms":rank(50),
        "p95_ms":rank(95), "max_ms":times[times.len()-1]})
}

fn verify_manifest(fixtures: &[Fixture]) -> Result<()> {
    if fixtures.len() != FIXTURES.len() {
        return Err("expected exactly three synthetic fixtures".into());
    }
    let mut ids = HashSet::new();
    let mut files = HashSet::new();
    for fixture in fixtures {
        if !ids.insert(&fixture.id) || !files.insert(&fixture.file) {
            return Err("duplicate fixture id or file".into());
        }
        let (_, image) = FIXTURES
            .iter()
            .find(|(name, _)| *name == fixture.file)
            .ok_or("manifest refers to an unknown fixture")?;
        verify_fixture(fixture, image)?;
    }
    Ok(())
}

fn run(config: Config) -> Result<()> {
    let fixtures: Vec<Fixture> = serde_json::from_str(include_str!("../fixtures/manifest.json"))?;
    verify_manifest(&fixtures)?;
    fs::create_dir_all(&config.out)?;
    let api = Ollama::new(config.port)?;
    let version: Value = api.get("/api/version")?;
    let model = api.model(&config.model)?;
    api.require_vision(&config.model)?;
    let loaded_before = api.loaded_model(&config.model)?;
    println!(
        "Ollama {} / {} / {} bytes / {}",
        version["version"], model.name, model.size, model.digest
    );
    println!(
        "Synthetic frames only. Descriptions are untrusted model output; semantic quality requires review."
    );
    let mut samples = Vec::new();
    let mut errors = Vec::new();
    // Round robin avoids placing every simple image before every complex image.
    for run in 1..=config.runs {
        for fixture in &fixtures {
            let (_, image) = FIXTURES
                .iter()
                .find(|(name, _)| *name == fixture.file)
                .ok_or("unknown fixture")?;
            let encoded = STANDARD.encode(image);
            let start = Instant::now();
            match api.describe(&config.model, &encoded) {
                Ok(response) => {
                    let client_ms = start.elapsed().as_secs_f64() * 1000.;
                    let validation_error = ollama::validate_chat(&response, &config.model)
                        .err()
                        .map(|error| error.to_string());
                    if let Some(error) = &validation_error {
                        errors.push(json!({"fixture":fixture.id,"run":run,"error":error,"client_ms":client_ms}));
                    }
                    println!(
                        "{} run {run}: {client_ms:.1} ms, load {:.1} ms, {} tokens",
                        fixture.id,
                        response.load_duration as f64 / 1_000_000.,
                        response.eval_count
                    );
                    samples.push(Sample {
                        fixture: fixture.id.clone(),
                        run,
                        client_ms,
                        validation_error,
                        thinking_chars: response
                            .message
                            .thinking
                            .as_ref()
                            .map_or(0, |text| text.chars().count()),
                        response,
                    });
                }
                Err(error) => {
                    eprintln!("{} run {run}: FAIL {error}", fixture.id);
                    errors.push(
                        json!({"fixture":fixture.id,"run":run,"error":error.to_string(),
                        "client_ms":start.elapsed().as_secs_f64()*1000.}),
                    );
                }
            }
        }
    }
    // Preserve completed descriptions when the final diagnostic endpoint fails.
    let loaded_after = match api.loaded_model(&config.model) {
        Ok(loaded) => loaded,
        Err(error) => {
            errors.push(json!({"stage":"loaded_after", "error":error.to_string()}));
            Value::Null
        }
    };
    let per_fixture: Vec<_> = fixtures.iter().map(|fixture| {
        let selected: Vec<_> = samples.iter().filter(|s| s.fixture == fixture.id).collect();
        json!({"fixture":fixture.id,
            "all":stats(selected.iter().map(|s| s.client_ms).collect()),
            "repeat_requests":stats(selected.iter().filter(|s| s.run > 1).map(|s| s.client_ms).collect())})
    }).collect();
    let complete = errors.is_empty() && samples.len() == config.runs * fixtures.len();
    let record = json!({
        "schema_version":1, "unix_time":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        "platform":env::consts::OS, "arch":env::consts::ARCH,
        "endpoint":format!("http://127.0.0.1:{}/api/chat",config.port),
        "ollama":version,"model":model,"loaded_before":loaded_before,"loaded_after":loaded_after,
        "prompt":ollama::PROMPT,"options":{"temperature":0,"seed":42,"num_predict":ollama::OUTPUT_TOKENS,
            "num_ctx":ollama::CONTEXT_TOKENS,"think":false,"keep_alive":"5m"},
        "fixture_origin":"generated System.Drawing bitmaps; no desktop capture",
        "fixtures":fixtures,"samples":samples,"errors":errors,"latency":per_fixture,
        "inference_complete":complete,"semantic_quality":"manual review required; no automatic accuracy claim",
        "unverified":["real desktop capture integration","scope leases and cancellation",
            "OAuth coding-agent handoff","semantic change detection","continuous/concurrent capture",
            "enforced network isolation","other models/hardware","production latency targets"]
    });
    let path = config.out.join("evidence.json");
    fs::write(&path, serde_json::to_vec_pretty(&record)?)?;
    println!("Record: {}", path.display());
    if !complete {
        return Err("one or more inference requests failed; inspect evidence.json".into());
    }
    println!(
        "PASS: {} completed synthetic-image descriptions; quality is recorded separately.",
        config.runs * FIXTURES.len()
    );
    Ok(())
}

fn main() {
    if env::args().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return;
    }
    if let Err(error) = Config::parse(env::args().skip(1)).and_then(run) {
        eprintln!("FAIL: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_match_dimensions_and_hashes() {
        let fixtures: Vec<Fixture> =
            serde_json::from_str(include_str!("../fixtures/manifest.json")).unwrap();
        assert_eq!(fixtures.len(), FIXTURES.len());
        for fixture in &fixtures {
            let (_, image) = FIXTURES
                .iter()
                .find(|(name, _)| *name == fixture.file)
                .unwrap();
            verify_fixture(fixture, image).unwrap();
            let mut tampered = image.to_vec();
            *tampered.last_mut().unwrap() ^= 1;
            assert!(verify_fixture(fixture, &tampered).is_err());
        }
    }

    #[test]
    fn rejects_invalid_arguments_and_remote_image_input() {
        for args in [
            vec!["--runs", "0"],
            vec!["--runs", "21"],
            vec!["--port", "0"],
            vec!["--port", "65536"],
            vec!["--host", "https://example.com"],
            vec!["--image", "private.png"],
            vec!["--model", "test:cloud"],
            vec!["--runs"],
        ] {
            assert!(Config::parse(args.into_iter().map(String::from)).is_err());
        }
    }

    #[test]
    fn percentiles_use_nearest_rank_and_handle_no_samples() {
        assert!(stats(vec![]).is_null());
        let s = stats(vec![3., 1., 2.]);
        assert_eq!(s["n"], 3);
        assert_eq!(s["p50_ms"], 2.);
        assert_eq!(s["p95_ms"], 3.);
    }

    #[test]
    fn rejects_duplicate_fixture_coverage() {
        let mut fixtures: Vec<Fixture> =
            serde_json::from_str(include_str!("../fixtures/manifest.json")).unwrap();
        fixtures[1].id = fixtures[0].id.clone();
        assert!(verify_manifest(&fixtures).is_err());
        let mut fixtures: Vec<Fixture> =
            serde_json::from_str(include_str!("../fixtures/manifest.json")).unwrap();
        fixtures[1].file = fixtures[0].file.clone();
        assert!(verify_manifest(&fixtures).is_err());
    }

    #[test]
    fn keeps_descriptions_when_final_memory_metadata_fails() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let model = "fixture:1";
        let chat = json!({"model":model,"message":{"role":"assistant","content":"Synthetic dialog."},
            "done":true,"done_reason":"stop","total_duration":100,"load_duration":10,
            "prompt_eval_count":1,"prompt_eval_duration":20,"eval_count":1,"eval_duration":70});
        let replies = vec![
            ("200 OK", json!({"version":"mock"})),
            (
                "200 OK",
                json!({"models":[{"name":model,"digest":"synthetic","size":1,"details":{}}]}),
            ),
            ("200 OK", json!({"capabilities":["vision"]})),
            ("200 OK", json!({"models":[]})),
            ("200 OK", chat.clone()),
            ("200 OK", chat.clone()),
            ("200 OK", chat),
            ("500 Internal Server Error", json!({})),
        ];
        let join = thread::spawn(move || {
            for (status, body) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 4096];
                loop {
                    let count = stream.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let body = body.to_string();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        let out = env::temp_dir().join(format!(
            "aeye-vision-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(
            run(Config {
                port,
                model: model.into(),
                runs: 1,
                out: out.clone()
            })
            .is_err()
        );
        join.join().unwrap();
        let record: Value =
            serde_json::from_slice(&fs::read(out.join("evidence.json")).unwrap()).unwrap();
        assert_eq!(record["samples"].as_array().unwrap().len(), 3);
        assert_eq!(record["errors"][0]["stage"], "loaded_after");
        assert_eq!(record["inference_complete"], false);
        fs::remove_file(out.join("evidence.json")).unwrap();
        fs::remove_dir(out).unwrap();
    }
}
