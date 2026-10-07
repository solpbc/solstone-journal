# MCP authorization for the journal

Owners give agents access to their journal, on the same computer,
over a private network, through solstone.me, or at an owner-operated hostname.
The remote routes serve internet clients when the owner turns them on. This
reference describes the shipped authorization flow and its limits.

OAuth supports public clients with PKCE, using CIMD or dynamic registration.
It does not support client secrets, confidential clients, arbitrary redirect
destinations, or OAuth scope strings. Only one pairing code can be open at a time.

During consent, the owner chooses whole-journal or facet access and the content
categories the connection may read. These permissions are stored per grant.

Static bearer tokens remain available and independent: `solstone journal mcp token
create|list|revoke`. The local and solstone.me routes accept these keys as well
as OAuth access tokens. The private-network and owner-operated hostname routes
require OAuth.

## Protocol support

The endpoint advertises `2025-03-26`. It accepts JSON-RPC over Streamable HTTP
at `/mcp`: `initialize` returns an `Mcp-Session-Id`, then `tools/list` and
`tools/call` use that session. `DELETE /mcp` ends it. The journal returns JSON
responses; it has no MCP GET/SSE stream or stdio transport.

This is a tools-and-session subset: `notifications/initialized` is not handled.
The endpoint does not implement the stateless `2026-07-28` revision, its
per-request metadata and header validation, or `server/discover`.

Tools are advertised from the connection's current grant. With transcripts,
entities and facets enabled, the set is `list_facets`, `search`, `fetch`,
`list_transcripts`, `get_transcript`, `list_entities`, `get_entity`,
`save_memory` and `recall_memory`. An absent or narrower read grant exposes
fewer tools. Own-memory access is described below.

<a id="doors"></a>

## Routes

The endpoint has four routes: `local` (`http://127.0.0.1:7659` on this
computer), `lan` (port 7660 on the journal's private addresses), `solstone.me`,
and `hostname` (an owner-operated domain). Where each one listens and how it is
turned on is in [SOLCLI.md](SOLCLI.md#journal-mcp-endpoint).

Each route serves its own OAuth endpoints, and a grant is bound to the route that
issued it. Its access and refresh tokens work only there:

- a `local` grant works only at `http://127.0.0.1:7659/mcp`;
- a `lan` grant works at the LAN route on any of the journal's admitted private
  addresses;
- a `hostname` grant works only at the hostname that issued it, and stops
  working once that hostname is changed or removed, even if it is later set
  back;
- a `solstone.me` grant works only through solstone.me.

A token presented at any other route is refused with 401. Static bearer tokens
are not bound to a route, but the private-network and owner-operated hostname
routes refuse them.

Public protocol metadata does not authorize journal reads. Every tool request
requires an active credential; ordinary reads also require the owner's current
read grant. A callback on the allowlist is a permitted destination, not proof
that the client is trustworthy.

## Local pairing

```
solstone journal mcp pairing generate [--door local|lan|solstone.me|hostname]
solstone journal mcp pairing revoke
```

`generate` prints an 8-character pairing code once. It is valid for 10
minutes and one successful use. The code works only at the route it was made
for; with no `--door`, that route is this computer. A code made by an older
version, which recorded no route, works nowhere. The ledger stores only a
hash. Generate always advances the pairing generation and invalidates any
previous code, replacing a locked code. Revoke clears a live code; revoke
does not clear a code that has already expired.

Registration and authorization answer only while a pairing code is open for
the route being asked. With no live code there (none made, expired, used,
revoked, locked, or made for another route), `POST /register` and
`GET /authorize` return 403 before parsing the request: no client metadata is
fetched and nothing is stored. The 403 does tell a caller whether a code is
open for that route; guessing is still bounded as below. So the owner makes the code first, then starts
the connection from the agent. A transaction belongs to the code that was open
when it started; once that code is used, replaced, revoked or expired, the
transaction is over and finishing it fails as an expired request. Making a new
code while an agent is waiting on the open one means starting again from the
agent. The consent page shows
no journal identity; the pairing code is the proof.

A transaction allows five wrong guesses; a sixth requires restarting
authorization from the client. An attempt to redeem a pairing code at a
different route fails with a pairing error and increments the failure count;
five wrong attempts or route mismatches exhaust the transaction. Twenty wrong
guesses from the same source in the same generation lock the current pairing
code. Locked pairing refuses further guesses without advancing generation.
Recover with `solstone journal mcp pairing generate` and the same `--door` (new code,
new generation) or `solstone journal mcp pairing revoke`.

Downgrade considerations: 2.0.19 still accepts a `local` code at solstone.me;
2.0.18 cannot read the OAuth file while a hostname code is in it; builds before
2.0.18 cannot read it while any code with a route is in it; the record can
outlast its 10 minutes. Before downgrading, make a new code with this version
and then revoke it, which clears it. 2.0.22 and earlier cannot read the OAuth
file while it holds a transaction still waiting on a code; the same make-then-
revoke clears those too. Builds that read only schema 1 refuse a
schema 2 OAuth file, with a schema error.

## OAuth clients

```
solstone journal mcp oauth list
solstone journal mcp oauth revoke --client-id ID
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
  while a pairing code is open for this route
- `GET /authorize`: consent form, only while a pairing code is open for this
  route; `POST /authorize`: pairing code
- `POST /token`: `authorization_code` and `refresh_token`

PKCE S256 is mandatory. Lifetimes: authorization code 5 minutes, access
token 1 hour, refresh grant 30 days from initial token issuance. Refresh tokens rotate on use;
rotation does not extend that grant deadline. Pair again after it expires.
Static bearer keys have no timed expiry and remain valid until revoked.
Presenting one of the
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
- exactly `https://grok.com/connectors-oauth-exchange-code/`
- `https://oauth-redirect.googleusercontent.com/r/user_bound_custom-mcp-<id>`
  for the observed Gemini custom-app callback shape. `<id>` is one nonempty
  segment of at most 160 ASCII letters, digits, underscores or hyphens.

Nothing else is accepted.

Hosted callbacks require HTTPS with no explicit port, query or fragment. The
complete registered path must match; another account or server's path is refused.
An admitted Google callback is a destination, not verified client identity.

The journal advertises issuer identification and includes the exact advertised
issuer in both successful and error OAuth redirects.

Use the `resource` URI from Protected Resource Metadata in authorization and
authorization-code token requests. A refresh request may omit `resource`; if
provided, it must match the route's resource. OAuth grants remain bound to the
issuing route even when that parameter is omitted on refresh.

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


## Agent memories

Every active bearer or OAuth connection can call `save_memory` and `recall_memory`,
including when its ordinary read grant is absent, limited or unavailable. Both
methods stay within that connection's own source. Other journal tools keep the
owner's existing read permissions. There is no separate write permission.

`save_memory` accepts only `content` and `operation_id`. Content is nonempty exact
UTF-8, up to 32,768 decoded bytes. The operation id is 1–128 ASCII letters, digits,
periods, underscores, colons or hyphens. Save once per intended note. If the response is lost or
says `uncertain_retry`, retry with the same operation id and exact content. A
successful retry returns the original coordinate, timestamp and origin rather
than another note. Reusing an id with different bytes is refused. A `deleted`
receipt means the owner removed that original; retry cannot recreate it.

Receipts contain no note body. `own_recall` is `ready`, `pending`, `unavailable`
or `deleted`; it describes the original's verified index readiness.
`ordinary_readable` reports whether the current ordinary grant also admits that
indexed original. Notification delivery is best effort: a stored note can remain
pending until normal indexing runs, even with thinking turned off.

`recall_memory` accepts optional `query`, `limit`, `day`, `day_from`, `day_to` and
`continuation`. Omit `query` to browse. An explicitly empty query does not browse.
Dates use `YYYYMMDD`. Pages return whole original notes with authenticated origin,
defaulting to five and capped at 20 notes and 65,536 content bytes. Neither
summaries nor other connections' memories are returned. Follow `continuation`
with the same query and date filters. Cursors can expire after an endpoint
restart. `complete: false` with a reason means coverage or execution could not be
confirmed; an empty incomplete page does not prove there are no memories.

A bearer token's durable id and an OAuth consent identify separate sources.
Renaming a connection or refreshing OAuth access preserves its source. Replacing
a bearer token or authorizing a new OAuth consent creates a new source. Ordinary
read-grant changes do not revoke own recall; revoking the credential does.
New originals use the owner's journal day. A retry retains its reserved day even
if the timezone changes.

Owners can inspect original notes and body-free request outcomes in their journal.
Deleting an original does not erase existing derivatives, copies already read by
an agent or backups. Backup restoration preserves retry consumption only when the
matching identity and operation records are restored together. An older backup
can roll consumption back. windows file publication is checked, but this does
not promise directory-entry durability across sudden power loss.
