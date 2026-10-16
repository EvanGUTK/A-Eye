# OAuth and provider authentication

Status: proposed. Supplements [AGENT-INTEGRATIONS.md](AGENT-INTEGRATIONS.md).

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
3. Core spins up a one-shot local HTTP server on a random port
     (e.g. http://127.0.0.1:54321/callback)
4. Core calls tauri::api::shell::open() → system browser opens:
     https://<provider-auth-endpoint>/authorize
       ?response_type=code
       &client_id=<client_id>
       &redirect_uri=http://127.0.0.1:<port>/callback
       &code_challenge=<challenge>
       &code_challenge_method=S256
       &scope=<scopes>
5. User approves in their browser (their normal session, their 2FA)
6. Browser redirects to the local callback with ?code=<auth_code>
7. Core exchanges: POST /token { code, code_verifier, redirect_uri }
8. Core stores access_token + refresh_token in OS credential store
9. Core shuts down the local HTTP server
10. UI receives a "connected" event; settings shows provider name + account
```

The local callback server listens for at most 5 minutes. If the user does not
complete auth in that window, it shuts down and the UI shows a timeout error.

## Provider-specific details

### Claude (claude.ai subscription)

| Field | Value |
|---|---|
| Auth endpoint | `https://claude.ai/oauth/authorize` |
| Token endpoint | `https://claude.ai/oauth/token` |
| Client ID | Same client_id used by the Claude Code CLI (public) |
| Scopes | `openid profile` + Claude-specific coding scopes |
| Token lifetime | Short-lived access token + long-lived refresh token |
| Refresh | Silent, before expiry, in Rust core |

The Claude Agent SDK (`@anthropic-ai/claude-code`) accepts the access token
from the credential store directly. A-Eye does not re-implement the agent
protocol — it passes the token to the SDK which handles the managed session.

Note from [AGENT-INTEGRATIONS.md](AGENT-INTEGRATIONS.md): Anthropic restricts
third-party use of subscription capacity without approval. A-Eye is a personal
developer tool using the user's own subscription. The UI must not claim the
product is Anthropic's Claude Code or imply unlimited subscription use.

### Codex (ChatGPT subscription)

| Field | Value |
|---|---|
| Auth endpoint | `https://auth.openai.com/authorize` |
| Token endpoint | `https://auth.openai.com/token` |
| Client ID | Same client_id used by the Codex CLI (public) |
| Scopes | OpenAI-defined coding/tool scopes |
| Token storage | `~/.codex/auth.json` format (existing Codex CLI convention) |
| App-server | Codex daemon started by A-Eye using stored tokens; connects via Unix socket |

The Codex app-server (`codex --app-server`) handles its own auth internally
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
  expires_at (ISO 8601)
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
