# solstone CLI Developer Guide

How the `solstone` CLI is organized, how to add new commands, and what files to maintain.

## Architecture

The CLI has two tiers with distinct purposes:

| Tier | Pattern | Framework | Purpose |
|------|---------|-----------|---------|
| **Top-level** | `solstone <cmd>` / `journal <cmd>` | Distinct native Rust executables | API-only journal access under `solstone`; same-device services and local authorities under `journal` |
| **Call** | `solstone call <app> <cmd>` | Native authority inventory | Tool-callable functions — what agents and humans invoke for data operations |

### The boundary

**If an AI agent should tool-call a journal-access data operation → `solstone call`.** These commands appear in SKILL.md files and are invoked by talent agents during conversations. Local-only host tools live under `journal`.

**If it's system plumbing or local-only host control → `journal <cmd>`.** Processing pipelines, supervisor, services, capture — things that cron or systemd runs.

**Interactive entry points** (`solstone help`, `journal engage`) are top-level for discoverability even though they're user-facing. Agents don't invoke these.

Launchers are split. `solstone` execs `solstone-core-sol` (API
transport, no journal filesystem authority). `journal` execs
`solstone-core-journal` (same-device journal operations). There is no CUDA
package. Installing only `solstone` does not install `journal`.

## Top-Level Commands (`solstone <cmd>`)

### How they work

The public `solstone` commands are top-level launchers that exec the
sibling native `solstone-core-sol` binary. Authority declarations live under
`core/native-sol/think/native/<command>/authority.toml`. Rust handlers live under
`core/crates/solstone-core-sol-client/native/think/<command>/command.rs`.

The public API-root commands are `solstone call`, `solstone import`, and
`solstone status`. `solstone status` queries journal network status through the native
HTTP boundary; it is distinct from `journal status`, which reports local
journal state.

`journal` is a separate launcher for `solstone-core-journal`. Its command
grammar and local operations live in `solstone-core-journal-cli`; its closed
process census maps every retained service name to its historical owner module.
The native dispatch table in `processes.rs` maps retained service names to
sibling binaries:

```rust
NativeProcessSpec {
    token: "spl",
    binary: "solstone-core",
    preset_argv: &["spl", "service"],
}
```

`NATIVE_PROCESS_SPECS` in
`core/crates/solstone-core-journal-cli/src/processes.rs` maps retained
services to sibling native binaries. Owner arguments are forwarded only after
fixed positions. Local writers (`archive`, `facet`, and `news`) are Rust.
Fixed aliases provide `journal up` and `journal down`. There are no retained
Python services.

### Adding a top-level public `solstone` command

1. **Create `core/native-sol/think/native/<command>/authority.toml`** with the command
   path, params, entry type, operation id, and handler name.

2. **Implement `core/crates/solstone-core-sol-client/native/think/<command>/command.rs`**
   and bind the handler declared by the authority.

3. **Regenerate the inventory** with `make build-native-sol-inventory`.

4. **Update parity fixtures and native-sol gates** for the new command.

Use this authority route for top-level commands that read or write journal data
through the native HTTP boundary. For local commands that touch no journal data
and have no `solstone call` oracle path, use a direct match arm in
`solstone_core_sol::run` alongside `root` and `skills`.

For host-only commands, use the native journal command root. Register retained
processes and explicit native cutovers in `processes.rs`; implement direct
journal mutations in Rust under `local_ops.rs` and the relevant owner crate.

### Files to maintain

| File | What to do |
|------|-----------|
| `core/native-sol/think/native/<command>/authority.toml` | Declare the public native command |
| `core/crates/solstone-core-sol-client/native/think/<command>/command.rs` | Implement the native handler |
| `core/crates/solstone-core-journal-cli/src/processes.rs` | Register a retained process or an explicit native cutover |
| `core/crates/solstone-core-journal-cli/src/local_ops.rs` | Compose a same-device Rust authority |

## Call Commands (`solstone call <app> <cmd>`)

### How they work

Native `solstone call` commands are declared by `authority.toml` files under
`core/native-sol/apps/*/native/` and `core/native-sol/think/tools/native/`.
Rust handlers live under
`core/crates/solstone-core-sol-client/native/{apps,tools}/`.
The production aggregate inventory is generated into
`core/crates/solstone-core-sol-client/src/generated/inventory.rs`.

Local-only service tools such as `journal navigate` and `journal identity` are
registered in the native journal process table instead of mounted under `solstone call`.

### Adding a new native app command

This is the happy path for most new commands.

1. **Create or update `core/native-sol/apps/<name>/native/authority.toml`** with the
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
solstone call myapp <command> [args...]
\`\`\`

## list

\`\`\`bash
solstone call myapp list [-d DAY] [-f FACET]
\`\`\`

List items for a day.

- `-d, --day`: day in `YYYYMMDD` (default: `SOL_DAY` env).
- `-f, --facet`: facet name (default: `SOL_FACET` env).
```

4. **Run `make skills`** to regenerate the checked-in router references with `scripts/build_skill_references.py` and refresh the installed `solstone` + `journal` router skill symlinks.

5. **Run `make check-skill-references`** before committing, or rely on `make install-checks`. The check invokes `scripts/build_skill_references.py --check` and fails when generated router references are stale.

### Local-only think tools

Use a top-level `journal <cmd>` entry when the command is meaningful only on
the journal host.

1. **Implement the owner crate** and its binary.
2. **Register a `NativeProcessSpec`** in `core/crates/solstone-core-journal-cli/src/processes.rs`.
3. **Optionally update a router skill reference** if agents need the command.

### Files to maintain for a new call command

| File | What to do | Required? |
|------|-----------|-----------|
| `core/native-sol/apps/<name>/native/authority.toml` | Native command path, params, and route contract | Yes |
| `core/crates/solstone-core-sol-client/native/apps/<name>/command.rs` | Native handler implementation | Yes |
| `core/payload/solstone/apps/<name>/talent/<name>/SKILL.md` | App command guidance fragment used by `scripts/build_skill_references.py` | If agents should use it |
| `core/payload/solstone/talent/solstone/references/commands.md` | Generated `solstone call <app>` inventory | Auto-generated by `make skills` (`scripts/build_skill_references.py`) |
| `core/payload/solstone/talent/journal/references/commands.md` | Generated journal-host command guidance | Auto-generated by `make skills` (`scripts/build_skill_references.py`) |
| `core/fixtures/native-sol/parity/<name>.jsonl` | Native parity vectors | Yes |

## Conventions

### Environment defaults

Commands that take `--day` or `--facet` should respect `SOL_DAY` and `SOL_FACET`
as defaults.

### Action logging

Mutating `solstone call` commands log to `facets/{facet}/logs/{day}.jsonl`
(or `config/actions/{day}.jsonl` when there is no facet).

### The `--consent` flag

Commands agents invoke proactively accept `--consent` as an audit trail that
the owner approved the call.

### Output patterns

- `--json` for machine-readable output. Default to human-friendly text.
- Errors on stderr, non-zero exit.
- `--yes` skips confirmation on destructive operations.
- `--limit` / `--cursor` for lists.

Command names are lowercase single words, or hyphenated multi-word
(`list-nudges-due`, `list-candidates`).

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

`journal setup --jsonl` runs `journal doctor --readiness --jsonl` for the doctor step and forwards `doctor.started`, `check.completed`, and `doctor.completed` lines verbatim. The readiness battery is the client readiness checks (`python_version`, `sol_importable`, `local_bin_sol_reachable`, `stale_alias_symlink`, `disk_space`, `journal_dir_writable`) plus `host_dependencies`, `default_stt_ready`, `feature:pdf-import`, and `feature:whisper`; it does not run runtime service, sync, config-dir, or launchd checks. Advisory doctor checks are also translated into setup-level `step.warning` events so consumers can handle setup warnings uniformly. Execution-error doctor failures remain `doctor_failed` step failures, not warnings.

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
core/native-sol/apps/<name>/native/   # solstone call authority
core/native-sol/think/native/<cmd>/   # top-level solstone command authority
core/native-sol/think/tools/native/   # journal-side tool authority
core/crates/solstone-core-sol-client/native/  # handlers
core/payload/solstone/talent/         # prompts + solstone/journal router skills
core/payload/solstone/apps/<name>/talent/  # app command fragments
```

Journal-side `solstone/apps/<name>/` is per-app *data* in a running journal,
not the codebase. See [APPS.md](APPS.md).

## Current Command Inventory

### Top-level (`solstone <cmd>` / `journal <cmd>`)

| Group | Commands |
|-------|----------|
| Think (processing) | `import`, `think`, `planner`, `indexer`, `supervisor`, `schedule`, `maintenance`, `top`, `health`, `status`, `notify`, `heartbeat` |
| Service | `service` (+ aliases `up`, `down`, `start`), `navigate`, `identity`, `settings`, `install-provider`, `thinking set-lane` |
| Observe (capture) | `transcribe`, `describe`, `sense`, `transfer` |
| Talent (AI agents) | `agents`, `cortex`, `talent`, `call`, `engage`, `providers` |
| Convey (web UI) | `convey`, `restart-convey`, `maint` |
| Schedule (read-only) | `schedule` |
| Specialized | `config`, `skills`, `streams`, `journal-stats`, `reprocess`, `formatter` |
| Installation | `doctor` |
| Help | `help` |

`journal install-provider local` and `journal install-provider parakeet` are native.

`reprocess` is the on-demand reprocess command: process-now by default; `--from-scratch` re-runs already-complete units and, with `--through`, can queue an inclusive past-day range.

`journal maintenance list|sync|run <name>` runs native maintenance (`solstone-core-maintenance`).

### Call (`solstone call <app> <cmd>`)

| App | Source | Commands |
|-----|--------|----------|
| `activities` | `core/native-sol/apps/activities/native/authority.toml` | list, get, create, update, mute, unmute |
| `entities` | `core/native-sol/apps/entities/native/authority.toml` | list, move, detect, attach, update, aka, record-merge-candidate, merge-candidates, accept-merge-candidate, dismiss-merge-candidate, merge, undo-merge, ambiguities, resolve-ambiguity, entity-history, restore-version, network, history, overview, observations, observe, search |
| `speakers` | `core/native-sol/apps/speakers/native/authority.toml` | list, show, detect-owner, confirm-owner, clusters, suggest |
| `transcripts` | `core/native-sol/apps/transcripts/native/authority.toml` | list, read, segments |
| `support` | `core/native-sol/apps/support/native/authority.toml` | register, search, article, create, list, show, reply, attach, feedback, announcements, diagnose |
| `sol` | `core/native-sol/apps/sol/native/authority.toml` | set-owner, sol-init |
| `settings` | `core/native-sol/apps/settings/native/authority.toml` | personal service keys (show/set/delete). Thinking provider selection lives in the Thinking app; local provider install lives at `journal install-provider local`. |
| `awareness` | `core/native-sol/apps/awareness/native/authority.toml` | status, imports, log, log-read |
| `journal` | `core/native-sol/think/tools/native/journal/authority.toml` | agents, facet (create/delete/mute/rename/show/unmute/update), facets, import, imports, news, read, retention (config/list), search, storage-summary |

`solstone skills` manages coding-agent skill installation with `install`, `uninstall`, and `list`. The former `build` verb is gone; `make skills` runs `scripts/build_skill_references.py` directly before invoking `solstone skills install`.

## Skill System

Project skill installation installs exactly two router skills into both `journal/.claude/skills/` and `journal/.agents/skills/`: `solstone` and `journal`. `solstone skills install --project` does not install per-app fragments or every `SKILL.md` as a top-level skill.

**Skill locations:**
- Installed router skills: `core/payload/solstone/talent/solstone/SKILL.md`, `core/payload/solstone/talent/journal/SKILL.md`
- App command fragments: `core/payload/solstone/apps/<name>/talent/<name>/SKILL.md`
- Generated references: `core/payload/solstone/talent/solstone/references/commands.md`, `core/payload/solstone/talent/journal/references/commands.md`

App command fragments are builder source. `make skills` folds their guidance into deterministic, checked-in generated references via `scripts/build_skill_references.py`. The generated references aggregate per-app command guidance from native authority files, including health's `solstone call health` commands and health's journal-host `journal health` / `journal talent` guidance. There is no in-repo `vit` skill.

Fragments document CLI commands and add behavioral guidance beyond what `--help` shows (e.g., "check entity context before attaching a new relationship to avoid duplicates"). Agents consume that guidance through the `solstone` and `journal` router skill references.

### Keeping skills in sync

When you add or change a `solstone call` command, update both the native authority/handler and the corresponding app command fragment. The generated router references are what agents actually read — they don't parse `--help` output. Include:
- Full command syntax with all flags
- Behavior notes (edge cases, defaults, validation)
- Examples showing common usage patterns

Then run `make skills`. `make check-skill-references` invokes `scripts/build_skill_references.py --check`; `make install-checks` runs the same check and fails when generated references are stale.
