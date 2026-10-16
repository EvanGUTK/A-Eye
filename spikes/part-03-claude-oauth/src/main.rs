use spike_03_claude_oauth::live;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--live") => std::process::exit(live::run()),
        _ => println!(
            "spike-03-claude-oauth\n  --live  opt-in real flow; needs A_EYE_OAUTH_CLIENT_ID, A_EYE_OAUTH_AUTH_URL, A_EYE_OAUTH_TOKEN_URL, A_EYE_LIVE_ACK_POLICY=1\n(capability record arrives with the #12 commit)"
        ),
    }
}
