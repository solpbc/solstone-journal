# Project Structure

## Directory Layout

```text
core/crates/                 # Native Rust crates (convey-shell, *-web, ingest, transcribe, describe, think, ...)
core/native-sol/             # solstone call authority declarations
core/payload/solstone/talent/  # Agent/generator configs + solstone/journal router skills
solstone/                    # detect_created spec + one JSON schema only, no code
docs/                        # Longform documentation
AGENTS.md                    # Development guidelines
README.md                    # Project overview
```

## Package Organization

- **Language**: Rust workspace under `core/`, edition 2024, MSRV 1.95 (`core/Cargo.toml`)
- **Crates**: one directory per crate under `core/crates/`, each a normal Cargo package (`Cargo.toml` + `src/`)
- **Entry Points**: `solstone-core-sol` provides the unified CLI; `solstone-core-journal` provides the functional `journal` alias
- **Journal**: Data stored under `journal/` at the project root; day content lives under `journal/chronicle/`
- **Calling**: When calling other modules as a separate process, always use the registered CLI surface (`solstone call <app> <verb>` or `solstone journal <cmd>`), never a private binary or subprocess path

## CLI Routing

The public `solstone` launcher execs `solstone-core-sol`. `solstone call` uses API transport; `solstone journal` enters the same-device runner also used by the `journal` alias. That runner's Rust parser owns local primitives and a closed process table for journal services.

## Agent & Skill Organization

`core/payload/` is the shipped payload: everything the installed binary reads at runtime, staged in the repository under the same relative paths it has once installed, so `core/payload/` is the checkout's stand-in for the installed `share/` prefix. `core/payload/solstone/talent/*.md` stores the talent prompts and generator templates. There is no separate Python post-hook tree — every possible talent write is a typed `WriteIntent` variant in `core/crates/solstone-core-talent-runtime/src/writers.rs` (see `AGENTS.md` §7 L8). The installed project skills are the two router skills at `core/payload/solstone/talent/solstone/` and `core/payload/solstone/talent/journal/`. App command fragments under `core/payload/solstone/apps/*/talent/*/SKILL.md` are optional reference guidance, not top-level installed skills.

## File Locations

- **Entry Points**: `core/crates/solstone-core-sol/` and `core/crates/solstone-core-journal-cli/`
- **Test Fixtures**: `tests/fixtures/journal/` - complete mock journal
- **Live Logs**: `journal/health/<service>.log`
- **Talent Prompts**: `core/payload/solstone/talent/*.md`
- **Generator Templates**: `core/payload/solstone/talent/*.md`
- **Agent Skills**: `core/payload/solstone/talent/{solstone,journal}/SKILL.md` - the two router skills installed into `journal/.agents/skills/` and `journal/.claude/skills/`; app `SKILL.md` fragments hold additional guidance, while live CLI help lists current commands
- **Scratch Space**: `scratch/` - git-ignored local workspace
