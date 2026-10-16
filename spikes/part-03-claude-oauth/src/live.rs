//! OPT-IN `--live` mode. NOT run as part of this spike: a real flow needs
//! interactive consent, and Anthropic's published policy says third-party
//! developers may not offer claude.ai login or collect/store claude.ai
//! credentials (see the capability record). It exists only so an informed user
//! can try an endpoint of their choosing. It never auto-launches a browser,
//! never prints tokens, and has no hardcoded endpoint or client_id.

use crate::flow::{run_flow, FlowConfig, PRODUCT_CALLBACK_TIMEOUT};
use std::time::Duration;

pub const ENV_CLIENT_ID: &str = "A_EYE_OAUTH_CLIENT_ID";
pub const ENV_AUTH_URL: &str = "A_EYE_OAUTH_AUTH_URL";
pub const ENV_TOKEN_URL: &str = "A_EYE_OAUTH_TOKEN_URL";
pub const ENV_SCOPE: &str = "A_EYE_OAUTH_SCOPE";
pub const ENV_ACCOUNT: &str = "A_EYE_ACCOUNT_ID";
pub const ENV_ACK: &str = "A_EYE_LIVE_ACK_POLICY";

pub fn run() -> i32 {
    let need = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (Some(client_id), Some(auth_url), Some(token_url)) =
        (need(ENV_CLIENT_ID), need(ENV_AUTH_URL), need(ENV_TOKEN_URL))
    else {
        eprintln!("--live requires {ENV_CLIENT_ID}, {ENV_AUTH_URL} and {ENV_TOKEN_URL} (no defaults are built in).");
        return 2;
    };
    // Cleartext would expose the auth code + PKCE verifier and the returned tokens.
    for (name, raw) in [(ENV_AUTH_URL, &auth_url), (ENV_TOKEN_URL, &token_url)] {
        if let Err(e) = crate::endpoint::validate_live_endpoint(raw) {
            eprintln!("--live refused: {name}: {e}");
            return 2;
        }
    }
    eprintln!("WARNING: Anthropic's Claude Code legal/compliance page states third-party developers may not");
    eprintln!("offer claude.ai login or collect/store claude.ai credentials. Proceed only for personal,");
    eprintln!("experimental use with a provider/endpoint you are entitled to use.");
    if std::env::var(ENV_ACK).as_deref() != Ok("1") {
        eprintln!("Set {ENV_ACK}=1 to acknowledge and continue.");
        return 2;
    }
    let cfg = FlowConfig {
        client_id,
        auth_url,
        token_url,
        scope: need(ENV_SCOPE).unwrap_or_default(),
        callback_timeout: PRODUCT_CALLBACK_TIMEOUT,
        http_timeout: Duration::from_secs(20),
    };
    let result = run_flow(&cfg, |url| {
        println!("Open this URL in your browser (not launched automatically):\n\n{url}\n");
        println!("Waiting up to {} seconds for the callback on 127.0.0.1 ...", PRODUCT_CALLBACK_TIMEOUT.as_secs());
    });
    match result {
        Ok(t) => {
            println!(
                "token exchange OK: access_token={:?} refresh_token={:?} expires_in={:?} scope={:?}",
                t.access_token, t.refresh_token, t.expires_in, t.scope
            );
            let account = need(ENV_ACCOUNT).unwrap_or_else(|| "default".into());
            match crate::store::store_tokens(&account, &t) {
                Ok(()) => println!("stored in keyring: service=a-eye account={}", crate::store::account_name(&account)),
                Err(e) => println!("UNVERIFIED: store skipped ({e}); nothing was written anywhere"),
            }
            0
        }
        Err(e) => {
            eprintln!("flow failed: {e}");
            1
        }
    }
}
