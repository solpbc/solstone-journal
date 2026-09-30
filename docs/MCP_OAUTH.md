# journal MCP OAuth

Local pairing and OAuth for the journal MCP endpoint. This is an MVP:
it is not intended for a public or untrusted client population. There is
no scope support, no client secrets or confidential clients, no third-party
dynamic redirect registration beyond the fixed allowlist, and only one
active pairing code at a time.

Static bearer tokens remain available and independent: `journal mcp token
create|list|revoke`. A client may authenticate with either scheme on each
request.

## Doors

The endpoint has four doors: `local` (`http://127.0.0.1:7659` on this
computer), `lan` (port 7660 on the journal's private addresses), `solstone.me`,
and `hostname` (an owner-operated domain). Where each one listens and how it is
turned on is in [SOLCLI.md](SOLCLI.md#journal-mcp-endpoint).

Each door serves its own OAuth endpoints, and a grant is bound to the door that
issued it. Its access and refresh tokens work only there:

- a `local` grant works only at `http://127.0.0.1:7659/mcp`;
- a `lan` grant works at the LAN door on any of the journal's admitted private
  addresses;
- a `hostname` grant works only at the hostname that issued it, and stops
  working once that hostname is changed or removed, even if it is later set
  back;
- a `solstone.me` grant works only through solstone.me.

A token presented at any other door is refused with 401. Static bearer tokens
are not bound to a door.

## Local pairing

```
journal mcp pairing generate [--door local|lan|solstone.me|hostname]
journal mcp pairing revoke
```

`generate` prints an 8-character pairing code once. It is valid for 10
minutes and one successful use. The code works only at the door it was made
for; with no `--door`, that door is this computer. A code made by an older
version, which recorded no door, works nowhere. The ledger stores only a
hash. Generate always advances the pairing generation and invalidates any
previous code, replacing a locked code. Revoke clears a live code; revoke
does not clear a code that has already expired.

Registration and authorization answer only while a pairing code is open for
the door being asked. With no live code there (none made, expired, used,
revoked, locked, or made for another door), `POST /register` and
`GET /authorize` return 403 before parsing the request: no client metadata is
fetched and nothing is stored. The 403 does tell a caller whether a code is
open for that door; guessing is still bounded as below. So the owner makes the code first, then starts
the connection from the agent. A transaction belongs to the code that was open
when it started; once that code is used, replaced, revoked or expired, the
transaction is over and finishing it fails as an expired request. Making a new
code while an agent is waiting on the open one means starting again from the
agent. The consent page shows
no journal identity; the pairing code is the proof.

A transaction allows five wrong guesses; a sixth requires restarting
authorization from the client. An attempt to redeem a pairing code at a
different door fails with a pairing error and increments the failure count;
five wrong attempts or door mismatches exhaust the transaction. Twenty wrong
guesses from the same source in the same generation lock the current pairing
code. Locked pairing refuses further guesses without advancing generation.
Recover with `journal mcp pairing generate` and the same `--door` (new code,
new generation) or `journal mcp pairing revoke`.

Downgrade considerations: 2.0.19 still accepts a `local` code at solstone.me;
2.0.18 cannot read the OAuth file while a hostname code is in it; builds before
2.0.18 cannot read it while any code with a door is in it; the record can
outlast its 10 minutes. Before downgrading, make a new code with this version
and then revoke it, which clears it. 2.0.22 and earlier cannot read the OAuth
file while it holds a transaction still waiting on a code; the same make-then-
revoke clears those too. Builds that read only schema 1 refuse a
schema 2 OAuth file, with a schema error.

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
- `POST /register`: dynamic client registration (classic DCR or CIMD), only
  while a pairing code is open for this door
- `GET /authorize`: consent form, only while a pairing code is open for this
  door; `POST /authorize`: pairing code
- `POST /token`: `authorization_code` and `refresh_token`

PKCE S256 is mandatory. Lifetimes: authorization code 5 minutes, access
token 1 hour, refresh grant 30 days (rotated on use). Presenting one of the
grant's last 16 rotated-out refresh tokens revokes the grant; an older one is
refused and the grant stays. One exception: the immediately previous token,
presented again within 30 seconds of the rotation that replaced it, gets back
the pair that rotation issued.

## Redirects

Only these callback shapes are admitted:

- `http(s)://localhost[:port]/...`
- `http(s)://127.0.0.1[:port]/...` (port-agnostic match, including a
  Codex-style `/callback/<id>` path)
- exactly `https://claude.ai/api/mcp/auth_callback`
- exactly `https://chatgpt.com/connector_platform_oauth_redirect`

Nothing else is accepted.

The journal advertises issuer identification and includes the exact advertised
issuer in both successful and error OAuth redirects.

## DCR and CIMD

Classic DCR is a metadata-only `POST /register` with no `client_id`. The
server mints a non-guessable `oauth:dcr:...` identifier.

A CIMD client presents its own `https://` document URL as `client_id`.
The endpoint fetches and validates that document: HTTPS only, no
private/loopback/link-local addresses, no redirects, 5 KB decoded-body cap,
10-second ceiling. HTTP/1.1 chunked bodies are decoded within that cap;
chunk framing and trailers have a separate 8 KB budget. Conflicting
Content-Length / Transfer-Encoding headers and unsupported transfer or
content encodings are refused.

## State corruption

`journal/mcp-endpoint/oauth.json` is a single owner-only atomic ledger.
If it is unreadable or corrupt, OAuth issuance and redemption fail closed
(503-class responses). The static-token ledger `tokens.json` is
independent and is not affected. Recover by restoring or removing the
corrupt file. New OAuth state starts empty; existing OAuth grants are
lost, static tokens are not.
