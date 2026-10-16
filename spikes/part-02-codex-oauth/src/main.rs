//! A-Eye spike part 02: Codex-style OAuth 2.0 + PKCE flow (mock provider only).

mod pkce;
mod secret;

fn main() {
    let p = pkce::generate();
    println!("challenge={}", p.challenge);
}
