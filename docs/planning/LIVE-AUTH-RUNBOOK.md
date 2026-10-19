# Live auth runbook: Codex (S0.1) and Claude (S0.2)

Status: prepared 2026-10-18. **Nothing in this file has been run live.** Every step that touches
a real provider needs a human in a browser; that requirement is permanent, not a gap to close.
Background: [OAuth architecture](../architecture/OAUTH.md), [roadmap S0.1/S0.2](ROADMAP.md),
[spike status](../../spikes/README.md).

## Codex, part 02: one command

Uses OpenAI's documented "Sign in with ChatGPT" route for open-source apps. A-Eye registers its
own client through the authorize endpoint; it does not borrow the Codex CLI's client id, and you
supply no id, key or secret.

Run from the repository root. Until this branch (`agents/sessions`) is merged, the code exists only
in its worktree, so use that as the root:

```bash
cd ~/projects/code/A-Eye-worktrees/sessions
cargo run --manifest-path spikes/part-02-codex-oauth/Cargo.toml -- --live-plan-usage
```

First build takes about 1.5 minutes (use `CARGO_BUILD_JOBS=1` if other agents are building; a
parallel build was OOM-killed once on this machine).

Persistence update (2026-10-19, issue #10): the live modes now use part 01 Secret
Service storage. A cached token is reused or refreshed before a callback port is
opened. New sign-ins are saved and read back; tokens survive process exit. The
default account alias is `plan-usage` (`default` for legacy `--live`), selectable via
`A_EYE_OAUTH_ACCOUNT_ID`. A missing/unusable keyring is an error, with no plaintext
fallback. See [part 02](../../spikes/part-02-codex-oauth/README.md) for restart,
refresh, recovery and verification limits. The startup paths below describe a
**missing** credential entry.

What happens:

1. The binary creates `~/.config/a-eye/host-id` (an opaque `urn:uuid:` value, mode 0600, not a
   credential) and listens on `127.0.0.1:1455`.
2. It prints one long `https://auth.openai.com/api/accounts/authorize?...` URL. **It does not
   open a browser.** Copy the URL into your normal browser (WSL is in mirrored networking mode, so
   a Windows browser reaches the WSL listener on 127.0.0.1; this has not been exercised with a
   real browser yet).
3. You sign in to ChatGPT and approve the consent screen. That click is the only manual step.
4. The browser lands on a "Sign-in complete. You can close this tab." page. The binary exchanges
   the code, checks the result and prints a table. It waits at most 5 minutes.

A pass looks like this (no row contains a token value):

```
  PASS        registration.issued_client_id  callback carried an issued client_id (prefix oaiapp_, len N)
  PASS        token.exchange                 access_token len=.. fp=..; refresh_token present fp=..; expires_in=Some(3600)s; ...
  PASS        token.plan_usage_scope         chatgpt.tokens.use.direct is in the granted scopes
  PASS        id_token.claims                iss, aud (= issued client_id), nonce and exp valid; alg=.. kid=..
  UNVERIFIED  id_token.signature             not checked: needs the provider JWKS (...)
```

Send back only that table and the exit code. Token values are never printed; the full token set
and issued client ID are retained through part 01 in Secret Service after successful claim checks.

### Know before you click

- **Server-side effect.** Approving registers an A-Eye client on OpenAI's side, bound to your
  account and the workspace you pick. The pages read do not say how to remove it. The host-id file
  is reused on later runs. A second run uses the saved session/issued client ID and refreshes
  when needed; it does not register another client automatically after a refresh failure.
- **Eligibility is unconfirmed.** OpenAI describes this route as available to open-source apps and
  points to a preview-limitations page. The server may refuse the request or the consent. If it
  does, record the exact error page text (not any URL parameters) and stop.
- **Port 1455 must be free.** A running `codex login` uses it. First registration requires
  exactly this port and `/auth/callback`.

### If it fails

| Symptom | Likely cause | Next step |
|---|---|---|
| `could not bind loopback callback port` | something holds 1455 | stop it (`ss -ltnp \| grep 1455`), rerun |
| Browser shows a Cloudflare "Just a moment" page | bot challenge on the authorize URL | complete it in the browser; do not script around it |
| Browser shows an OpenAI error page | server rejected a parameter or this app | record the page text; compare with [OAuth architecture](../architecture/OAUTH.md) |
| `no valid OAuth callback within 300s` | browser could not reach 127.0.0.1:1455 | open the URL in a browser inside WSL, or check `wslinfo --networking-mode` |
| `registration incomplete` | callback had no issued `client_id` | record it; the docs say registration is then incomplete |
| `token endpoint rejected ... invalid_grant` | code expired, reused, or PKCE/redirect mismatch | rerun; one-time codes are consumed |
| `FAIL token.plan_usage_scope` | sign-in worked but plan usage was not granted | record the granted scope list shown |

### Still unverified after a pass

Refresh-token rotation; a Responses API call with the access token; the Codex app-server turn
with `ACCESS_TOKEN` (S0.1 text and image turn, approval deny, cancel, resume); ID-token signature
against the JWKS; behaviour of a repeated registration. These are later parts, not this command.

### Legacy route (not recommended)

`--live` with `A_EYE_OAUTH_CLIENT_ID`, `A_EYE_OAUTH_AUTH_URL`, `A_EYE_OAUTH_TOKEN_URL` uses the
Codex CLI's public client id, which would have to be copied out of the Codex source. Whether
another application may do that is an open policy question, and the documented route above makes
it unnecessary. It is left in place unchanged.

## Claude, part 03: no live OAuth run is prepared, on purpose

Re-read 2026-10-18 at code.claude.com/docs/en/legal-and-compliance: Anthropic "does not permit
third-party developers to offer Claude.ai login into their own applications", and developers
"may not collect, store, or intermediate Claude.ai credentials or session tokens — sign-in to a
Claude account must complete through Anthropic's own flow." The Agent SDK overview adds "Unless
previously approved ... Use the API key authentication methods". This is finding F01; it still
holds and is now stated more firmly. Obtaining Claude Code's client id from its binary or source,
or reusing a subscription token, is the extraction path the roadmap rules out, so it is not
attempted and no endpoint, client id or scope for it has been verified.

The `--live` mode in part 03 stays gated behind `A_EYE_LIVE_ACK_POLICY=1` and is not part of any
runbook.

S0.2 per the [roadmap](ROADMAP.md) uses supported routes only, and both need you, not an agent:

1. **Subscription route:** you sign in to the unmodified `claude` CLI yourself (`claude` then its
   login prompt), and A-Eye is added to that CLI as an MCP server (spike 09). No token reaches A-Eye.
2. **API route:** you export your own `ANTHROPIC_API_KEY` in the shell that will run the SDK
   session (spike 15). Never paste it into a chat or a file in this repo.

Neither route is built yet; this file only records that they are the permitted ones.
