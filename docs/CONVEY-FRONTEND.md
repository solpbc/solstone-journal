# Convey Frontend Conventions

**Status:** binding for all convey UI work. Companion to `CONVEY.md` (HTTP API
conventions) and `APPS.md` (app development guide).

Convey's owner-facing web UI is a **pure client-rendered application**: JSON
APIs + SSE/WS events + static files. The server composes no HTML for the
serving path.

## Architecture: static shell + per-app workspaces

- **One static shell**
  (`core/crates/solstone-core-convey-shell/assets/static/shell.html`) is served
  unconditionally for every `/app/{name}` route. The client derives the
  current app from `location.pathname` and boots from the app table that ships
  in the shell's script bundle plus `GET /api/shell`, the per-request part.
- **Per-app workspace fragments** stay one file per app
  (`assets/workspace.html` or `assets/{name}/workspace.html` in the crate that
  carries the app, such as
  `core/crates/solstone-core-home-web/assets/workspace.html`): markup + `<style>` + `<script>`, served
  verbatim as a static asset — zero server-side template processing. The shell
  fetches the fragment, mounts it into `<main>`, and re-executes its scripts in
  document order via the shared mount helper.
- **No framework, no build step for authors.** Vanilla JS (ES modules or classic scripts),
  template literals, `<template>` elements where they help. Static files are
  compiled into the journal binary as-is and served by axum. Consistency comes from the shared helpers and design
  tokens (`static/tokens.css` and `static/tokens-dark.css`), not from a framework;
  `init.html` links `tokens-dark.css` immediately after `tokens.css`. `shell.html` lists it behind
  `media="(prefers-color-scheme: dark)"`, and declares `color-scheme` in its own `app.css` to
  stand in for what that query hides. The served shell carries it inside `shell.css` (below), in
  an `@media (prefers-color-scheme: dark)` block at its place, so the cascade is the same, and its
  bytes load with the rest of that one stylesheet.
- **The shell is served as three files.** `shell.html` keeps listing every
  stylesheet and deferred script it loads, in order; that list is the source.
  Each script tag says `data-shell-part="boot"` or `data-shell-part="rest"`.
  The convey shell's `build.rs` concatenates them, unchanged, into
  `/static/shell.css`, `/static/shell.js` (boot) and `/static/shell-rest.js`
  (`core/build-support/convey_shell_bundle.rs`), with a `/* file: … */` banner
  per file. The served page names the stylesheet and the boot script. The rest
  script is a placeholder with no `src` until boot fetches it, after the first
  view is ready. Nothing is minified or transpiled, and no other toolchain is
  involved.
  - Each script runs inside its own function, so a file that throws stops only
    itself; its error still reaches `window`'s `error` event, named by its file
    and line (the diagnostic console maps bundle positions; `shell.js.map` and
    `shell-rest.js.map` do the same for developer tools).
  - Every name a file declares at its top level stays a global, read and
    written through the same binding, so `relativeTime` works from any file as
    before. The build refuses a name declared at the top of two files, a
    top-level destructuring declaration, and anything its lexer can't read (an
    unclosed string or bracket, a `/` it can't tell is division or a regular
    expression). A full parse of every file and of both script bundles runs
    under Node in `make ci-full`.
  - A new shell script or stylesheet is added to `shell.html` like any other.
    It must be a plain `/static/` file, and a script must be a classic `defer`
    script (no `type`, no `async`) with `data-shell-part="boot"` or `"rest"`.
- **Precompressed.** Every compressible dashboard file the shell's build knows
  (the shell's own and each web crate's recorded assets) is gzipped once at
  build time. A client that accepts gzip gets that copy, with
  `Content-Encoding: gzip`, one `Vary: accept-encoding` and the `"<tag>-gzip"`
  validator, as on-the-fly compression gave. JSON reads and filled templates
  are still compressed per response.
- **`AppServices`** (`static/app.js`) is the shared client runtime: service
  and task registration, notifications, `renderMarkdown` (marked + DOMPurify),
  `escapeHtml`. **`apiJson`/`ApiError`** (`static/api.js`) is the only HTTP
  client. **`appEvents`** (`static/websocket.js`) is the only event transport.

## Shell boot sequence

1. Static shell parses with `#app-rail`, `#app-dock`, and `#status-instrument`
   slots. An inline script in its `<head>` starts, beside the download of the
   shell's stylesheet and boot script, `GET /api/shell`, the stream labels, and from the entry
   table (below) the view's fragment, its scripts and styles, and its first
   reads.
2. `shell_boot.js` reads `/api/shell` through `apiJson` with `scope: 'shell'`,
   which takes the answer the head started (below). It
   joins `app_names` with the app table the bundle carries
   (`window.SOL_SHELL_REGISTRY`, generated from the registry at build time) and
   hands the rest of the shell one object with `apps`, `settings`, `version`,
   `clock` and `pinned`. `shell_boot.js` calls `renderAppRail`, `renderAppDock`, and
   `renderStatusInstrument`. The rail's top group is home, the resolved pins,
   and more. The dock is home, the first two pins, and more. Status sits in
   the desktop rail or mobile dock, with a tooltip and a link to Health details.
   The same status node survives navigation rebuilds. Health mounts the shared
   status details section after its workspace loads. `/app/more/` is the page
   that lists every other view.
3. The workspace fragment for the current app is fetched and mounted.
4. The fragment's script(s) run; each app makes **at most one initial-state
   fetch** before first meaningful paint (see below), then subscribes to its
   events. A view on the view contract (below) is mounted instead, after its
   scripts register it.

Facet selection is workspace-local and uses the owning workspace's URL/query
contract. The day, where an app is day-scoped, lives in the URL path
(`/app/{name}/{YYYYMMDD}`). The server never embeds workspace state in HTML.

## The entry table and early reads

**The table** (`window.__SOL_ENTRY_TABLE__`) is generated at build time from
each view's fragment and written into the served shell page
(`core/build-support/convey_entry_table.rs`): per view, its fragment, its
scripts (hashed, with any `data-route-prefix`) and styles, and its first reads.
The shell page is `no-cache` with a validator, so the table always matches the
names the journal is serving.

**A fragment declares its first reads** in a comment beside its markup:

```html
<!-- first-read: /app/home/ -> /app/home/api/pulse /app/home/api/briefing -->
<!-- first-read: /app/transcripts/{day} -> /app/transcripts/api/day/{day} -->
```

The route is a whole path; `{day}` (eight digits) is its only variable. Declare
only what the view's own code asks for at that route with no other state, and
never a date the path doesn't carry: the page doesn't work out dates. A query
string or a hash on the address starts no reads, so the view asks for itself.
`<!-- open-day: ROUTE -> URL -->` names a read that picks the day of an undated
route; the shell moves to that address before the view mounts, so the view only
ever sees a day in its path (transcripts' root).

**Early reads are taken by exact address, once.** `boundedRequest` and the
`fetch` wrapper in `api.js` take a read the head started instead of asking
again. It keeps its in-flight request whatever its age, under what's left of
its first-byte bound counted from when it started, and past that it ends in the
view's error state, like any read. It's asked for again only for an answer the
ordinary path has to handle itself: a redirect, a non-OK status or the wrong
type (a `502`, `503` or `504` spends one of the ordinary retries). A view's
mount owns the early reads: a different view or a later mount ends them, and
`view:ready` ends whatever nothing took. Nothing is kept: early answers stay in
memory, and reads stay `no-store`.

## The view contract and moving between views

**A view on the contract** declares it in its fragment with
`<!-- view-contract -->`, and its script only registers:

```js
window.SolViews.register('transcripts', {
  mount(ctx) {
    // draw into the fragment's markup, read, subscribe
    return { update(params) { /* a new day in place */ }, dispose() {} };
  }
});
```

`mount(ctx)` returns its dispose function, or `{ dispose, update }` when the
view takes a change of parameters in place. A view's scripts run once in the
life of the page (they only register); the view starts each time it is
mounted. Home (with its week and originals pages), transcripts and more are
on the contract; every other view is not, and keeps its full page loads.

**Everything a view adds outside its own markup goes through `ctx`**, and is
taken back when the view goes: `ctx.on(target, type, fn, options)` for the
document, the window or any other target that outlives the markup;
`ctx.setTimeout`/`setInterval`/`visibleInterval`; `ctx.observe(observer)`;
`ctx.listen` and `ctx.onConnectionState` for the live stream;
`ctx.global(name, value)`; `ctx.bodyClass(name, on)`; `ctx.title(text)`;
`ctx.history.push/replace(url)` for the view's own entries;
`ctx.onDispose(fn)`. `ctx.signal` aborts when the view goes. A mount that
throws part-way takes back what it had added, so "try again" mounts the view
again in place. A view's stylesheets stay in the page, switched off while
another view is shown.

**Reads go through `ctx.read(url, opts)` (JSON, through `apiJson`) and
`ctx.fetch`** (`static/store.js`). One read of an address is in flight per
mount: a second ask while it is out shares it. A read asked because something
changed passes `fresh: true` and is asked of its own. A view's reads are
aborted when it goes and never answer after that, so a view that has gone
can't draw. Writes go out unchanged and are never cut short. Memory only.

**Moving between two contract views stays in the page** (`static/router.js`).
A click on a link, or a day step, between two contract views swaps `<main>`;
the rail, the status mark, the sun arc and the live stream stay. Anything else
is a whole page load: to or from another view, out of the journal, or when
the page has learned the journal is another build (`appEvents.buildChanged()`)
or doesn't know the destination's app. Each history entry has one owner: an
entry that changes the address path is the router's; one that keeps the path
and changes only the query or the hash is the view's (a transcript segment),
and the router leaves it to the view. A move starts the next view's declared
first reads at once, in a table of their own, adopted as the head's are. The
swap runs in a view transition once those reads answer or 300 ms pass. Focus
moves to the first heading in the view's own markup, or to `<main>` when the
view draws its headings itself (a drawn heading can be drawn again, which would
drop focus to the page). `#view-announcer` says the view's name, and scroll is
restored per history entry. Code is fetched ahead on a press or
a hover on a rail entry; data never is. A view whose first load ended in its
error state reads again when the live stream next connects, once per connect.

**Loading states in a contract view** don't set `aria-busy`
(`SurfaceState.loading({ text, busy: false })`), so they are announced as they
appear. Their "still working…" line is hidden from assistive technology; the
page says it instead, once, when the view's first load passes ten seconds.

## `GET /api/shell` (contract sketch)

The shell's per-request part, `no-store`, read on every page:

```json
{
  "app_names": ["activities", "backup", "body", "…", "home", "more", "…"],
  "version": "2.0.73",
  "clock": { "tz": "America/Denver", "label": "Denver", "today": "20261109" },
  "pinned": ["search", "entities", "thinking"]
}
```

- `app_names` lists the apps this router serves, in order: the registry, then
  `agents` when its routes are mounted. That can differ between routers, so it
  stays here, not in the shipped table.
- `version` is the journal's version. It stays here so a release doesn't change
  the shell's cached files when none of them changed.
- `clock` is the journal's zone, its label, and the journal's day key `today`
  (for clients with no zone table of their own, like the `solstone` CLI).
- `pinned` is the resolved `convey.pinned_views` list.

The app definitions ship with the code, in `shell.js`, as
`window.SOL_SHELL_REGISTRY`:

```json
{
  "apps": [
    {
      "name": "home",
      "label": "home",
      "icon": "🏠",
      "icon_svg": "<svg …>",
      "date_nav": false,
      "app_bar": true,
      "pinnable": false,
      "focus": { "group": "keeps", "rank": 0 },
      "description": "today and yesterday, written up, and anything your journal needs from you",
      "source": "written by your journal from everything in it",
      "keywords": ["today", "yesterday", "briefing", "week"],
      "rail_group": null,
      "rail_rank": 0,
      "workspace_url": "/app/home/workspace",
      "background_url": null
    }
  ],
  "settings": { "reporting_enabled": true }
}
```

Notes:

- The table holds every app any router can serve, the agents app included.
  Each app carries `app_bar`, `background_url`, `date_nav`, `description`,
  `focus`, `icon`, `icon_svg`, `keywords`, `label`, `name`, `pinnable`,
  `rail_group`, `rail_rank`, `source`, and `workspace_url`. `focus` is
  `{ "group": "keeps"|"shape"|"runs", "rank": <number> }`, or `null` on
  `more`. `rail_group` is `"management"` or `null`. The side rail's top group
  is home, then `pinned`, then more.
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

## Read bounds and the live stream

**Reads are bounded.** `api.js` gives every read two clocks, never a total:
a first-byte bound (30 s, or 60 s on the single-flight routes listed in
`api.js`) and a 20 s stall bound that every received byte resets. The bounds
apply to `apiJson`, the fragment fetch and every same-origin GET or HEAD
through `window.fetch`, which `api.js` wraps. They don't apply to `/sse/`,
`keepalive` requests or writes: a write has no deadline, so the client never
gives up on one that may still succeed. A read past its bound ends in the
view's error state. A loading state shows "still working…" after 10 s.

**Retries are for transport only.** A GET through `apiJson` or the fragment
fetch is retried at most twice, about 1 s and then 3 s apart with jitter,
after a network error or a `502`, `503` or `504`. A plain `fetch` read is not
retried. A read that passed its bound is not retried either, because the
server is probably still running it.

**`apiJson` reads belong to their mount.** A newer in-page mount aborts the
`apiJson` reads an older one started, and their answers are never drawn.
Shell-level reads pass `scope: 'shell'` and outlive mounts. A plain `fetch`
read needs its own `signal`.

**`/sse/events` wire contract.** Changes to it are additive only: the phone
runtime in `tools/mobile-serving` and `tools/journal_device_sim` read it too.
- On connect: `event: hello`, `retry: 2000` and
  `data: {"server_build":"<16 hex>"}`. `server_build` changes when the version
  or any recorded asset changes. A page that sees it change does one full load
  on its next in-page mount.
- Every 5 s: `event: heartbeat` with `data: {"at":<unix ms>}`, in the same
  frame as a `: heartbeat` comment. The 15 s keep-alive comment also stays.
- No `id:`. The stream has no replay, so it doesn't promise a resume.

**The client heals the stream** (`websocket.js`). After 15 s with no event it
closes the stream and asks `/api/system/about` at once:
- an OK answer opens a new stream;
- a `401` or `403` takes the sign-in path, without a reconnect loop (a `403`
  carrying the phone proxy's refusal marker reloads the page once instead);
- no answer, or any other answer, asks again on a backoff that grows to every
  15 s, with jitter. A new stream that fails after an OK answer also waits on
  that backoff, so the client never loops.

The `online` event, or the page coming back into view, asks at once when the
stream is already known lost. Otherwise a page coming back into view gives
its buffered events 3 s before it judges the stream silent. The status mark
never shows "connected" when the last event is more than 40 s old.

## Testing

- Component-level behavior is exercised by the static test pages under
  `core/crates/solstone-core-convey-shell/assets/static/tests/` — keep them green, add pages for new shared helpers
  (the mount helper, render helpers).
- Route-level behavior is tested in each app's crate: Rust tests next to the
  routes, and, where a workspace has them, DOM checks in `tests/*_dom.js`, each
  driven by a matching `tests/*_dom.rs`.
- The serving path has no template engine: pages are static files compiled into
  the binary.
