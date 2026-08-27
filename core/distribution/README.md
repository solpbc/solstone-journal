# solstone-journal: for the agent helping install it

This file ships inside the installed tree, at `share/README.md`, next to `bin/` and `share/LICENSE`. If you are an agent (Claude, Codex, or similar) helping an owner get the journal running, start here.

## Where things are

- `~/.local/bin/solstone`: day-to-day CLI (status, help)
- `~/.local/bin/journal`: host operations (setup, service, doctor)
- `~/.config/systemd/user/solstone.service`: the systemd user unit
- `~/.local/solstone-journal/versions/<version>-<hash>/`: one immutable directory per installed version
- `~/.local/solstone-journal/current`: symlink to the live version; `current/bin` is what is on PATH
- `~/journal` (default, overridable): the owner's journal. Never move, delete, reformat, or otherwise touch its contents yourself.

## The one command that fixes almost everything

```
journal setup
```

Safe to re-run. It repairs config, fetches the transcription model, installs skill links, and reconciles the service unit. If it finds a leftover install from the earlier Python-based journal, installed via `pip`, `uv tool`, or `pipx` (its `solstone`, `journal`, and `sol` binaries under `~/.local/bin`), it stops that install's service, backs up its binaries, and replaces them with this one, automatically, in this one invocation.

Do not pre-uninstall the old install. Running `pip uninstall`, `uv tool uninstall`, or `pipx uninstall` yourself, or manually running `journal service stop` against the old install, before running `journal setup`, only removes the evidence setup needs to find and safely replace it. Just run `journal setup`.

Backups of anything setup replaces land at `~/.local/share/solstone/setup-backups/`, timestamped. Nothing is deleted outright.

## Diagnosing a stuck install

1. `solstone --version && journal service status`: is anything installed and running?
2. `journal doctor`: read-only. Reports what is missing (transcription runtime, models, the system OpenMP library) without changing anything.
3. `journal service logs`: if the service will not start.
4. `journal setup`: re-run it. It repairs a fresh, partial, or broken install the same way.

## Setup will not silently claim your journal

`journal setup` refuses to silently claim an existing journal it does not already own. Interactively it asks, defaulting to no. Non-interactively it refuses and names `--accept-existing-journal` as the explicit opt-in. If the journal lives somewhere other than `~/journal`, point setup at it directly:

```
journal setup --journal /path/to/journal --accept-existing-journal
```

## What setup does not see

Only artifacts under the owner's `$HOME` are examined. A `solstone`/`journal` install done as root, or into a system Python outside the owner's home directory, is invisible to this detection. If that is the owner's situation, that is a real gap in what setup can find automatically. Say so, and remove the old install by hand; there is no flag that covers it.

## Uninstalling

```
journal setup --clean-uninstall --yes
```

Removes the managed service, wrappers, config, and setup manifest. Never the journal.
