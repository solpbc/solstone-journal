# Sol CLI Developer Guide

How the `sol` CLI is organized, how to add new commands, and what files to maintain.

## Architecture

The CLI has two tiers with distinct purposes:

| Tier | Pattern | Framework | Purpose |
|------|---------|-----------|---------|
| **Top-level** | `sol <cmd>` / `journal <cmd>` | Distinct native Rust executables | API-only journal access under `sol`; same-device services and local authorities under `journal` |
| **Call** | `sol call <app> <cmd>` | Native authority inventory | Tool-callable functions — what agents and humans invoke for data operations |

### The boundary

**If an AI agent should tool-call a journal-access data operation → `sol call`.** These commands appear in SKILL.md files and are invoked by talent agents during conversations. Local-only host tools live under `journal`.

**If it's system plumbing or local-only host control → `journal <cmd>`.** Processing pipelines, supervisor, services, capture — things that cron or systemd runs.

**Interactive entry points** (`sol chat`, `sol help`, `journal engage`) are top-level for discoverability even though they're user-facing. Agents don't invoke these.

Console scripts are split across distributions. The base `solstone` package
ships the public POSIX `sol` and `solstone` launchers and depends on
`solstone-core-sol`, whose binary has API transport but no journal filesystem
authority. The `solstone-journal` / `solstone-journal-cuda` distributions ship
the host-only `journal` launcher and depend on `solstone-core-journal`, whose
separate binary owns same-device journal operations. Installing only the base
`solstone` distribution does not install the `journal` launcher or
`solstone-core-journal` binary.

## Top-Level Commands (`sol <cmd>`)

### How they work

The public `sol` / `solstone` commands are top-level launchers that exec the
sibling native `solstone-core-sol` binary. Authority declarations live under
`solstone/think/native/<command>/authority.toml`. Rust handlers live under
`core/crates/solstone-core-sol-client/native/think/<command>/command.rs` —
the shared-client vocab scan is `src/`-only, and `solstone/` must stay
deletable from the cargo build.

The public API-root commands are `sol call`, `sol chat`, `sol import`, and
`sol status`. `sol status` queries journal network status through the native
HTTP boundary; it is distinct from `journal status`, which reports local
journal state.

`journal` is a separate launcher for `solstone-core-journal`. Its command
grammar and local operations live in `solstone-core-journal-cli`; its closed
process census maps every retained service name to its historical owner module.
An explicit native dispatch table replaces Python only after the complete
owner-facing grammar has landed:

```rust
NativeProcessSpec {
    token: "spl",
    binary: "solstone-core",
    preset_argv: &["spl", "service"],
}
```

`NATIVE_PROCESS_SPECS` in
`core/crates/solstone-core-journal-cli/src/processes.rs` maps `grab`, `spl`, and `depict`
to sibling native binaries. The journal uses the same process-replacing runner
for those binaries, while retained Python services use their module name and
fixed bootstrap code. Owner arguments are forwarded only after fixed
positions. `journal_native_dispatch.rs` runs the compiled journal binary with
poisoned sibling interpreters and verifies each native program and argument
list. Local writers (`archive`, `facet`, and `news`) remain entirely in Rust.
Fixed aliases provide `journal up` and `journal down`.

### Adding a top-level public `sol` command

1. **Create `solstone/think/native/<command>/authority.toml`** with the command
   path, params, entry type, operation id, and handler name.

2. **Implement `core/crates/solstone-core-sol-client/native/think/<command>/command.rs`**
   and bind the handler declared by the authority.

3. **Regenerate the inventory** with `make build-native-sol-inventory`.

4. **Update parity fixtures and native-sol gates** for the new command.

Use this authority route for top-level commands that read or write journal data
through the native HTTP boundary. For local commands that touch no journal data
and have no `sol call` oracle path, use a direct match arm in
`solstone_core_sol::run` alongside `root` and `skills`.

For host-only commands, use the native journal command root. Register retained
processes and explicit native cutovers in `processes.rs`; implement direct
journal mutations in Rust under `local_ops.rs` and the relevant owner crate.

### Files to maintain

| File | What to do |
|------|-----------|
| `solstone/think/native/<command>/authority.toml` | Declare the public native command |
| `core/crates/solstone-core-sol-client/native/think/<command>/command.rs` | Implement the native handler |
| `core/crates/solstone-core-journal-cli/src/processes.rs` | Register a retained process or an explicit native cutover |
| `core/crates/solstone-core-journal-cli/src/local_ops.rs` | Compose a same-device Rust authority |

## Call Commands (`sol call <app> <cmd>`)

### How they work

Native `sol call` commands are declared by `authority.toml` files under
`solstone/apps/*/native/` and `solstone/think/tools/native/`. Rust handlers
live under `core/crates/solstone-core-sol-client/native/{apps,tools}/`.
The production aggregate inventory is generated into
`core/crates/solstone-core-sol-client/src/generated/inventory.rs`.

Local-only service tools such as `journal navigate` and `journal identity` are
registered in the native journal process table instead of mounted under `sol call`.

### Adding a new native app command

This is the happy path for most new commands.

1. **Create or update `solstone/apps/<name>/native/authority.toml`** with the
   path, params, operation id, HTTP method, route, and handler.

2. **Implement `core/crates/solstone-core-sol-client/native/apps/<name>/command.rs`**
   and bind the handler declared by the authority.

3. **Regenerate the inventory** with `make build-native-sol-inventory`.

4. **Update the app command fragment** (if agents should use these commands).
   `make skills` discovers these fragments via `scripts/build_skill_references.py` from
   `core/payload/solstone/apps/*/talent/*/SKILL.md`:

```markdown
# core/payload/solstone/apps/myapp/talent/myapp/SKILL.md
---
name: myapp
description: >
  What this command fragment covers. When to trigger it.
  TRIGGER: keyword1, keyword2, keyword3.
---

# MyApp CLI Fragment

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

4. **Run `make skills`** to regenerate the checked-in router references with `scripts/build_skill_references.py` and refresh the installed `sol` + `journal` router skill symlinks.

5. **Run `make check-skill-references`** before committing, or rely on `make install-checks`. The check invokes `scripts/build_skill_references.py --check` and fails when generated router references are stale.

### Local-only think tools

Use a top-level `journal <cmd>` entry when the command is meaningful only on
the journal host and depends heavily on `solstone/think/` internals.

1. **Create the owner module** with a `main()` entry when the retained service still runs in Python.
2. **Register its fixed token and module** in `core/crates/solstone-core-journal-cli/src/processes.rs`.
3. **When the full grammar is native, add an explicit `NativeProcessSpec`** and package its sibling binary with both journal distributions.
4. **Optionally update a router skill reference** if the command needs agent-facing guidance.

### Files to maintain for a new call command

| File | What to do | Required? |
|------|-----------|-----------|
| `solstone/apps/<name>/native/authority.toml` | Native command path, params, and route contract | Yes |
| `core/crates/solstone-core-sol-client/native/apps/<name>/command.rs` | Native handler implementation | Yes |
| `core/payload/solstone/apps/<name>/talent/<name>/SKILL.md` | App command guidance fragment used by `scripts/build_skill_references.py` | If agents should use it |
| `core/payload/solstone/talent/sol/references/commands.md` | Generated `sol call <app>` inventory | Auto-generated by `make skills` (`scripts/build_skill_references.py`) |
| `core/payload/solstone/talent/journal/references/commands.md` | Generated journal-host command guidance | Auto-generated by `make skills` (`scripts/build_skill_references.py`) |
| `core/fixtures/native-sol/parity/<name>.jsonl` | Native parity vectors | Yes |

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

## Journal Doctor

`journal doctor` diagnoses journal-host health. It is role-aware: on a machine
without a local journal directory or installed journal service, folder and
service checks emit `skip` (`no local journal` / `no local journal service`)
instead of false failures. Its battery is:

- `disk_space` — advisory.
- `config_dir_readable`, `journal_dir_writable`, `service_identity`,
  `service_running`, `journal_sync` — blockers.
- `supervisor_conflict` — blocker; macOS only; fails when `journal.app` and the
  legacy LaunchAgent are both supervising one journal, or when a foreign
  persistent LaunchAgent relaunches `/Applications/solstone.app`. Proven-conflict
  actions are `journal service uninstall` for the legacy service and one-line
  `remove foreign launchers targeting /Applications/solstone.app` commands for
  foreign launcher plists; other diagnoses remain visible with their actions
  withheld until the topology is resolved.
- `launchd_stale_plist` — advisory on macOS; skipped on Linux. It advises
  removing the legacy service first and reinstalling the headless service only
  as a separate step.

Journal-host blocker failures include invalid service config, service identity
mismatch, crash loops, systemd failed state, and journal-sync conflicts. An
installed service with no supervisor socket is a warning when the OS unit is not
failed.

Use `journal doctor` for “why is this journal host unhealthy?” and `journal
health` for the live supervisor status view. ⚠ There is no fresh-clone check that
runs before `.venv`/`uv` exist; `make preflight` filled that role and was removed
with the Python reference cut.

## Structured output: `journal setup --jsonl` and doctor `--jsonl`

Use `--jsonl` when another process needs progress events as they happen. The
contract is one JSON object per stdout line, flushed immediately; doctor
`--jsonl` is mutually exclusive with doctor `--json`, and the existing doctor
`--json` payload keeps its short statuses (`ok`, `warn`, `fail`, `skip`). A
per-check `execution_error` field is always present in doctor and preflight JSON:
`null` when the check completed normally, or `{"type": "...", "message": "..."}`
when the check runner raised an ordinary exception.

| Event | Emitted by | When |
|-------|------------|------|
| `setup.started` | `journal setup --jsonl` | Setup arguments are resolved and the run starts. |
| `setup.completed` | `journal setup --jsonl` | Setup reaches a terminal `ok` or `failed` state. |
| `step.started` | `journal setup --jsonl` | A setup step starts. |
| `step.completed` | `journal setup --jsonl` | A setup step finishes with `outcome: "ok"` or `outcome: "skipped"`. |
| `step.failed` | `journal setup --jsonl` | A setup step fails or reaches a dead end. |
| `step.warning` | `journal setup --jsonl` | Setup translates advisory diagnostics, dropped doctor lines, or non-fatal wrapper-provisioning failures. |
| `doctor.started` | doctor `--jsonl` | Doctor diagnostics begin. |
| `check.completed` | doctor `--jsonl` | One diagnostic check finishes. Status is long form: `ok`, `warning`, `failed`, or `skipped`. |
| `doctor.completed` | doctor `--jsonl` | Doctor diagnostics finish with `status: "ok"`, `"warning"`, or `"failed"`. |

An execution error fails the doctor or preflight aggregate independently of the
check's severity. Summary `errors` are a subset of `failed`, so a consumer that
wants completed health failures should compute `failed - errors`.

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

Step names are fixed and ordered: `doctor`, `journal`, `install_models`, `skills_user`, `skills_journal`, `wrapper`, `service`, `brain`.

Skipped, warning, or resumed reasons are fixed: `--skip-models`, `--skip-brain`, `--skip-models implies --skip-brain`, `--skip-skills`, `--skip-service`, `--skip-wrapper`, `a provider is already configured`, `provider config is not in the expected shape`, `local provider unavailable on this host`, `local bootstrap did not start`, `sol on this Mac already keeps this journal`, `prior_run_ok`, `resumed_after_restart`.

The `wrapper` setup step provisions both managed wrappers in-process for source
and packaged installs. It backs up a non-owned alias under `/tmp` before
overwriting it. When `--skip-wrapper` is passed, the step is skipped entirely
and no alias is inspected or replaced. Provisioning failures emit
`step.warning` and setup still exits successfully so the next run can repair
the wrappers.

### Doctor pass-through

`journal setup --jsonl` runs `journal doctor --readiness --jsonl` for the doctor step and forwards `doctor.started`, `check.completed`, and `doctor.completed` lines verbatim. The readiness battery is the client readiness checks (`python_version`, `sol_importable`, `local_bin_sol_reachable`, `stale_alias_symlink`, `disk_space`, `journal_dir_writable`) plus `host_dependencies`, `default_stt_ready`, `feature:pdf-import`, `feature:pdf-export`, and `feature:whisper`; it does not run runtime service, sync, config-dir, or launchd checks. Advisory doctor checks are also translated into setup-level `step.warning` events so consumers can handle setup warnings uniformly. Execution-error doctor failures remain `doctor_failed` step failures, not warnings.

Example stream excerpt for setup readiness:

```jsonl
{"event":"setup.started","ts":"2026-05-11T20:00:00Z","version":"0.0.0+source","mode":"non_interactive"}
{"event":"step.started","ts":"2026-05-11T20:00:00Z","step":"doctor","index":1,"total":8}
{"event":"doctor.started","ts":"2026-05-11T20:00:00Z","version":"0.0.0+source","port":5015,"feature":""}
{"event":"check.completed","ts":"2026-05-11T20:00:01Z","name":"python_version","severity":"blocker","status":"ok","detail":"Python version ok","fix":"","execution_error":null}
{"event":"doctor.completed","ts":"2026-05-11T20:00:01Z","status":"ok","duration_ms":120,"summary":{"total":10,"failed":0,"warnings":0,"skipped":0,"errors":0}}
{"event":"step.completed","ts":"2026-05-11T20:00:01Z","step":"doctor","outcome":"ok","duration_ms":121}
{"event":"step.completed","ts":"2026-05-11T20:00:04Z","step":"service","outcome":"ok","duration_ms":900}
{"event":"setup.completed","ts":"2026-05-11T20:00:04Z","status":"ok","duration_ms":4000}
```

### Consumer snippet

```python
import json
import subprocess

proc = subprocess.Popen(
    ["journal", "setup", "--jsonl", "--yes"],
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
│   ├── tools/
│   │   ├── native/journal/          # native sol call journal authority/handler
│   │   ├── navigate.py             # journal navigate (built-in)
│   │   └── sol.py                  # journal identity (built-in)
│   └── *.py                        # Top-level command modules
├── solstone/apps/
│   ├── activities/
│   │   ├── native/                 # sol call activities native authority/handler
│   │   └── talent/activities/SKILL.md # builder source for generated router refs
│   ├── entities/native/
│   ├── speakers/native/
│   ├── support/native/
│   ├── transcripts/native/
│   ├── awareness/native/
│   └── ...
├── talent/
│   ├── sol/SKILL.md                # installed sol router skill
│   ├── journal/SKILL.md            # installed journal router skill
│   ├── sol/references/commands.md  # generated app command inventory
│   ├── journal/references/commands.md # generated journal-host command guidance
│   └── *.md                        # Agent prompt files
├── journal/.agents/skills/          # sol + journal router symlinks
└── AGENTS.md                        # Developer guide
```

### The `solstone/apps/` dual role

`solstone/apps/` contains convey web routes and, when an app exposes agent-facing
CLI commands, a native `native/authority.toml` plus `native/command.rs`.

## Current Command Inventory

### Top-level (`sol <cmd>` / `journal <cmd>`)

| Group | Commands |
|-------|----------|
| Think (processing) | `import`, `think`, `planner`, `indexer`, `supervisor`, `schedule`, `maintenance`, `top`, `health`, `status`, `notify`, `heartbeat` |
| Service | `service` (+ aliases `up`, `down`, `start`), `navigate`, `identity`, `settings`, `install-provider` |
| Observe (capture) | `transcribe`, `describe`, `sense`, `transfer`, `observer` |
| Talent (AI agents) | `agents`, `cortex`, `talent`, `call`, `engage`, `providers` |
| Convey (web UI) | `convey`, `restart-convey`, `maint` |
| Schedule (read-only) | `schedule` |
| Specialized | `config`, `skills`, `streams`, `journal-stats`, `reprocess`, `formatter`, `detect-created` |
| Installation | `doctor` |
| Help | `help`, `chat` |

`solstone-core install-provider local` and `solstone-core install-provider parakeet` are native beside the existing Python-backed `journal install-provider` route; all remain live during cutover.

`reprocess` is the on-demand reprocess command: process-now by default; `--from-scratch` re-runs already-complete units and, with `--through`, can queue an inclusive past-day range.

`journal maintenance list|sync|run <app:name> [-- args]` manages app-owned recurring routines discovered from `apps/*/maintenance.py`.

### Call (`sol call <app> <cmd>`)

| App | Source | Commands |
|-----|--------|----------|
| `activities` | `solstone/apps/activities/native/authority.toml` | list, get, create, update, mute, unmute |
| `entities` | `solstone/apps/entities/native/authority.toml` | list, move, detect, attach, update, aka, record-merge-candidate, merge-candidates, accept-merge-candidate, dismiss-merge-candidate, merge, undo-merge, ambiguities, resolve-ambiguity, entity-history, restore-version, network, history, overview, observations, observe, search |
| `speakers` | `solstone/apps/speakers/native/authority.toml` | list, show, detect-owner, confirm-owner, clusters, suggest |
| `transcripts` | `solstone/apps/transcripts/native/authority.toml` | list, read, segments |
| `support` | `solstone/apps/support/native/authority.toml` | register, search, article, create, list, show, reply, attach, feedback, announcements, diagnose |
| `sol` | `solstone/apps/sol/native/authority.toml` | set-name, reset, set-owner, sol-init |
| `settings` | `solstone/apps/settings/native/authority.toml` | personal service keys (show/set/delete). Thinking provider selection lives in the Thinking app; local provider install lives at `journal install-provider local`. |
| `awareness` | `solstone/apps/awareness/native/authority.toml` | status, imports, log, log-read |
| `journal` | `solstone/think/tools/native/journal/authority.toml` | agents, facet (create/delete/mute/rename/show/unmute/update), facets, import, imports, news, read, retention (config/list), search, storage-summary |

`sol skills` manages coding-agent skill installation with `install`, `uninstall`, and `list`. The former `build` verb is gone; `make skills` runs `scripts/build_skill_references.py` directly before invoking `sol skills install`.

## Skill System

Project skill installation installs exactly two router skills into both `journal/.claude/skills/` and `journal/.agents/skills/`: `sol` and `journal`. `sol skills install --project` does not install per-app fragments or every `SKILL.md` as a top-level skill.

**Skill locations:**
- Installed router skills: `core/payload/solstone/talent/sol/SKILL.md`, `core/payload/solstone/talent/journal/SKILL.md`
- App command fragments: `core/payload/solstone/apps/<name>/talent/<name>/SKILL.md`
- Generated references: `core/payload/solstone/talent/sol/references/commands.md`, `core/payload/solstone/talent/journal/references/commands.md`

App command fragments are builder source. `make skills` folds their guidance into deterministic, checked-in generated references via `scripts/build_skill_references.py`. The generated references aggregate per-app command guidance from native authority files, including health's `sol call health` commands and health's journal-host `journal health` / `journal talent` guidance. There is no in-repo `vit` skill.

Fragments document CLI commands and add behavioral guidance beyond what `--help` shows (e.g., "check entity context before attaching a new relationship to avoid duplicates"). Agents consume that guidance through the `sol` and `journal` router skill references.

### Keeping skills in sync

When you add or change a `sol call` command, update both the native authority/handler and the corresponding app command fragment. The generated router references are what agents actually read — they don't parse `--help` output. Include:
- Full command syntax with all flags
- Behavior notes (edge cases, defaults, validation)
- Examples showing common usage patterns

Then run `make skills`. `make check-skill-references` invokes `scripts/build_skill_references.py --check`; `make install-checks` runs the same check and fails when generated references are stale.
