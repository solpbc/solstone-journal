# Project Structure

## Directory Layout

```text
solstone/
├── observe/        # Multimodal capture & AI analysis
├── think/          # Data post-processing, AI agents & orchestration
├── convey/         # Web app frontend & backend
├── solstone/apps/           # Convey app extensions (see docs/APPS.md)
├── talent/           # Agent/generator configs + sol/journal router skills
├── tests/          # Pytest test suites + test fixtures under tests/fixtures/
├── docs/           # All documentation (*.md files)
├── AGENTS.md       # Development guidelines (this file)
├── CLAUDE.md       # Symlink to AGENTS.md for Claude Code
└── README.md       # Project overview
```

Each package has a README.md symlink pointing to its documentation in `docs/`.

## Package Organization

- **Python**: Requires Python 3.12+
- **Modules**: Each top-level folder is a Python package with `__init__.py` unless it is data-only (e.g., `tests/fixtures/`)
- **Imports**: Prefer absolute imports (e.g., `from solstone.think.utils import setup_cli`) whenever feasible
- **Entry Points**: Public launchers select sibling `solstone-core-sol` (API-only) or `solstone-core-journal` (same-device) executables
- **Journal**: Data stored under `journal/` at the project root; day content lives under `journal/chronicle/`
- **Calling**: When calling other modules as a separate process always use the registered CLI surface and never call using `python -m ...` (e.g., use `journal indexer`, NOT `python -m solstone.think.indexer`)

## CLI Routing

The public `sol` / `solstone` launchers exec `solstone-core-sol`, which reaches the journal only through API transport. The `journal` launchers exec `solstone-core-journal`; its Rust parser owns local primitives and a closed process table for journal services.

## Agent & Skill Organization

`solstone/talent/*.md` stores agent personas and generator templates. The installed project skills are the two router skills at `solstone/talent/sol/` and `solstone/talent/journal/`. App command fragments under `solstone/apps/*/talent/*/SKILL.md` are builder source for generated router references, not top-level installed skills.

## File Locations

- **Entry Points**: `core/crates/solstone-core-sol/` and `core/crates/solstone-core-journal-cli/`
- **Test Fixtures**: `tests/fixtures/journal/` - complete mock journal
- **Live Logs**: `journal/health/<service>.log`
- **Agent Personas**: `solstone/talent/*.md` (apps can add their own talent files under `solstone/apps/*/talent/`, see [docs/APPS.md](docs/APPS.md))
- **Generator Templates**: `solstone/talent/*.md` (apps can add their own talent files under `solstone/apps/*/talent/`, see [docs/APPS.md](docs/APPS.md))
- **Agent Skills**: `solstone/talent/{sol,journal}/SKILL.md` - the two router skills installed into `journal/.agents/skills/` and `journal/.claude/skills/`; app `SKILL.md` fragments feed generated references via `make skills` (`scripts/build_skill_references.py`)
- **Scratch Space**: `scratch/` - git-ignored local workspace
