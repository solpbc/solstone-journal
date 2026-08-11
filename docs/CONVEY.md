# solstone-convey

Web-based journal review interface built with Flask. It exposes a few small views for exploring daily summaries and entity data stored inside a **journal** folder.

## Installation

```bash
make install
```

## Usage

Run the server with:

```bash
convey
```

## Architecture

Convey uses an **app plugin system** where all functional views are implemented as independent apps in the `/apps/` directory. The core `solstone/convey/` package provides access gating, WebSocket communication, and the app loading infrastructure.

```
convey/
  __init__.py        - Flask app factory, app registry, context processors
  state.py           - global state (journal_root)
  bridge.py          - Callosum WebSocket bridge for real-time events
  utils.py           - shared helpers (format_date, spawn_agent, etc.)
  root.py            - access gate, setup routes, and root redirect
  templates/
      init.html      - static setup/onboarding page (pre-journal)
  static/            - shared CSS and JavaScript
      app.css        - app system styles
      app.js         - facet pills, services, notification center
      websocket.js   - WebSocket connection handler
      error-handler.js - global error handling
      colors.js      - color palette
      vendor/        - third-party libraries (marked.js)

apps/                - App plugin directory (see APPS.md)
  {app_name}/
    app.json         - metadata (icon, label)
    routes.py        - Flask blueprint with routes
    workspace.html   - main UI template
    background.html  - (optional) background service script
```

### App System

All functional views are implemented as apps in `/apps/`. Each app:
- Has its own directory with `app.json`, `routes.py`, and `workspace.html`
- Uses blueprint name `app:{name}` with URL prefix `/app/{name}/`
- Is automatically discovered and registered by `AppRegistry`
- Can provide facet-scoped views and background services

Browse `/apps/` to see available apps.

### Core Routes

The `solstone/convey/root.py` module provides essential routes:

- `/` - Redirects to `/app/home/`
- `/favicon.ico` - Serve favicon

All functional views are accessed at `/app/{name}/` URLs.

### Frontend conventions

The client-side architecture — static shell + per-app workspace fragments, the
`/api/shell` contract, initial-state endpoint conventions, rendering and
loading/error-state rules — is specified in [`CONVEY-FRONTEND.md`](CONVEY-FRONTEND.md).
Read it before touching any workspace, shell chrome, or shared client helper.

### HTTP API conventions

These conventions apply to every app `routes.py` and the core blueprints. New and
refactored routes follow them; the shared helpers in `solstone/convey/utils.py`
(`error_response`, `success_response`, `parse_pagination_params`) are the reusable
parts, and the browser client `solstone/convey/static/api.js` (`apiJson`,
`saveControl`) is the consumer side.

**Namespacing.** JSON APIs live under `/api/` — `/app/{name}/api/<resource>` for an
app, `/api/<domain>` for a core blueprint (`/api/config`, `/api/system`,
`/api/chat`, …). HTML pages live at `/app/{name}/<view>` with no `/api/` infix. The
unauthenticated setup wizard lives under `/init/...`. The `/api/` infix is the
JSON-vs-HTML discriminator: never put JSON at a non-`/api/` path, never put a page
under `/api/`.

**Resources and verbs.** Model nouns; the HTTP method is the verb. `GET` is safe and
never mutates server state. `POST` creates or runs a non-idempotent action; `PUT`
replaces, `PATCH` partial-updates, `DELETE` removes — use them rather than
overloading `POST`. Address a resource the same way on read and write (by URL id,
not by a name in the body). Reserve a verb in the path (`/pair`, `/reprocess`,
`/accept`) for genuine RPC transitions that have no clean resource mapping.

**Responses.** The HTTP status code is the success signal — the body carries the
resource, not a `{"success": true}` flag. Return the resource object on a read; a
`{"items": [...], "total": N, "next_cursor"|"offset": ...}` envelope for a
collection — never a bare top-level array; `201` plus the created resource
(including its server-assigned id) on create; `202` plus a status handle for async
work.

**Errors.** Every JSON route errors through `error_response(REASON, detail=...)` →
`{error, reason_code, detail}` (see "Owner-facing errors" below). The HTTP status
comes from the `Reason`. `reason_code` is the machine-readable contract. Never
return a bare `"", 404`, an `abort()`, an in-band `{"error": ...}` at HTTP 200, or
raw exception text on a JSON route. HTML page routes may render a 404 page or plain
text.

**Identity.** The actor comes from `flask.g.identity` (stamped per request), never
from a client-supplied field. Key-based ingest authenticates with an
`Authorization: Bearer` header — never a key in the URL path — and derives its
storage scope from the authenticated record; a scope id in the URL is an assertion
to check against that record, not an input. Any key-authenticated route must be in
the `require_access` allowlist.

**Pagination.** Use `parse_pagination_params()` (offset/limit, max 100) or a cursor;
no list endpoint returns an unbounded full array.

**Composed reads.** Aggregate related data server-side in one named endpoint (the
shape behind `/app/home/api/pulse`, `settings` `/api/providers`, `speakers`
`/api/review`) rather than making the client fan out across many calls.

### Owner-facing errors

Reasons live in `solstone/convey/reasons.py`.
Add a new owner-facing error by defining an `UPPER_SNAKE_CASE` Reason constant.
Route call sites should use `error_response(REASON_NAME, detail=...)`.
`Reason.message` is sol speaking: first-person, lowercase first letter except
the I pronoun, no exception class names or paths. Put those specifics in
`detail`.

### Observer Callosum SSE Feed

Observer clients can open a server-sent events feed at
`/app/observer/callosum`. The observer key is supplied in the
`X-Solstone-Observer` header or the `Authorization: Bearer` header. The feed is
a passive view of the Callosum bus: each `data:` frame is the same event-shaped
payload the bridge saw (`tract`, `event`, `ts`, plus event fields). Chat events
appear only after the chat append path has written its JSONL record, so
subscribers see post-disk state rather than speculative messages.

This endpoint is inside the observer trust boundary. It performs no redaction
or per-field filtering because observers are treated as part of the local
owner-controlled system. If convey moves off-device or into a hosted deployment,
that assumption must be revisited before exposing this feed.

Keep Callosum events event-shaped. The SSE route should not translate payloads
into app-specific DTOs or add compatibility aliases; it forwards the bus shape
and relies on producers to keep `tract`/`event`/`ts` discipline. In a hosted or
multi-tenant mode, the feed will also need scoping by the observer's authorized
facet or scope set before forwarding any event.

<!-- BEGIN GENERATED callosum-registry -->
| Tract | Events |
|---|---|
| `activity` | `live`, `recorded` |
| `chat` | `owner_message`, `sol_message`, `talent_queued`, `talent_spawned`, `talent_finished`, `talent_errored`, `reflection_ready`, `chat_queue_depth`, `chat_error`, `sol_chat_request`, `sol_chat_request_superseded`, `owner_chat_open`, `owner_chat_dismissed`, `support_draft`, `result`, `support_submit_claim` |
| `cortex` | `request`, `start`, `thinking`, `tool_start`, `tool_end`, `finish`, `error`, `talent_updated`, `info`, `status`, `cancel`, `dry_run`, `progress`, `text_delta`, `tool_budget_exhausted`, `warning`, `budget_escalation` |
| `importer` | `started`, `status`, `completed`, `error`, `file_imported`, `enrichment_ready` |
| `link` | `pair_complete`, `last_seen`, `stream_reset` |
| `logs` | `exec`, `line`, `exit` |
| `navigate` | `request` |
| `notification` | `*` |
| `observe` | `status`, `observing`, `detected`, `described`, `transcribed`, `observed`, `memory_throttle_started`, `memory_throttle_completed` |
| `storage` | `warning` |
| `supervisor` | `started`, `stopped`, `restarting`, `status`, `queue`, `scheduled`, `provider_runtime`, `request`, `restart`, `drain`, `skipped`, `sync_conflict` |
| `support` | `proactive_suggestion` |
| `think` | `started`, `status`, `group_started`, `group_completed`, `talent_started`, `talent_completed`, `completed`, `segments_started`, `segments_completed`, `memory_throttle_started`, `memory_throttle_completed`, `daily_complete` |
<!-- END GENERATED callosum-registry -->

The observer SSE feed is exercised by the `apps/observer` SSE tests; a
registered observer client opens the feed and the bridge emits the ping.

### Adding a New App

See [APPS.md](APPS.md) for detailed instructions on creating new apps.
