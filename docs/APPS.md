# Convey apps

Convey apps are native. There is no `solstone/apps/` plugin directory and no
`app.json` / `routes.py` discovery.

## Where a surface lives

Register the app in
`core/crates/solstone-core-convey-shell/src/registry.rs` (`APP_REGISTRY`).
That table is the metadata `app.json` used to carry: name, icon, label, and
date navigation.

The HTTP surface is a `*-web` crate (`solstone-core-health-web`,
`solstone-core-home-web`, …) or assets under
`core/crates/solstone-core-convey-shell/assets/<app>/`.

Frontend conventions live in [CONVEY-FRONTEND.md](CONVEY-FRONTEND.md).
Framework-level shell changes live in [CONVEY.md](CONVEY.md).

`solstone call <app> <verb>` authority lives under `core/native-sol/apps/<app>/`.

## Shell chrome metadata

`/api/shell` exposes each registry row with the fields the chrome reads:
`app_bar`, `background_url`, `date_nav`, `description`, `focus`, `icon`,
`icon_svg`, `keywords`, `label`, `name`, `pinnable`, `rail_group`,
`rail_rank`, `source`, and `workspace_url`. `focus` is
`{ "group": "keeps"|"shape"|"runs", "rank": <number> }`, or `null` on `more`.
`rail_group` is `"management"` or `null`.

`pinned` is not stored on that registry payload. Each `GET /api/shell`
inserts it for that request, the same way it inserts `clock`: the resolved
`convey.pinned_views` list from `config/journal.json`. The side rail's top
group is home, then those pins, then `more`. The bottom group is the
management rail (import, settings, network, and agents when agent routes are
mounted), ordered by `rail_rank`. `/app/more/` lists every served view except
more: what it shows, where its contents come from, and a pin control for each
pinnable view.

`PUT /api/shell/pins` with `{"view","pinned"}` writes only
`convey.pinned_views` and returns `{"pinned":[...]}`.

## Journal storage

A running journal still has `solstone/apps/<name>/` for per-app state. That
path is journal data, not the codebase. See
[storage.md](../core/payload/solstone/talent/journal/references/storage.md).

## Adding an app

1. Add a row to `APP_REGISTRY`.
2. Add a `*-web` crate or assets under convey-shell.
3. Wire the router into convey-shell.
4. If the app has `solstone call` verbs, add `core/native-sol/apps/<name>/`.

Do not recreate `solstone/apps/` in this repository.

Talent `load` frontmatter (which sources a generator reads) is documented in
[THINK.md](THINK.md#prompt-context-configuration).
