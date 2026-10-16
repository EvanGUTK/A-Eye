//! A-Eye spike part 03: OAuth 2.0 + PKCE against a LOCAL MOCK provider.
//! No real provider is contacted unless `--live` is passed.

pub mod callback;
pub mod error;
pub mod flow;
pub mod http;
pub mod live;
pub mod mock;
pub mod pkce;
pub mod secret;
pub mod token;
