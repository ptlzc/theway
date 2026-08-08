# `/login` + persistent `~/.theway/auth.json` + OAuth (Codex, Copilot, Google)

> Parent: master roadmap issue.
> Tier: 5 (auth / cloud).

## Goal

Today `theway` reads `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / etc. from env. Users on Anthropic
Pro/Max subscriptions, GitHub Copilot, OpenAI ChatGPT/Codex, Google OAuth-only flows can't
authenticate at all.

Ship:

- `theway login <provider>` opens the OAuth flow (PKCE; spawn the system browser), captures
  callback on a localhost listener, stores refreshable token under `~/.theway/auth.json`.
- Auto-refresh on expiry; transparent to the agent loop.
- `/login` inside the REPL routes to the same flow.
- Per-provider auth selection precedence: env API key > `auth.json` token > unauthenticated
  (fail with a helpful message).
- Supported providers in v1: Anthropic (Claude Pro/Max), Google (Gemini OAuth), GitHub Copilot,
  OpenAI Codex.

## Architecture

```
theway-llm-provider/src/auth/
  store.rs       JSON-on-disk + chmod 600; in-memory cache; fsync on write
  oauth/         pkce.rs (challenge/verifier), flow.rs (per-provider helpers)
  refresh.rs     Token refresh executor; background task with jitter
  resolve.rs     Choose credential per (provider, request)
```

`theway`:

```
theway/src/auth/
  cli.rs         theway login <provider> command
  slash.rs       /login implementation in the REPL
  ui.rs          Status display + browser-opener helper
```

Each provider gets a small descriptor:

```rust
struct OAuthProvider {
    name: &'static str,
    auth_url: &'static str,
    token_url: &'static str,
    client_id: &'static str,
    scopes: &'static [&'static str],
    redirect_port: u16, // bind 127.0.0.1:<port>
}
```

The token store is one JSON file with per-provider entries: `{ access, refresh, expires_at,
scopes }`. Refresh task wakes ~60s before expiry, refreshes, writes back.

## Stability

- File permissions: `chmod 600` after write; never world-readable.
- Atomic writes: rename-temp pattern. Fsync the directory.
- If a token refresh fails N consecutive times, mark the credential degraded and surface an
  error on next use rather than retrying forever.
- Local callback listener has a strict timeout (5min) and only accepts the matching `state`
  param.
- Browser-open failures fall back to printing the URL for the user to paste manually.

## Extensibility

- New OAuth providers drop in by adding a descriptor — no other code changes.
- `auth.json` schema is versioned; migrations supported.
- `AuthResolver` is a trait so an enterprise extension could replace the store with a
  Keychain / Secret Service backend later.

## Performance

- Token refresh runs on a single background task; lazy schedule (no work when nothing is
  expiring soon).
- Auth resolution is O(1) hash lookup.

## Testing

| Layer | What |
|---|---|
| **unit** | PKCE challenge generation; refresh expiry math (token expires in 30s → refresh now); store round-trip; precedence resolver. |
| **integration** | Mocked OAuth server (wiremock) — full PKCE flow end to end; refresh-on-expiry replaces the token in-store. |
| **e2e** | `theway login anthropic` with the mock server endpoints overridden via test env: capture browser-open URL, drive the callback synthetically, verify `auth.json` written with correct perms (mode 0600). |

## Acceptance criteria

- `theway login anthropic` results in a stored token usable by `theway chat`.
- Token refresh happens automatically before expiry; the agent loop never sees an expired-token
  401.
- `auth.json` is mode 0600 on disk.
- `theway logout <provider>` removes the entry and any in-memory cache.

## Out of scope

- Single-sign-on / enterprise identity providers (SAML / OIDC issuer config) — separate
  roadmap.
- Keychain / Secret Service integration (interesting, but a follow-up).
- See [[13-bedrock-vertex]] for cloud-IAM-style auth (SigV4, ADC).
