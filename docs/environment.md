# Environment

## Journal Path

`get_journal()` / `get_journal_info()` in `solstone.think.utils` are the canonical journal resolvers. Trust them unconditionally.

Resolver order (with the source label `get_journal_info()` returns):

1. `SOLSTONE_JOURNAL` env var, when set and non-empty → `"env"`
2. `~/.config/solstone/config.toml`, when it has a non-empty `journal = "..."` key → `"config"`
3. source-tree fallback: `<project_root>/journal` when both `<project_root>/pyproject.toml` and `<project_root>/.git` exist → `"source"`
4. built-in default: `~/journal` → `"default"`

`get_journal_info()` no longer raises — there is always a resolved path. `get_journal()` raises `SolstoneNotConfigured` only when `os.makedirs` on the resolved path fails.

Who sets `SOLSTONE_JOURNAL`:

- Installed runs: the managed wrapper at `~/.local/bin/sol`
- Unit tests: the `set_test_journal_path` autouse fixture in `tests/conftest.py`
- Makefile sandboxes: explicit per-command env injection in `make sandbox` / verify targets

Who must **not** set it:

- application code
- service files
- agent prompts
- ad hoc subprocess environments spawned by app code

If you think you need to set `SOLSTONE_JOURNAL` from application code, fix the actual resolution problem instead.

## Service Installation

There are two install paths and they handle journal resolution differently. For a fresh source checkout, `.venv/bin/journal setup` installs managed bash wrappers at `~/.local/bin/sol` and `~/.local/bin/journal`, then installs solstone as a systemd user service (Linux) or launchd agent (macOS) with convey on port 5015. After the first run, the wrappers let you use `sol` and `journal` from anywhere; override with `.venv/bin/journal setup --port 8000` on the first run or `journal setup --port 8000` after the wrapper exists. For packaged installs (`uv tool install solstone` or `pipx install solstone`), `sol` and `journal` are installed directly at `~/.local/bin/` as tool entry points — there are no managed bash wrappers, and `SOLSTONE_JOURNAL` is not exported in the service env block; the default journal location resolves via `get_journal()`. Both paths install the service.

Installed services invoke `~/.local/bin/journal`. They do **not** write `SOLSTONE_JOURNAL` into the service env block; the wrapper exports it before execing the venv `journal`.

Use:

- `journal config show` to display the resolved journal path, user-facing source label, and wrapper status
- `journal config journal <path>` to atomically rewrite the wrapper's embedded journal path
- `journal service <install|start|stop|restart|status|logs>` for service lifecycle management

## API Keys

Store API keys in `.env` file, never commit to repository.

## Error Handling & Logging

- Raise specific exceptions with clear messages
- Use logging module, not print statements
- Validate all external inputs (paths, owner data)
- Fail fast with clear errors - avoid silent failures

## Documentation

- Update README files for new functionality
- Code comments explain "why" not "what"
- Function signatures should include type hints; highlight gaps when touching older modules
- **All docs in `docs/` plus journal references in `solstone/talent/journal/`**: Browse `solstone/talent/journal/SKILL.md`, APPS.md, CORTEX.md, CALLOSUM.md, THINK.md, and more
- Each package has a README.md symlink pointing to its documentation in `docs/`.
- **App/UI work**: [docs/APPS.md](docs/APPS.md) is required reading before modifying `solstone/apps/`

## Git Practices

- **Git**: Small focused commits, descriptive branch names. Run git commands directly (not `git -C`) since you're already in the repo.

## Getting Help

- Run `sol` for status and CLI command list
- Check [docs/DOCTOR.md](docs/DOCTOR.md) for debugging and diagnostics
- Browse `docs/` for all subsystem documentation
- Review test files in `tests/` for usage examples
