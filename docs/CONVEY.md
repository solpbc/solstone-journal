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

### Authentication

Password authentication is configured from Settings → Security when enabling network access. For headless setups, use the CLI:

```bash
journal password set
```

When a password is set, it is stored as a secure hash in `config/journal.json` under `convey.password_hash`.

## Architecture

Convey uses an **app plugin system** where all functional views are implemented as independent apps in the `/apps/` directory. The core `solstone/convey/` package provides authentication, WebSocket communication, and the app loading infrastructure.

```
convey/
  __init__.py        - Flask app factory, app registry, context processors
  state.py           - global state (journal_root)
  bridge.py          - Callosum WebSocket bridge for real-time events
  utils.py           - shared helpers (format_date, spawn_agent, etc.)
  views/
      __init__.py    - blueprint registration
      home.py        - authentication (login/logout) and root redirect
  templates/
      app.html       - main app container template
      menu_bar.html  - dynamic left sidebar menu
      status_pane.html - WebSocket status indicator
      login.html     - login page
      macros.html    - Jinja macros
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

The `solstone/convey/views/home.py` module provides essential routes:

- `/` - Redirects to `/app/home/`
- `/login` - Authentication page
- `/logout` - Clear session and redirect to login
- `/favicon.ico` - Serve favicon

All functional views are accessed at `/app/{name}/` URLs.

### Owner-facing errors

Reasons live in `solstone/convey/reasons.py`.
Add a new owner-facing error by defining an `UPPER_SNAKE_CASE` Reason constant.
Route call sites should use `error_response(REASON_NAME, detail=...)`.
`Reason.message` is sol speaking: first-person, lowercase first letter except
the I pronoun, no exception class names or paths. Put those specifics in
`detail`.

### Observer Callosum SSE Feed

Observer clients can open a server-sent events feed at
`/app/observer/<key>/callosum`. The feed is a passive view of the Callosum bus:
each `data:` frame is the same event-shaped payload the bridge saw
(`tract`, `event`, `ts`, plus event fields). Chat events appear only after the
chat append path has written its JSONL record, so subscribers see post-disk
state rather than speculative messages.

This endpoint is inside the observer trust boundary. It performs no redaction
or per-field filtering because observers are treated as part of the local
owner-controlled system. If convey moves off-device or into a hosted deployment,
that assumption must be revisited before exposing this feed.

Keep Callosum events event-shaped. The SSE route should not translate payloads
into app-specific DTOs or add compatibility aliases; it forwards the bus shape
and relies on producers to keep `tract`/`event`/`ts` discipline. In a hosted or
multi-tenant mode, the feed will also need scoping by the observer's authorized
facet or scope set before forwarding any event.

The observer SSE feed (`/app/observer/api/list` flipping `live` on/off as a
Callosum ping flows through the bridge) is exercised by the `apps/observer` SSE
tests; a registered observer client opens the feed and the bridge emits the ping.

### Adding a New App

See [APPS.md](APPS.md) for detailed instructions on creating new apps.
