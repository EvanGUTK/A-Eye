# Part 02: Codex OAuth persistence (issue #10)

Both opt-in live modes now save tokens through
[part 01's credential backend](../part-01-credential-store/README.md) and load them
on the next invocation. The default no-argument run remains a synthetic OAuth
capability demonstration; it uses no live provider and deletes its temporary
keyring test entry.

## Run

Linux, Rust 1.89 or later (tested with rustc/cargo 1.93.1):

```bash
cargo build --locked --manifest-path spikes/part-02-codex-oauth/Cargo.toml
cargo run --locked --manifest-path spikes/part-02-codex-oauth/Cargo.toml -- --help
cargo test --locked --manifest-path spikes/part-02-codex-oauth/Cargo.toml -- --test-threads=1
cargo clippy --locked --manifest-path spikes/part-02-codex-oauth/Cargo.toml --all-targets -- -D warnings
```

For human-operated sign-in, follow the [live runbook](../../docs/planning/LIVE-AUTH-RUNBOOK.md).
`--live-plan-usage` uses the documented open-source route and retains its issued
client ID. `--live` still requires `A_EYE_OAUTH_CLIENT_ID`, `A_EYE_OAUTH_AUTH_URL`,
and `A_EYE_OAUTH_TOKEN_URL`; optional scope/callback settings are in `--help`.
Neither mode opens a browser automatically. A usable cached session requires no
callback listener or browser. No live sign-in or refresh was performed for this
persistence change.

## Storage and startup decisions

- Both live modes use part 01's `SecretServiceBackend`: service `a-eye`, account
  `codex:<alias>`. `A_EYE_OAUTH_ACCOUNT_ID` selects a stable local alias (1–128 ASCII
  letters, digits, `-_.`), defaulting to `default` for `--live`, `plan-usage` for
  `--live-plan-usage`. These are local selections, not claims of verified account
  identity; different registrations should use different aliases. Keep the same
  requested scopes for an alias: changing `A_EYE_OAUTH_SCOPE` requires a new alias
  and consent; cached grants are not automatically expanded.
- A version-1 JSON record retains access, refresh and ID tokens, token type, granted
  scope, `expires_in`, receipt time, token endpoint, client ID, refresh encoding and
  resource. Keeping receipt time preserves expiry across restart. Reads validate
  record version/shape; a write must read back identically before success is reported.
- Startup reuses an access token with more than 60 seconds remaining. Expired,
  nearly expired, unknown-expiry, or future-dated records are refreshed. Refresh
  does not retain the previous access-token expiry. If the response omits refresh
  token, ID token, scope or token type, the previous values are retained; replacement
  tokens and metadata are saved together.
- Only a missing entry starts browser OAuth. Store failures, invalid records,
  endpoint/client/resource mismatches, absent refresh tokens and provider failures
  return errors without deleting the stored record or silently registering another
  client. Retry transient failures; for a fresh registration select a new alias.
  No sign-out, revocation or returning browser reauthorization UI is added here.
- The live CLI holds an OS file lock at `~/.config/a-eye/codex-oauth.lock` from
  credential load through refresh/sign-in and storage readback. A concurrent CLI
  exits with a retry message. The OS releases the lock on exit/crash; the empty
  lock file remains. Rust 1.89 supplies this API without a locking dependency.
- Part 01's existing encrypted FileStore is used with temporary synthetic fixtures
  for restart tests. Live mode deliberately fails if Secret Service is unavailable:
  an unlock-passphrase UI is absent, so silently selecting a file or supplying a
  fixed passphrase would be unsafe. There is no plaintext fallback.
- Secret wrappers redact Debug output and wipe their owned strings on drop.
  Serialized storage buffers are zeroized. Refresh errors expose HTTP status and
  allowlisted error codes, never provider descriptions or arbitrary response text.
  The existing authorization-code exchange diagnostics are unchanged.
- A Secret Service timeout abandons a worker, which may complete later. Storage
  errors are fatal; no automatic retry or fallback is attempted. After a timed-out
  write, resolve/unlock the keyring and let that operation finish before retrying.

## Provider compatibility evidence

Checked 2026-10-19 using public documentation/source only:

- [OpenAI accounts and sessions](https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions)
  documents a form-encoded refresh grant at
  `https://auth.openai.com/api/accounts/oauth/token`, with the **issued** client ID,
  refresh token and `resource=https://api.openai.com/v1`; omit scope. It requires
  serialized refresh and persistence of rotating tokens. The spike implements this
  shape and tests it against a loopback mock.
- [OpenAI registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)
  documents the saved token fields and issued client ID. Existing claim validation
  must pass before this spike saves a plan-usage exchange. JWKS signature validation
  remains unimplemented; this change does not establish verified account identity.
- [Codex authentication](https://learn.chatgpt.com/docs/auth) documents cached
  credentials and automatic refresh. The legacy JSON refresh encoding is verified
  from OpenAI's [request_chatgpt_token_refresh source](https://github.com/openai/codex/blob/c3bb20602d77b0c8e62f84e9956e8086f439dfeb/codex-rs/login/src/auth/manager.rs)
  at commit `c3bb20602d77b0c8e62f84e9956e8086f439dfeb`. Access/refresh/ID tokens are
  returned as optional fields in that source; this spike requires a usable access
  token and retains omitted refresh/ID tokens. Missing expiry triggers refresh on
  the next startup rather than guessing a lifetime.

All Rust dependencies remain exactly pinned in the manifests/lockfiles. The new
path dependency reuses part 01's existing pinned crypto dependencies; `zeroize`
1.9.1 was already used by part 01. No live token request, capture, or paid service
is required for build/test validation.

## Verification scope

2026-10-19 on Ubuntu 24.04.4 / WSL2, rustc/cargo 1.93.1: both locked builds
passed; part 01 passed 39 tests, part 02 passed 79 tests (including 14 new
persistence tests), serially. Both all-target Clippy runs passed with warnings
denied. The part-02 default mock run exited 0 with 8 PASS / 0 FAIL / 6 UNVERIFIED;
its Secret Service probe found no daemon. `--help`, `git diff --check`, and
`python3 scripts/check_docs.py` passed. CodeRabbit was unavailable (`coderabbit:
command not found`); a correctness/security/concurrency/error-handling/coverage/style
self-review and independent static review found no remaining actionable issue.

Tests cover full mock OAuth exchange to durable storage, fresh store instances,
startup reuse without another browser exchange, JSON/form refresh, issued client
ID/resource binding, rotation/omitted fields, failures without record loss,
malformed records, redaction and process locking. They use only synthetic tokens,
loopback HTTP servers and temporary encrypted files. The real Linux Secret Service
integration and a real provider's restart/refresh behavior remain unverified for
this change; the live binary still requires a human to consent.

The initial unchanged 65-test suite had one parallel failure in
`callback::tests::timeout_returns_err_not_panic_and_frees_port` (the assertion that
no listener accepted a connection on a released port). Serial execution avoids
competition from other tests opening ephemeral listeners; the existing test is
unchanged.
