# Project Structure

## Directory Layout

```text
solstone/
├── observe/        # Multimodal capture & AI analysis
├── think/          # Data post-processing, AI agents & orchestration
│   └── sol_cli.py  # Unified CLI entry point (run: sol <command>)
├── convey/         # Web app frontend & backend
├── solstone/apps/           # Convey app extensions (see docs/APPS.md)
├── talent/           # Agent/generator configs + Agent Skills (talent/*/SKILL.md)
├── tests/          # Pytest test suites + test fixtures under tests/fixtures/
├── docs/           # All documentation (*.md files)
├── AGENTS.md       # Development guidelines (this file)
├── CLAUDE.md       # Symlink to AGENTS.md for Claude Code
└── README.md       # Project overview
```

Each package has a README.md symlink pointing to its documentation in `docs/`.

## Package Organization

- **Python**: Requires Python 3.11+
- **Modules**: Each top-level folder is a Python package with `__init__.py` unless it is data-only (e.g., `tests/fixtures/`)
- **Imports**: Prefer absolute imports (e.g., `from solstone.think.utils import setup_cli`) whenever feasible
- **Entry Points**: Commands are registered in `solstone/think/sol_cli.py`'s `COMMANDS` dict (pyproject.toml defines the `sol` and `journal` entry points)
- **Journal**: Data stored under `journal/` at the project root; day content lives under `journal/chronicle/`
- **Calling**: When calling other modules as a separate process always use the registered CLI surface and never call using `python -m ...` (e.g., use `journal indexer`, NOT `python -m solstone.think.indexer`)

## CLI Routing

`solstone/think/sol_cli.py`'s `COMMANDS` dict maps command names to module paths. The unified CLI is `sol`. Run `sol` to see status and available commands. `sol call` routes to `solstone/think/call.py`, which discovers `solstone/apps/*/call.py` Typer sub-apps and mounts them as subcommands.

## Agent & Skill Organization

`solstone/talent/*.md` stores agent personas and generator templates. Apps can add their own in `solstone/apps/*/talent/*.md`. Skills live at `solstone/talent/*/SKILL.md` and are symlinked into `journal/.agents/skills/` and `journal/.claude/skills/` via `sol skills install --project`, wrapped by `make skills`.

## File Locations

- **Entry Points**: `solstone/think/sol_cli.py` `COMMANDS` dict
- **Test Fixtures**: `tests/fixtures/journal/` - complete mock journal
- **Live Logs**: `journal/health/<service>.log`
- **Agent Personas**: `solstone/talent/*.md` (apps can add their own in `solstone/talent/`, see [docs/APPS.md](docs/APPS.md))
- **Generator Templates**: `solstone/talent/*.md` (apps can add their own in `solstone/talent/`, see [docs/APPS.md](docs/APPS.md))
- **Agent Skills**: `solstone/talent/*/SKILL.md` - symlinked into `journal/.agents/skills/` and `journal/.claude/skills/` via `sol skills install --project`, wrapped by `make skills`; read https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices to create the best skills
- **Scratch Space**: `scratch/` - git-ignored local workspace
