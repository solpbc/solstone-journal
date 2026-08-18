# Environment

## Journal Path

`solstone-core-journal::resolve_journal_path` is the resolver. Do not set
`SOLSTONE_JOURNAL` from application code.

Order:

1. `SOLSTONE_JOURNAL` when set and non-empty → `env`
2. `~/.config/solstone/config.toml` `journal = "..."` → `config`
3. checkout root `journal/` when the process knows it is in a source tree → `source`
4. `~/journal` → `default`

Who may set `SOLSTONE_JOURNAL`:

- the installed `~/.local/bin/sol` / `journal` wrapper
- a test, explicitly
- `make sandbox` / `make dev`, explicitly

Who must not: application code, service files, agent prompts, ad hoc
subprocesses spawned by app code.

Use:

- `journal config show` — resolved path and source
- `journal config journal <path>` — rewrite the wrapper's embedded path
- `journal service <install|start|stop|restart|status|logs>` — service lifecycle

## Service Installation

`journal setup` installs the `sol` and `journal` wrappers and the host service
(systemd user on Linux, launchd on macOS). Convey listens on port 5015.
Installed services invoke `journal` from PATH. They do not write
`SOLSTONE_JOURNAL` into the service env block; the wrapper exports it.

See [INSTALL.md](../INSTALL.md).

## API Keys

Owner cloud keys live in `config/journal.json` under `env`. Do not commit
keys. There is no required `.env` file for the native journal.
