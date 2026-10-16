use spike_03_claude_oauth::handoff::{run_stub, StubMode};
use spike_03_claude_oauth::{live, record};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Re-executed by the handoff check as the stand-in for the SDK runner.
        Some("--sdk-stub") => {
            let mode = args
                .get(1)
                .and_then(|m| StubMode::from_arg(m))
                .unwrap_or(StubMode::Clean);
            std::process::exit(run_stub(mode));
        }
        Some("--live") => std::process::exit(live::run()),
        Some("--help") | Some("-h") => {
            println!("spike-03-claude-oauth\n  (no args)  run all checks against a LOCAL MOCK and print the capability record\n  --live     opt-in real flow; needs A_EYE_OAUTH_CLIENT_ID, A_EYE_OAUTH_AUTH_URL, A_EYE_OAUTH_TOKEN_URL, A_EYE_LIVE_ACK_POLICY=1");
        }
        _ => {
            let exe = std::env::current_exe().expect("current_exe");
            let rec = record::collect(&exe);
            print!("{}", record::render(&rec));
            let failures = rec.local_failures();
            if failures > 0 {
                eprintln!("\n{failures} local check(s) FAILED");
                std::process::exit(2);
            }
        }
    }
}
