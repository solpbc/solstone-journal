# Journal MCP OAuth

Local pairing and OAuth for the journal MCP endpoint. This is an MVP:
it is not intended for a public or untrusted client population. There is
no scope support, no client secrets or confidential clients, no third-party
dynamic redirect registration beyond the fixed allowlist, and only one
active pairing code at a time. Do not claim public readiness.

Static bearer tokens remain available and independent: `journal mcp token
create|list|revoke`. A client may authenticate with either scheme on each
request.

## Local pairing

```
journal mcp pairing generate
journal mcp pairing revoke
```

`generate` prints an 8-character pairing code once. It is valid for 10
minutes and one successful use. The ledger stores only a hash. Generate
and revoke always advance the pairing generation and invalidate any
previous code.

A transaction allows five wrong guesses; a sixth requires restarting
authorization from the client. Twenty wrong guesses from the same source
in the same generation lock the current pairing code. Locked pairing
refuses further guesses without advancing generation. Recover with
`journal mcp pairing generate` (new code, new generation) or `journal mcp
pairing revoke`.

## OAuth clients

```
journal mcp oauth list
journal mcp oauth revoke --client-id ID
```

`list` prints `client_id`, `client_name` or `-`, and `created_at`.
`revoke` invalidates that client's current access and refresh tokens and
blocks new authorization until the owner pairs again. It does not delete
the client record. There is no token-by-token list. A revoked generation
does not come back on restart.

## Endpoints and lifetimes

- `GET /.well-known/oauth-protected-resource`
- `GET /.well-known/oauth-authorization-server`
- `POST /register` — dynamic client registration (classic DCR or CIMD)
- `GET /authorize` — consent form; `POST /authorize` — pairing code
- `POST /token` — `authorization_code` and `refresh_token`

PKCE S256 is mandatory. Lifetimes: authorization code 5 minutes, access
token 1 hour, refresh grant 30 days (rotated on use).

## Redirects

Only these callback shapes are admitted:

- `http(s)://localhost[:port]/...`
- `http(s)://127.0.0.1[:port]/...` (port-agnostic match, including a
  Codex-style `/callback/<id>` path)
- exactly `https://claude.ai/api/mcp/auth_callback`

Nothing else is accepted.

## DCR and CIMD

Classic DCR is a metadata-only `POST /register` with no `client_id`. The
server mints a non-guessable `oauth:dcr:...` identifier.

A CIMD client presents its own `https://` document URL as `client_id`.
The endpoint fetches and validates that document: HTTPS only, no
private/loopback/link-local addresses, no redirects, 5 KB cap, 10-second
ceiling.

## State corruption

`journal/mcp-endpoint/oauth.json` is a single owner-only atomic ledger.
If it is unreadable or corrupt, OAuth issuance and redemption fail closed
(503-class responses). The static-token ledger `tokens.json` is
independent and is not affected. Recover by restoring or removing the
corrupt file. New OAuth state starts empty; existing OAuth grants are
lost, static tokens are not.
