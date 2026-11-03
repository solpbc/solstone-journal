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

- the installed `~/.local/bin/solstone` / `journal` wrapper
- a test, explicitly
- `make sandbox` / `make dev`, explicitly

Who must not: application code, service files, agent prompts, ad hoc
subprocesses spawned by app code.

Use:

- `solstone journal config show` — resolved path and source
- `solstone journal config journal <path>` — rewrite the wrapper's embedded path
- `solstone journal service <install|start|stop|restart|status|logs>` — service lifecycle

## Service Installation

On Linux, `solstone journal setup` installs the `solstone` and `journal` wrappers
and the systemd user service. Convey listens on port 5015.
Service units do not write
`SOLSTONE_JOURNAL` into the service env block; the wrapper exports it.

On macOS, the journal app owns the runtime and starts the supervisor. It does
not install PATH wrappers or a separate launchd service.

See [INSTALL.md](../INSTALL.md).

## API Keys

Owner cloud keys live in `config/journal.json` under `env`. Do not commit
keys. There is no required `.env` file for the native journal.

## Desktop local HTTP endpoint

On macOS and Linux, the configured journal serves an owner-only pathname socket
beside its existing TCP listener. The default is the account temporary directory
plus `solstone/journal.sock` on macOS, and the OS account home directory plus
`.local/run/solstone/journal.sock` on Linux. `SOLSTONE_JOURNAL_SOCKET` selects an
explicit socket and its sibling `journal.json` runtime record. Give each journal
its own directory when overriding this path. New directories use mode `0700`; an
existing directory must already have that mode. Other journal roots run TCP-only
unless they have an explicit socket override.

An explicit `SOLSTONE_CONVEY_PORT` selects TCP. Otherwise the CLI prefers an
existing socket owned by its OS account, then falls back to `localhost:5015`.
It reads no journal-local files. An empty socket override selects TCP-only for
internal children. Internal callers use their own journal's published endpoint.
Windows and embedded mobile hosts keep their existing transport.
