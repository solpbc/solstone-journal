# Sol CLI Developer Guide

How the `sol` CLI is organized, how to add new commands, and what files to maintain.

## Architecture

The CLI has two tiers with distinct purposes:

| Tier | Pattern | Framework | Purpose |
|------|---------|-----------|---------|
| **Top-level** | `sol <cmd>` | Custom dispatcher + argparse | System infrastructure — pipelines, daemons, orchestration |
| **Call** | `sol call <app> <cmd>` | Typer (auto-discovered) | Tool-callable functions — what agents and humans invoke for data operations |

### The boundary

**If an AI agent should tool-call it → `sol call`.** These commands appear in SKILL.md files and are invoked by talent agents during conversations.

**If it's system plumbing → `sol <cmd>`.** Processing pipelines, supervisor, services, capture — things that cron or systemd runs.

**Interactive entry points** (`sol chat`, `sol help`, `sol engage`) are top-level for discoverability even though they're user-facing. Agents don't invoke these.

## Top-Level Commands (`sol <cmd>`)

### How they work

`solstone/think/sol_cli.py` contains a static `COMMANDS` dict mapping command names to module paths:

```python
COMMANDS: dict[str, str] = {
    "think": "solstone.think.thinking",
    "import": "solstone.think.importers.cli",
    ...
}
```

Each module must export a `main()` function. The dispatcher does `importlib.import_module(path)` then calls `module.main()`.

Commands are organized into `GROUPS` for help display, and `ALIASES` provide shortcuts (e.g., `journal up` → `journal service up`).

### Adding a top-level command

1. **Create the module** with a `main()` function:

```python
# solstone/think/my_cmd.py
import argparse

def main() -> None:
    parser = argparse.ArgumentParser(prog="sol my-cmd")
    parser.add_argument("--day", help="Day YYYYMMDD")
    args = parser.parse_args()
    # ... implementation
```

2. **Register in `solstone/think/sol_cli.py`** — add to `COMMANDS`:

```python
COMMANDS: dict[str, str] = {
    ...
    "my-cmd": "solstone.think.my_cmd",
}
```

3. **Add to a group** in `GROUPS` for help display.

4. **No skill or AGENTS.md changes needed** — top-level commands aren't agent tools.

### Files to maintain

| File | What to do |
|------|-----------|
| `solstone/think/sol_cli.py` `COMMANDS` dict | Register the command |
| `solstone/think/sol_cli.py` `GROUPS` dict | Add to appropriate group |
| Module file (e.g., `solstone/think/my_cmd.py`) | Implement with `main()` |

## Call Commands (`sol call <app> <cmd>`)

### How they work

`solstone/think/call.py` is the gateway. It creates a root `typer.Typer()` and mounts sub-apps from two sources:

**Auto-discovered apps** — scans `solstone/apps/*/call.py` at import time:
```
apps/todos/call.py      → sol call todos ...
apps/activities/call.py → sol call activities ...
apps/entities/call.py   → sol call entities ...
```

Each `call.py` must export `app = typer.Typer()`. The directory name becomes the sub-command name. Errors in one app don't prevent others from loading.

**Manually mounted built-ins** — for commands tightly coupled to `solstone/think/` internals:
```python
# think/call.py
from solstone.think.tools.call import app as journal_app
from solstone.think.tools.navigate import app as navigate_app
from solstone.think.tools.routines import app as routines_app
from solstone.think.tools.sol import app as sol_app

call_app.add_typer(journal_app, name="journal")
call_app.add_typer(navigate_app, name="navigate")
call_app.add_typer(routines_app, name="routines")
call_app.add_typer(sol_app, name="identity")
```

These live under `solstone/think/tools/` because they import think-internal APIs directly.

### Adding a new auto-discovered app

This is the happy path for most new commands.

1. **Create `solstone/apps/<name>/call.py`**:

```python
# solstone/apps/myapp/call.py
import typer
from solstone.think.facets import log_call_action

app = typer.Typer(help="Short description of what this app does.")

@app.command("list")
def list_items(
    day: str | None = typer.Option(None, "--day", "-d", help="Day YYYYMMDD."),
    facet: str | None = typer.Option(None, "--facet", "-f", help="Facet name."),
) -> None:
    """List items."""
    from solstone.think.utils import resolve_sol_day, resolve_sol_facet
    day = resolve_sol_day(day)
    facet = resolve_sol_facet(facet)
    # ... implementation
```

2. **That's it for the CLI.** Auto-discovery picks it up on next run.

3. **Create the agent skill** (if agents should use these commands):

```markdown
# solstone/apps/myapp/talent/myapp/SKILL.md
---
name: myapp
description: >
  What this skill does. When to trigger it.
  TRIGGER: keyword1, keyword2, keyword3.
---

# MyApp CLI Skill

Common pattern:
\`\`\`bash
sol call myapp <command> [args...]
\`\`\`

## list

\`\`\`bash
sol call myapp list [-d DAY] [-f FACET]
\`\`\`

List items for a day.

- `-d, --day`: day in `YYYYMMDD` (default: `SOL_DAY` env).
- `-f, --facet`: facet name (default: `SOL_FACET` env).
```

4. **Run `sol skills install --project`** to create the symlink in `journal/.agents/skills/` (`make skills` wraps this).

5. **Update AGENTS.md** — add the skill to the Skills table.

### Adding a manually-mounted built-in

Use this when the command depends heavily on `solstone/think/` internals and shouldn't live in `solstone/apps/`.

1. **Create `solstone/think/tools/<name>.py`** with `app = typer.Typer()`.
2. **Mount in `solstone/think/call.py`**:
```python
from solstone.think.tools.mytools import app as mytools_app
call_app.add_typer(mytools_app, name="mytools")
```
3. **Optionally create a skill** in `solstone/talent/<name>/SKILL.md`.

### Files to maintain for a new call command

| File | What to do | Required? |
|------|-----------|-----------|
| `solstone/apps/<name>/call.py` | Typer app with commands | Yes |
| `solstone/apps/<name>/talent/<name>/SKILL.md` | Skill doc for agents | If agents should use it |
| `journal/.agents/skills/<name>` | Symlink (via `sol skills install --project`; `make skills` wrapper) | Auto-generated |
| `AGENTS.md` Skills table | Add trigger description | If skill exists |
| `tests/test_<name>_call.py` | CLI tests | Yes |

## Conventions

### Environment defaults

Commands that take `--day` or `--facet` should respect `SOL_DAY` and `SOL_FACET` environment variables as defaults. Use the helpers:

```python
from solstone.think.utils import resolve_sol_day, resolve_sol_facet

day = resolve_sol_day(day_arg)    # Falls back to SOL_DAY env
facet = resolve_sol_facet(facet_arg)  # Falls back to SOL_FACET env
```

### Action logging

All mutating `sol call` commands should log their actions for audit:

```python
from solstone.think.facets import log_call_action

log_call_action(
    facet=facet,
    action="myapp_create",
    params={"key": "value"},
    day=day,
)
```

This writes to `facets/{facet}/logs/{day}.jsonl` (or `config/actions/{day}.jsonl` if facet is None).

### The `--consent` flag

Commands that agents invoke proactively (without the user explicitly asking) should accept a `--consent` flag:

```python
consent: bool = typer.Option(
    False,
    "--consent",
    help="Assert that explicit user approval was obtained before calling this command (agent audit trail).",
)
```

This is for audit trail — it records that the agent confirmed user consent before acting.

### Output patterns

- **JSON output**: Use `--json` flag for machine-readable output. Default to human-friendly text.
- **Errors**: Write to stderr via `typer.echo(..., err=True)` and `raise typer.Exit(1)`.
- **Confirmations**: Use `--yes` to skip interactive confirmation for destructive operations.
- **Pagination**: Use `--limit` / `--cursor` for list commands.

### Typer command naming

```python
@app.command("list")      # Verb as command name
@app.command("show")      # Singular operations
@app.command("create")    # CRUD verbs
```

Use lowercase, single-word names. Hyphenated names for multi-word (`list-nudges-due`, `set-name`).

## Doctor Commands

`doctor` is a universal command surface: both `sol doctor` and `journal doctor`
dispatch to `solstone.think.doctor`, with the battery selected by the active
binary.

`sol doctor` checks universal CLI usability and is designed to run cleanly on a
journal-less or repo-less machine. Its default battery has four checks:

- `python_version` — blocker; light package-metadata Requires-Python floor, no
  `pyproject.toml` required.
- `sol_importable` — blocker.
- `local_bin_sol_reachable` — advisory.
- `stale_alias_symlink` — blocker; checks only the `sol` wrapper.

`journal doctor` diagnoses journal-host health. It is role-aware: on a machine
without a local journal directory or installed journal service, folder and
service checks emit `skip` (`no local journal` / `no local journal service`)
instead of false failures. Its battery is:

- `disk_space` — advisory.
- `config_dir_readable`, `journal_dir_writable`, `service_identity`,
  `service_running`, `journal_sync`, `stale_alias_symlink` — blockers.
- `launchd_stale_plist` — advisory on macOS; skipped on Linux.
- `feature:pdf`, `feature:whisper` — advisories with the exact extra-install
  command when missing.

Journal-host blocker failures include invalid service config, service identity
mismatch, crash loops, systemd failed state, and journal-sync conflicts. An
installed service with no supervisor socket is a warning when the OS unit is not
failed. `--feature <name>` runs a single feature advisory on either surface.

Use `sol doctor` for “can this CLI run?”, `journal doctor` for “why is this
journal host unhealthy?”, `make preflight` for the stdlib-only fresh-clone check
before `.venv`/`uv` exist, and `sol health` for the live supervisor status view.

## Structured output: `journal setup --jsonl` and doctor `--jsonl`

Use `--jsonl` when another process needs progress events as they happen. The contract is one JSON object per stdout line, flushed immediately; doctor `--jsonl` is mutually exclusive with doctor `--json`, and the existing doctor `--json` payload keeps its short statuses (`ok`, `warn`, `fail`, `skip`).

| Event | Emitted by | When |
|-------|------------|------|
| `setup.started` | `journal setup --jsonl` | Setup arguments are resolved and the run starts. |
| `setup.completed` | `journal setup --jsonl` | Setup reaches a terminal `ok` or `failed` state. |
| `step.started` | `journal setup --jsonl` | A setup step starts. |
| `step.completed` | `journal setup --jsonl` | A setup step finishes with `outcome: "ok"` or `outcome: "skipped"`. |
| `step.failed` | `journal setup --jsonl` | A setup step fails or reaches a dead end. |
| `step.warning` | `journal setup --jsonl` | Setup translates advisory diagnostics or dropped doctor lines. |
| `doctor.started` | doctor `--jsonl` | Doctor diagnostics begin. |
| `check.completed` | doctor `--jsonl` | One diagnostic check finishes. Status is long form: `ok`, `warning`, `failed`, or `skipped`. |
| `doctor.completed` | doctor `--jsonl` | Doctor diagnostics finish with `status: "ok"`, `"warning"`, or `"failed"`. |

| Code | When |
|------|------|
| `doctor_failed` | Doctor reports a blocking failure or cannot start. |
| `doctor_jsonl_incomplete` | Doctor exits without a `doctor.completed` event. |
| `doctor_timeout` | Doctor exceeds its timeout. |
| `journal_dir_invalid` | The requested journal path is a regular file. |
| `journal_existing_blocked` | Non-interactive setup refuses to auto-claim an existing journal. |
| `service_up_failed` | Service installation succeeded but service startup failed. |
| `setup_unhandled_exception` | A setup step raised an unexpected exception. |
| `step_subprocess_failed` | A setup subprocess exited non-zero. |
| `step_subprocess_timeout` | A setup subprocess exceeded its timeout. |

Step names are fixed and ordered: `doctor`, `journal`, `install_models`, `skills_user`, `skills_journal`, `wrapper`, `service`.

Skipped or resumed reasons are fixed: `--skip-models`, `--skip-skills`, `--skip-service`, `packaged_install`, `prior_run_ok`, `resumed_after_restart`.

### Doctor pass-through

`journal setup --jsonl` runs `sol doctor --readiness --jsonl` for the doctor step and forwards `doctor.started`, `check.completed`, and `doctor.completed` lines verbatim. The readiness battery is the four universal checks plus `disk_space`, `journal_dir_writable`, `feature:pdf`, and `feature:whisper`; it does not run runtime service, sync, config-dir, or launchd checks. Advisory doctor checks are also translated into setup-level `step.warning` events so consumers can handle setup warnings uniformly.

Example stream excerpt for setup readiness:

```jsonl
{"event":"setup.started","ts":"2026-05-11T20:00:00Z","version":"0.0.0+source","mode":"non_interactive"}
{"event":"step.started","ts":"2026-05-11T20:00:00Z","step":"doctor","index":1,"total":7}
{"event":"doctor.started","ts":"2026-05-11T20:00:00Z","version":"0.0.0+source","port":5015,"feature":""}
{"event":"check.completed","ts":"2026-05-11T20:00:01Z","name":"python_version","severity":"blocker","status":"ok","detail":"Python version ok","fix":""}
{"event":"doctor.completed","ts":"2026-05-11T20:00:01Z","status":"ok","duration_ms":120,"summary":{"total":8,"failed":0,"warnings":0,"skipped":0}}
{"event":"step.completed","ts":"2026-05-11T20:00:01Z","step":"doctor","outcome":"ok","duration_ms":121}
{"event":"step.completed","ts":"2026-05-11T20:00:04Z","step":"service","outcome":"ok","duration_ms":900}
{"event":"setup.completed","ts":"2026-05-11T20:00:04Z","status":"ok","duration_ms":4000}
```

### Consumer snippet

```python
import json
import subprocess

proc = subprocess.Popen(
    ["sol", "setup", "--jsonl", "--yes"],
    stdout=subprocess.PIPE,
    text=True,
    bufsize=1,
)
for line in proc.stdout:
    event = json.loads(line)
    print(event["event"], event)
proc.wait()
```

## Directory Structure

```
solstone/
├── think/
│   ├── sol_cli.py                  # Entry point + COMMANDS registry
│   ├── call.py                     # sol call gateway (Typer root + mounts)
│   ├── tools/
│   │   ├── call.py                 # sol call journal (built-in)
│   │   ├── routines.py             # sol call routines (built-in)
│   │   └── sol.py                  # sol call identity (built-in)
│   └── *.py                        # Top-level command modules
├── solstone/apps/
│   ├── todos/
│   │   ├── call.py                 # sol call todos (auto-discovered)
│   │   ├── todo.py                 # Data models
│   │   └── talent/todos/SKILL.md     # Agent skill doc
│   ├── activities/
│   │   ├── call.py                 # sol call activities (auto-discovered)
│   │   └── talent/calendar/SKILL.md
│   ├── entities/call.py
│   ├── speakers/call.py
│   ├── support/call.py
│   ├── transcripts/call.py
│   ├── agent/call.py
│   ├── awareness/call.py
│   └── ... (web-only apps without call.py)
├── talent/
│   ├── journal/SKILL.md            # Skills not tied to an app
│   ├── routines/SKILL.md
│   ├── vit/SKILL.md
│   └── *.md                        # Agent prompt files
├── journal/.agents/skills/          # Symlinks (generated by sol skills install --project; make skills wrapper)
└── AGENTS.md                        # Sol identity + skill table
```

### The `solstone/apps/` dual role

`solstone/apps/` contains both CLI apps (with `call.py`) and convey web routes (without). The presence of `call.py` is the marker for "this app exposes CLI commands." Web-only apps (home, search, stats, etc.) only serve the convey UI.

## Current Command Inventory

### Top-level (`sol <cmd>`)

| Group | Commands |
|-------|----------|
| Think (processing) | `import`, `think`, `planner`, `indexer`, `supervisor`, `schedule`, `top`, `health`, `callosum`, `notify`, `heartbeat` |
| Service | `service` (+ aliases `up`, `down`, `start`) |
| Observe (capture) | `transcribe`, `describe`, `sense`, `transfer`, `observer` |
| Talent (AI agents) | `agents`, `cortex`, `talent`, `call`, `engage`, `providers` |
| Convey (web UI) | `convey`, `restart-convey`, `maint` |
| Specialized | `config`, `skills`, `streams`, `journal-stats`, `formatter`, `detect-created` |
| Installation | `doctor` |
| Help | `help`, `chat` |

### Call (`sol call <app> <cmd>`)

| App | Source | Commands |
|-----|--------|----------|
| `todos` | `solstone/apps/todos/call.py` | list, add, done, cancel, move, upcoming, list-nudges-due, dispatch-nudges |
| `activities` | `solstone/apps/activities/call.py` | list, get, create, update, mute, unmute |
| `entities` | `solstone/apps/entities/call.py` | list, show, search, observe, merge |
| `speakers` | `solstone/apps/speakers/call.py` | list, show, detect-owner, confirm-owner, clusters, suggest |
| `skills` | `solstone/apps/skills/call.py` | list, show, observe, seed, promote, refresh, mark-dormant, retire, edit-request, rename |
| `transcripts` | `solstone/apps/transcripts/call.py` | list, read, segments |
| `support` | `solstone/apps/support/call.py` | register, search, article, create, list, show, reply, attach, feedback, announcements, diagnose |
| `sol` | `solstone/apps/sol/call.py` | name, set-name, reset, thickness, set-owner, sol-init |
| `settings` | `solstone/apps/settings/call.py` | keys (show/set/delete), providers (show/install local), vertex service-account |
| `awareness` | `solstone/apps/awareness/call.py` | status, imports, log, log-read |
| `journal` | `solstone/think/tools/call.py` | search, events, facets, facet (show/create/update/rename/mute/unmute/delete/merge), news, agents, read, imports, import, retention purge, storage-summary |
| `routines` | `solstone/think/tools/routines.py` | list, templates, create, edit, delete, run, output, suggestions, suggest-respond, suggest-state |
| `identity` | `solstone/think/tools/sol.py` | self, partner, agency, pulse, awareness, briefing |
| `navigate` | `solstone/think/tools/navigate.py` | *(single command)* |

`sol skills` manages coding-agent skill installation; `sol call skills` manages owner-wide journal skill patterns.

## Skill System

Skills are documented in `SKILL.md` files and symlinked into both `journal/.claude/skills/` and `journal/.agents/skills/` by `sol skills install --project`; `make skills` wraps this.

**Skill locations:**
- App skills: `solstone/apps/<name>/talent/<name>/SKILL.md`
- Core skills: `solstone/talent/<name>/SKILL.md`

**Skill ≠ call command.** Not every skill has a corresponding `call.py`, and not every `call.py` has a skill:
- `health` and `vit` have skills but no `call.py`
- Some call apps provide the CLI while the skill provides agent behavioral context

Skills document the CLI commands but also add behavioral guidance beyond what `--help` shows (e.g., "check upcoming before adding a future todo to avoid duplicates").

### Keeping skills in sync

When you add or change a `sol call` command, update the corresponding SKILL.md. The skill doc is what agents actually read — they don't parse `--help` output. Include:
- Full command syntax with all flags
- Behavior notes (edge cases, defaults, validation)
- Examples showing common usage patterns
