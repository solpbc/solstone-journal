# Convey Frontend Conventions

**Status:** binding for all convey UI work. Companion to `CONVEY.md` (HTTP API
conventions) and `APPS.md` (app development guide).

Convey's owner-facing web UI is a **pure client-rendered application**: JSON
APIs + SSE/WS events + static files. The server composes no HTML for the
serving path. The one exception is PDF generation (`news` and `reflections`
`/pdf` routes), which uses Jinja as a rendering *library* — never as a page
server.

## Architecture: static shell + per-app workspaces

- **One static shell**
  (`core/crates/solstone-core-convey-shell/assets/static/shell.html`) is served
  unconditionally for every `/app/{name}` route. The client derives the
  current app from `location.pathname` and boots from `GET /api/shell`.
- **Per-app workspace fragments** stay one file per app
  (`apps/{name}/workspace.html`): markup + `<style>` + `<script>`, served
  verbatim as a static asset — zero server-side template processing. The shell
  fetches the fragment, mounts it into `<main>`, and re-executes its scripts in
  document order via the shared mount helper.
- **No framework, no build step.** Vanilla JS (ES modules or classic scripts),
  template literals, `<template>` elements where they help. Static files ship
  in the wheel as-is. Consistency comes from the shared helpers and design
  tokens (`static/tokens.css`), not from a framework.
- **`AppServices`** (`static/app.js`) is the shared client runtime: service
  and task registration, notifications, `renderMarkdown` (marked + DOMPurify),
  `escapeHtml`. **`apiJson`/`ApiError`** (`static/api.js`) is the only HTTP
  client. **`appEvents`** (`static/websocket.js`) is the only event transport.

## Shell boot sequence

1. Static shell parses with `#app-launcher`, `#app-rail`, `#app-dock`, and
   `#status-instrument` slots. The launcher is a direct body child.
2. `GET /api/shell` returns the app registry and shell state. `shell_boot.js`
   calls `renderAppRail`, `renderAppDock`, `renderAppLauncher`,
   `renderStatusInstrument`, and `installLauncherInteractions`.
3. The workspace fragment for the current app is fetched and mounted.
4. The fragment's script(s) run; each app makes **at most one initial-state
   fetch** before first meaningful paint (see below), then subscribes to its
   events.

Facet selection is workspace-local and uses the owning workspace's URL/query
contract. The day, where an app is day-scoped, lives in the URL path
(`/app/{name}/{YYYYMMDD}`). The server never embeds workspace state in HTML.

## `GET /api/shell` (contract sketch)

```json
{
  "version": "0.7.0",
  "apps": [
    {
      "name": "home",
      "label": "home",
      "icon": "🏠",
      "icon_svg": "<svg …>",
      "date_nav": false,
      "app_bar": true,
      "launcher_group": "your_journal",
      "launcher_rank": 0,
      "rail_group": "primary",
      "rail_rank": 0,
      "workspace_url": "/app/home/workspace",
      "background_url": null
    }
  ],
  "settings": { "reporting_enabled": true }
}
```

Notes:

- Each app has 12 fields: `app_bar`, `background_url`, `date_nav`, `icon`,
  `icon_svg`, `label`, `launcher_group`,
  `launcher_rank`, `name`, `rail_group`, `rail_rank`, and `workspace_url`.
  The grouped launcher sorts by launcher group/rank. The pinned rail is the
  independent non-null rail group/rank projection; it is not a persisted
  starred or user-reordered list.
- `workspace_url` always points at the static workspace fragment route.
  `background_url` points at the static background fragment route when present;
  a `null` `background_url` means the app registers no background service.
- The shell endpoint carries **state the chrome needs on every page**, nothing
  app-specific. App state never rides on `/api/shell`.

## Per-app conventions

- **Initial state:** `GET /app/{name}/api/state`, with context as query
  params (`?day=YYYYMMDD`, `?facet={name}`) where the app is day- or
  facet-scoped. Apps whose existing granular APIs already serve first paint in
  one request may use those instead — the rule is *first paint costs at most
  one state fetch*, not endpoint proliferation.
- **All JSON routes** follow the HTTP API conventions in `CONVEY.md`
  (`/api/` namespacing, resource envelopes, `error_response` reason codes).
- **Workspace fragments contain no template constructs.** Anything the server
  used to interpolate becomes a field on the state endpoint.
- **Scoped styles:** fragment CSS stays prefixed per app (e.g. `.tr-`) as
  today.
- **Background services** register via
  `AppServices.register(appName, service)` from the app's background script,
  loaded by the shell from `background_url`.

## Rendering rules

- Build DOM with `document.createElement`/`textContent`, `<template>`
  clones, or template literals passed through `AppServices.escapeHtml` for
  every interpolated value. Never assign unescaped data to `innerHTML`.
- Markdown renders only through `AppServices.renderMarkdown` (sanitized).
- One renderer per event type: the same function renders an item whether it
  arrived from the initial-state fetch or from a live SSE/WS event. Dual
  render paths (server-rendered history + client-rendered live events) are a
  defect class, not a pattern.

## Loading, error, and empty states

Every workspace must be honest about what it knows:

- **Loading:** render an explicit loading state (skeleton or spinner with
  label) between mount and first data paint — never a blank region.
- **Failure:** a failed initial-state fetch renders a visible error state with
  the server's reason and a retry affordance (`ApiError` carries
  `reasonCode`/`correlationId`). Never render an empty-looking page on error —
  "no data" and "couldn't load" are different states and must look different.
- **Partial data:** when one source fails and others succeed, show what loaded
  and mark the gap; don't present partial data as complete.
- **Empty:** a true empty state says what would appear here and, where
  applicable, how to get it.

## Testing

- Component-level behavior is exercised by the static test pages under
  `convey/static/tests/` — keep them green, add pages for new shared helpers
  (the mount helper, render helpers).
- Route-level behavior is unit-tested per app (`apps/{name}/tests/`).
- A CI guard enforces the architecture: no Jinja constructs in served templates,
  and no flask `render_template` outside the PDF modules.
