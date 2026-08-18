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

`sol call <app> <verb>` authority lives under `core/native-sol/apps/<app>/`.

## Journal storage

A running journal still has `solstone/apps/<name>/` for per-app state. That
path is journal data, not the codebase. See
[storage.md](../core/payload/solstone/talent/journal/references/storage.md).

## Adding an app

1. Add a row to `APP_REGISTRY`.
2. Add a `*-web` crate or assets under convey-shell.
3. Wire the router into convey-shell.
4. If the app has `sol call` verbs, add `core/native-sol/apps/<name>/`.

Do not recreate `solstone/apps/` in this repository.

Talent `load` frontmatter (which sources a generator reads) is documented in
[THINK.md](THINK.md#prompt-context-configuration).
