# Convey

The journal's web UI. The process is `solstone-core convey`. There is no
Flask app, and `make install` does not install it.

## Architecture

`solstone-core-convey-shell` owns the shell, session gate, and app registry.
Per-app HTTP lives in `solstone-core-*-web` crates or in
`convey-shell/assets/<app>/`. See [APPS.md](APPS.md).

`/` redirects to `/app/home/`. App surfaces live at `/app/{name}/`.

### Frontend conventions

The client-side architecture (static shell + per-app workspace fragments, the
`/api/shell` contract, initial-state endpoint conventions, rendering and
loading/error-state rules) is specified in [`CONVEY-FRONTEND.md`](CONVEY-FRONTEND.md).
Read it before touching any workspace, shell chrome, or shared client helper.

### Shell chrome

The canonical static shell has dedicated rail, dock, launcher, and status
instrument slots. `shell_boot.js` renders the rail, dock, grouped launcher,
and status instrument from `/api/shell`. The launcher is a body-level modal
dialog so the shared modal-layer focus and inert behavior applies. The rail
uses non-null `rail_group`/`rail_rank`; the launcher uses
`launcher_group`/`launcher_rank` and contains every registered app exactly
once. Facet selection belongs to the workspace that owns it.

### HTTP API conventions

These conventions apply to every native app router. The browser client
`core/crates/solstone-core-convey-shell/assets/static/api.js` (`apiJson`,
`saveControl`) is the consumer side.

**Namespacing.** JSON APIs live under `/api/`: `/app/{name}/api/<resource>` for an
app, `/api/<domain>` for a core blueprint (`/api/shell`, `/api/system/status`,
…). HTML pages live at `/app/{name}/<view>` with no `/api/` infix. The
unauthenticated setup wizard lives under `/init/...`. The `/api/` infix is the
JSON-vs-HTML discriminator: never put JSON at a non-`/api/` path, never put a page
under `/api/`.

**Resources and verbs.** Model nouns; the HTTP method is the verb. `GET` is safe and
never mutates server state. `POST` creates or runs a non-idempotent action; `PUT`
replaces, `PATCH` partial-updates, `DELETE` removes. Use them rather than
overloading `POST`. Address a resource the same way on read and write (by URL id,
not by a name in the body). Reserve a verb in the path (`/pair`, `/reprocess`,
`/accept`) for genuine RPC transitions that have no clean resource mapping.

**Responses.** The HTTP status code is the success signal. The body carries the
resource, not a `{"success": true}` flag. Return the resource object on a read; a
`{"items": [...], "total": N, "next_cursor"|"offset": ...}` envelope for a
collection, never a bare top-level array; `201` plus the created resource
(including its server-assigned id) on create; `202` plus a status handle for async
work.

**Errors.** Every JSON route returns `{error, reason_code, detail}`. The HTTP
status is the success signal; `reason_code` is the machine-readable contract.
Never return a bare `"", 404`, an in-band `{"error": ...}` at HTTP 200, or raw
exception text on a JSON route. HTML page routes may render a 404 page or
plain text.

**Identity.** The actor comes from the session gate, never from a
client-supplied field. Key-based ingest authenticates with an
`Authorization: Bearer` header (never a key in the URL path) and derives its
storage scope from the authenticated record; a scope id in the URL is an
assertion to check against that record, not an input.

**Pagination.** Offset/limit (max 100) or a cursor; no list endpoint returns
an unbounded full array.

**Composed reads.** Aggregate related data server-side in one named endpoint (the
shape behind `/app/home/api/pulse`, `settings` `/api/providers`, `speakers`
`/api/review`) rather than making the client fan out across many calls.

### Owner-facing errors

`reason_code` is the machine-readable contract. The owner-facing message is
sol speaking: first-person, lowercase first letter except the I pronoun, no
exception class names or paths. Put those specifics in `detail`.

### Adding a New App

See [APPS.md](APPS.md).
