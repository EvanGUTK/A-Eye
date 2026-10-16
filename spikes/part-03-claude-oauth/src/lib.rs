//! A-Eye spike part 03: OAuth 2.0 + PKCE against a LOCAL MOCK provider, secret
//! storage via the OS keyring, and env-only token handoff to an SDK-runner
//! child process. No real provider is contacted unless `--live` is passed.

pub mod callback;
pub mod error;
pub mod flow;
pub mod handoff;
pub mod http;
pub mod live;
pub mod mock;
pub mod pkce;
pub mod record;
pub mod secret;
pub mod store;
pub mod token;
