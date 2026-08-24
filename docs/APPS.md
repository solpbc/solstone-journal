# Convey apps

Convey apps are native. There is no `solstone/apps/` plugin directory and no
`app.json` / `routes.py` discovery.

## Where a surface lives

Register the app in
`core/crates/solstone-core-convey-shell/src/registry.rs` (`APP_REGISTRY`).
That table is the metadata `app.json` used to carry: name, icon, label, date
nav, facets.

The HTTP surface is a `*-web` crate (`solstone-core-health-web`,
`solstone-core-home-web`, …) or assets under
`core/crates/solstone-core-convey-shell/assets/<app>/`.

Frontend conventions live in [CONVEY-FRONTEND.md](CONVEY-FRONTEND.md).
Framework-level shell changes live in [CONVEY.md](CONVEY.md).

`solstone call <app> <verb>` authority lives under `core/native-sol/apps/<app>/`.

## Shell chrome metadata

`/api/shell` exposes each registry row as a 13-field `ShellApp`: `app_bar`,
`background_url`, `date_nav`, `facets_enabled`, `icon`, `icon_svg`, `label`,
`launcher_group`, `launcher_rank`, `name`, `rail_group`, `rail_rank`, and
`workspace_url`. There is no starred-app state.

The launcher renders every app once by `launcher_group` and `launcher_rank`.
Apps with a non-null `rail_group` render in the pinned rail by `rail_rank`;
the `primary` and `management` rail groups are independent of launcher
grouping.

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
