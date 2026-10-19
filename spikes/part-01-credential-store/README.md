# Part 01: credential store

Standalone Linux credential-store spike, also exposed as the
`spike_01_credential_store` Rust library for [part 02](../part-02-codex-oauth/README.md).
The library exports the existing `backend::CredentialBackend`,
`backend::SecretServiceBackend`, and `filestore::FileStore`; the binary and part 02
use these same implementations.

Secret Service uses service `a-eye` through pinned `keyring` 4.2.0, with a 15-second
operation timeout. FileStore uses the existing AES-256-GCM/Argon2id encrypted format,
private file permissions, and atomic replacement. Its caller supplies the unlock
passphrase; no production default passphrase exists in the library. FileStore
read/modify/write operations require caller serialization for concurrent writers.

From the repository root:

```bash
cargo build --locked --manifest-path spikes/part-01-credential-store/Cargo.toml
cargo test --locked --manifest-path spikes/part-01-credential-store/Cargo.toml
cargo clippy --locked --manifest-path spikes/part-01-credential-store/Cargo.toml --all-targets -- -D warnings
```

The existing evidence binary (`cargo run` with the same manifest) uses only
synthetic canaries and a temporary encrypted file. One existing test probes the
local Secret Service with a synthetic canary; no OAuth provider is involved.
A missing daemon does not prove that a real desktop keyring works. See the
[spike status](../README.md) for the earlier keyring evidence and limitations.
