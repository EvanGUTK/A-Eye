# OAuth and provider authentication

Status: proposed. Supplements [AGENT-INTEGRATIONS.md](AGENT-INTEGRATIONS.md).

Corrections from spikes 02 and 03 (2026-10-15) are marked **Verified from source** or
**Unverified**. Both spikes ran against a local mock provider only; no live Codex or
Claude sign-in has been performed. "Verified from source" means read in the public
`openai/codex` repository at commit `6b99197`, not exercised against the live service.

A-Eye authenticates users to their existing provider subscriptions via standard
OAuth 2.0 + PKCE flows built into the application. No API key management is
required for subscription-based providers. API key entry remains available as a
fallback and is the only path for providers that have no OAuth (Grok, Ollama).

## Principles

- A-Eye never stores raw passwords or subscription cookies
- Each provider's token lives in the OS credential store (libsecret on Linux)
- Token refresh happens silently in the Rust core; the UI only sees
  connected/disconnected state
- Revoking a session in A-Eye does not revoke the user's provider account
- A-Eye presents itself honestly; it does not claim to be the provider's
  own first-party client

## OAuth 2.0 + PKCE flow (Claude and Codex)

```
1. User clicks "Sign in with [Provider]" in A-Eye settings
2. Rust core generates:
     code_verifier  = 32 random bytes → base64url
     code_challenge = SHA-256(code_verifier) → base64url
3. Core spins up a one-shot local HTTP server on 127.0.0.1
     (random port in the generic flow; the Codex public client uses a fixed
     registered redirect instead, see the Codex section)
4. Core calls tauri::api::shell::open() → system browser opens:
     https://<provider-auth-endpoint>
       ?response_type=code
       &client_id=<client_id>
       &redirect_uri=http://127.0.0.1:<port><callback-path>
       &code_challenge=<challenge>
       &code_challenge_method=S256
       &state=<random state, checked on the callback>
       &scope=<scopes>
5. User approves in their browser (their normal session, their 2FA)
6. Browser redirects to the local callback with ?code=<auth_code>
7. Core exchanges: POST <token-endpoint> (form-encoded) { grant_type=authorization_code,
     code, code_verifier, redirect_uri, client_id }; redirects are never followed and
     the endpoint must be https (loopback http only for mock tests)
8. Core stores access_token + refresh_token in OS credential store
9. Core shuts down the local HTTP server
10. UI receives a "connected" event; settings shows provider name + account
```

The local callback server listens for at most 5 minutes. If the user does not
complete auth in that window, it shuts down and the UI shows a timeout error.

**Verified by spikes 02 and 03 (mock provider):** the `state` parameter must be generated
and checked before the code is trusted; the server binds 127.0.0.1 only and checks the
`Host` header; each connection needs its own absolute deadline, because a per-read
timeout lets a client that trickles bytes hold the one-shot server past the 5-minute
limit; and the token exchange must not follow HTTP redirects (a redirect to a cleartext
host was followed and its response accepted before this was fixed).

## Provider-specific details

### Claude (claude.ai subscription)

| Field | Value |
|---|---|
| Auth endpoint | `https://claude.ai/oauth/authorize` (**Unverified**) |
| Token endpoint | `https://claude.ai/oauth/token` (**Unverified**) |
| Client ID | Same client_id used by the Claude Code CLI (public) (**Unverified**; spike 03 does not hardcode or extract one) |
| Scopes | `openid profile` + Claude-specific coding scopes (**Unverified**) |
| Token lifetime | Short-lived access token + long-lived refresh token (**Unverified**; Anthropic documents `claude setup-token` as minting a one-year token) |
| Refresh | Silent, before expiry, in Rust core |

The Claude Agent SDK (npm package `@anthropic-ai/claude-agent-sdk`; the CLI is
`@anthropic-ai/claude-code`) is intended to accept the token from the credential
store. Anthropic documents the environment variable `CLAUDE_CODE_OAUTH_TOKEN`; spike 03
proves only that A-Eye can hand a token to a child process through that variable
without it appearing in argv, logs or files. Whether the SDK accepts a token minted by
A-Eye's own OAuth flow is **Unverified**. A-Eye does not re-implement the agent
protocol — it passes the token to the SDK which handles the managed session.

Note from [AGENT-INTEGRATIONS.md](AGENT-INTEGRATIONS.md): Anthropic restricts
third-party use of subscription capacity without approval. A-Eye is a personal
developer tool using the user's own subscription. The UI must not claim the
product is Anthropic's Claude Code or imply unlimited subscription use.

Policy finding from spike 03 (recorded as F01 in its capability record, read 2026-10-15
at code.claude.com/docs/en/legal-and-compliance and `/agent-sdk/overview`): Anthropic
states that third-party developers may not offer claude.ai login in their own
applications or "route requests through Free, Pro, or Max plan credentials on behalf of
their users", and that developers "may not collect, store, or intermediate Claude.ai
credentials or session tokens"; the Agent SDK overview adds "Unless previously approved".
The documented permitted path is an end user signing in to the unmodified Claude Code
binary with their own subscription. The project decision is to keep A-Eye-owned OAuth
for Claude ([ADR 014](../decisions/README.md)); this risk is accepted by the project
owner and is not resolved by the spike. Live use should wait for Anthropic's approval or
a switch to the permitted path.

Re-read 2026-10-18 at the same two pages: F01 still holds, and `legal-and-compliance` now also says
"sign-in to a Claude account must complete through Anthropic's own flow". No Claude endpoint,
client id or scope has been verified, and none will be obtained by extracting them from the Claude
CLI (see the [live auth runbook](../planning/LIVE-AUTH-RUNBOOK.md)).

Open decision: [AGENT-INTEGRATIONS.md](AGENT-INTEGRATIONS.md) currently says embedded
Claude uses supported API authentication unless the publisher has explicit approval for
another route, which conflicts with ADR 014 until that approval exists or one of the two
documents is revised. No release gate is defined yet; the spike's `A_EYE_LIVE_ACK_POLICY=1`
is an operator acknowledgement, not Anthropic approval. Whether to require written
approval before shipping Claude OAuth is for the project owner to decide.

### Codex (ChatGPT subscription)

| Field | Value |
|---|---|
| Issuer | `https://auth.openai.com` (**Verified from source**) |
| Auth endpoint | `https://auth.openai.com/oauth/authorize` (**Verified from source**; the earlier `/authorize` path was wrong) |
| Token endpoint | `https://auth.openai.com/oauth/token` (**Verified from source**; the earlier `/token` path was wrong) |
| Redirect URI | Fixed: `http://127.0.0.1:1455/auth/callback`, fallback port `1457` (**Verified from source**). A source comment on the fallback port says to keep it in sync with the Codex CLI Hydra redirect URI allow-list, so a random port with `/callback` will most likely be rejected by the Codex public client (inference, **Unverified** live) |
| Client ID | Same client_id used by the Codex CLI (public) (**Unverified**; spike 02 does not hardcode or extract one, and whether another application may use it is an open policy question) |
| Scopes | `openid profile email offline_access api.connectors.read api.connectors.invoke` (**Verified from source**) |
| Extra authorize parameters | `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `originator=<client name>` (**Verified from source**) |
| PKCE | S256; the CLI generates a verifier from 64 random bytes (the 32 random bytes in the generic flow above are also valid per RFC 7636) (**Verified from source**) |
| Token exchange | Authorization-code grant is form-encoded (`application/x-www-form-urlencoded`); ChatGPT token refresh uses JSON (**Verified from source**) |
| Token storage | Per the Codex docs (developers.openai.com/codex/auth) the CLI caches credentials in `~/.codex/auth.json` or the OS credential store, selected by `cli_auth_credentials_store` (`file`, `keyring`, `auto`, `ephemeral`). A-Eye stores its own tokens in the OS credential store (see below); sharing `auth.json` is **Unverified** |
| Token lifetime | **Unverified** (spike 02 used synthetic values) |
| App-server | Codex daemon started by A-Eye using stored tokens; connects via Unix socket (**Unverified**; no app-server turn was run) |

#### Documented route for open-source apps: ChatGPT plan usage (preferred)

Read 2026-10-18 at developers.openai.com/siwc/token-sharing-open-source. A-Eye is MIT-licensed
open source, so this route applies and removes the need to reuse the Codex CLI's client id.
Spike 02 implements the first registration as `--live-plan-usage`; it is tested against a
synthetic mock only and has not been run live
([runbook](../planning/LIVE-AUTH-RUNBOOK.md)).

| Field | Value |
|---|---|
| Authorize endpoint | `https://auth.openai.com/api/accounts/authorize` (**Verified from docs** and the provider's OIDC discovery document; automated GET is blocked by a Cloudflare challenge, so acceptance of the parameters is **Unverified** live) |
| Token endpoint | `https://auth.openai.com/api/accounts/oauth/token` (**Verified from docs** and discovery; a POST with a fake code returned HTTP 400 `invalid_grant` JSON, so it is reachable from a non-browser client) |
| Client ID | First registration uses the fixed `dynamic_agent_client`; the callback returns the issued `oaiapp_...` id, which is then used for the exchange and reauthorization. No client secret or partner key. Each issued id is bound to the user and the selected workspace (**Verified from docs**) |
| Host id | `ext_agent_host_id`, an opaque stable per-host value (JWK thumbprint URI recommended; `urn:uuid:` and `did:key` accepted; spike 02 uses `urn:uuid:`), persisted before the first sign-in; `agent_name_hint` is the app name (**Verified from docs**) |
| Redirect URI | `http://127.0.0.1:1455/auth/callback` for first registration; later sign-ins may use another port with the same scheme, host and path; not `localhost` (**Verified from docs**) |
| Scopes | `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`, with `resource=https://api.openai.com/v1` (**Verified from docs**). A valid ID token alone does not authorize plan usage; the granted scopes must include `chatgpt.tokens.use.direct` |
| Other parameters | fresh `state`, `nonce`, PKCE S256; the same `redirect_uri` and `resource` on the form-encoded code exchange (**Verified from docs**) |
| ID token | verify signature against the provider JWKS (`https://auth.openai.com/.well-known/jwks.json`, from discovery), `iss`, `aud` = issued client id, `exp`, `nonce`. Spike 02 checks every claim but not the signature (**Unverified**) |
| Lifetimes | access token 1 hour (`expires_in: 3600`), refresh token 30 days, rotated on each refresh (**Verified from docs**; **Unverified** live) |
| Refresh endpoint and request format | not documented on the pages read (**Unverified**) |
| Codex app-server | token passed as the `ACCESS_TOKEN` environment variable with a custom `model_provider` override, `codex app-server --listen stdio://`; renewal means restarting the process and `thread/resume` (**Verified from docs**; no turn was run) |
| Limits | Responses API needs `store: false`, `stream: true`; images are accepted when the model accepts them; hosted tools (file search, code interpreter, computer use, image generation) are unsupported (**Verified from docs**) |
| Availability | described for open-source and local apps; a limited-trial/interest-form path exists for paid or hosted apps; eligibility of this app is **Unverified** |

The Codex CLI client route below was re-checked against `openai/codex` at `70b3bb6` on
2026-10-18: endpoints, redirect, scopes and extra parameters are unchanged.

The Codex app-server (`codex app-server` in the CLI source; the `--app-server` spelling and the socket path below are **Unverified**) handles its own auth internally
when tokens are present. A-Eye can either:

- **Reuse the Codex CLI daemon** if already running (preferred; no duplicate process)
- **Start its own managed Codex daemon** with the stored tokens (managed mode)

Socket path: `~/.codex/app-server-control/app-server-control.sock` (symlink to
the live socket in `/tmp/codex-daemon-<uid>/`).

### Grok (xAI API key)

Grok has no OAuth subscription path. Auth is an API key entered manually.

| Field | Value |
|---|---|
| Entry | Settings text field (masked) |
| Storage | OS credential store (same as OAuth tokens) |
| Endpoint | `https://api.x.ai/v1` (OpenAI-compatible) |
| Model | `grok-2-vision-1212` for image input; `grok-3` for text-only |

Because the Grok API is OpenAI-compatible, the Codex adapter can be reused
with `base_url` overridden. No separate adapter is required.

### Ollama (local, no auth)

| Field | Value |
|---|---|
| Entry | URL field (default `http://localhost:11434`) |
| Auth | None |
| Model selection | Dropdown populated by `GET /api/tags` |

See [LOCAL-MODEL.md](LOCAL-MODEL.md) for how Ollama is used.

## Token storage on Linux

Implementation update (2026-10-19, issue #10): part 02's opt-in live modes now use
part 01's Secret Service backend for a versioned token record and startup
load/refresh. The record keeps all token fields, receipt time, endpoint, issued
client ID, resource and refresh encoding. Account suffixes are caller-selected
local aliases until verified identity is available. Refresh runs under a process
lock and persists rotating credentials before reporting success. See the
[part-02 README](../../spikes/part-02-codex-oauth/README.md) for fixture evidence and
limitations. The encrypted-file fallback below remains a proposed live UX;
part 02 uses FileStore only in synthetic restart tests, without an unlock UI.

Tokens are stored via the **Secret Service D-Bus API**
(`org.freedesktop.secrets`), provided by gnome-keyring or kwallet on most
desktop Arch setups.

```
Credential key format:
  service: "a-eye"
  account: "<provider>:<account_id>"

Stored attributes:
  access_token
  refresh_token
  expires_at (ISO 8601; spike 02 stores RFC 3339 `expires_at`, spike 03 stores unix seconds as
  `expires_at_unix`; to be reconciled)
  account_email (display only)
```

Rust crate: `keyring` (cross-platform, uses libsecret on Linux).

**Fallback (no keyring daemon):** AES-256-GCM encrypted file at
`~/.config/a-eye/credentials.enc`, key derived via Argon2id from a
user-set unlock passphrase. A-Eye prompts for the passphrase on first use
each session. This path is clearly labeled in the UI.

A-Eye detects which path is active at startup and shows it in Settings →
Privacy. The user can always see which credentials are stored and delete them.

## Settings UI

Each provider entry in Settings shows:

```
[Claude]   ● Connected  evan@example.com   [Sign out]
[Codex]    ● Connected  OpenAI account     [Sign out]
[Grok]     ○ Not connected                 [Add API key]
[Ollama]   ● Connected  localhost:11434    [Change URL]
```

"Sign out" removes the token from the credential store and marks the provider
disconnected. It does not revoke the session server-side (that requires the
user to log out in their browser). The UI explains this distinction.

## Phase 0 OAuth spike: AE-001-L / AE-002-L

**AE-001-L Codex OAuth on Linux:**
Complete the PKCE flow in a standalone Rust binary (no Tauri yet).
Store token via `keyring`. Start Codex app-server daemon. Send one synthetic
text + image turn. Receive response. Record: token format, socket protocol
version, image input acceptance, session resumability.

**AE-002-L Claude OAuth on Linux:**
Complete the PKCE flow. Store token. Invoke Claude Agent SDK with the token.
Send one text + image turn. Verify managed session binding.
Record: auth endpoint behavior, token lifetime, SDK version compatibility.

Both spikes must run on Arch Linux with a Wayland compositor. Record the
exact OS version, compositor, GPU driver, and Node/Rust versions used.
