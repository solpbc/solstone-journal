# solstone Developer Guide

This file is the **developer guide** for the solstone repository. Read it before writing code.

Audience:

- **Coders** (cwd = repo root, editing `solstone/observe/`, `solstone/think/`, `solstone/convey/`, `solstone/apps/`, `solstone/talent/`, `tests/`) — you're in the right place.
- **Cogitate talents** (cwd = `journal/`, running inside the live system) — your entry is `solstone/talent/journal/SKILL.md`, installed into `journal/.claude/skills/journal/` and `journal/.agents/skills/journal/`.
- **Operators** debugging a running system — see `docs/DOCTOR.md`.

For the journal-side runtime entry point, see `journal/AGENTS.md`.

`CLAUDE.md` and `GEMINI.md` at the repo root are symlinks to this file.

## 1. Start here

Read, in order, when you enter the repo for a coding task:

1. **This file through §8** — the invariants must be in working memory before your first edit.
2. **`solstone/think/sol_cli.py`** — the CLI entry point. Skim the `COMMANDS`, `ALIASES`, and `GROUPS` dicts. ~340 lines, scannable in one pass. You now know the whole top-level command surface.
3. **`solstone/think/top.py` (first ~100 lines)** — the interactive TUI. Ties callosum + supervisor + service status together in one vantage point. Good "oh, this is how it connects" moment.
4. **The area you're about to touch:**
   - User-visible feature or `sol call <app> <verb>` → `solstone/apps/<name>/call.py` + `solstone/apps/<name>/routes.py` + `solstone/apps/<name>/templates/`.
   - Think pipeline → `solstone/think/<module>.py` + its tests.
   - AI talent prompt or behavior → `solstone/talent/<name>.md` (+ optional `.py` post-hook).
   - Capture / observe → `solstone/observe/<module>.py`.
5. **Run `sol`** (no args) — prints current journal status + grouped command list. Orients you to live state.
6. **`make dev`** or **`make sandbox`** when you need a running stack to iterate against.

> If you cannot state in one sentence **which module owns the data your change touches**, stop and re-read §7 L2 (the domain ownership table). Writing to a domain from the wrong module is how we got the 14 layer violations the April 2026 audit catalogued.

## 2. Repo map

| Dir | Purpose | Go here when | Depth doc |
|-----|---------|--------------|-----------|
| `solstone/think/sol_cli.py` | CLI entry point — `COMMANDS` / `ALIASES` / `GROUPS` dicts | adding a top-level `sol <cmd>` | `docs/SOLCLI.md` |
| `solstone/observe/` | Multimodal capture — screen, audio, transcribe, describe, sense, transfer | capture-side bugs, new input modalities | `docs/OBSERVE.md` |
| `solstone/think/` | Post-processing core — cortex, talent, callosum, indexer, entities, facets, activities, scheduler, heartbeat, supervisor | anything downstream of capture; most coder work lives here | `docs/THINK.md`, `docs/CORTEX.md`, `docs/CALLOSUM.md` |
| `solstone/convey/` | Web app framework — app discovery, routing, bridge | layout / framework-level UI changes | `docs/CONVEY.md` |
| `solstone/apps/` | Convey apps — each self-contained (`call.py` Typer sub-app + `routes.py` + `templates/`) | adding a user-facing feature, a `sol call <app>` verb, a UI surface | `docs/APPS.md` (required reading before modifying `solstone/apps/`) |
| `solstone/talent/` | AI talent configs (markdown prompts + optional `.py` post-hooks) + `SKILL.md`s (journal, coder, partner, …) | defining or tuning a talent; adding a journal-side skill | `solstone/talent/journal/SKILL.md`, `docs/PROMPT_TEMPLATES.md` |
| `scripts/` | Repo maintenance scripts — `check_layer_hygiene.py` | tooling that guards the codebase; wired into `make ci` | (none) |
| `tests/` | Pytest suites + `tests/fixtures/journal/` mock journal | writing tests; debugging flakiness; `make dev` / `make sandbox` use fixtures as the journal | `docs/testing.md` |
| `docs/` | All longform documentation | reference lookups; never your first stop | §10 below |
| `journal/` | The live journal (user data). Git-ignored content; checked-in template (`AGENTS.md`, skills symlinks) | **rarely as a coder** — modify `solstone/think/`, `solstone/apps/`, or `solstone/talent/`, not journal data | `solstone/talent/journal/SKILL.md` |

Top-level dirs intentionally not in the table: `.venv/`, `scratch/`, `logs/`, `tmp/`, `observers/`, `routines/`, `skills/` — not active coder surfaces.

## 3. Mental model

**The pipeline:** `observe` (capture) → JSON transcripts in `journal/chronicle/YYYYMMDD/` → `think` (analyze) → SQLite index + derived artifacts → `convey` (web UI) and `sol call` CLIs.

**Think is the center.** observe feeds it raw material; convey + apps render its outputs; talent prompts + cortex run AI against it; indexer makes it searchable. A change in `solstone/think/` usually ripples outward.

**Key concepts, priority-ordered:**

- **Journal** — the on-disk record rooted at `journal/` in the repo. Every day is a `journal/chronicle/YYYYMMDD/` directory. Segments (timestamped capture windows) are anchored to creation/modification time, not content "about" time. `get_journal()` from `solstone.think.utils` is the single source of truth for journal path resolution; trust it unconditionally. Source-checkout installs inherit `SOLSTONE_JOURNAL` from the managed bash wrapper at `~/.local/bin/sol`; packaged installs (`uv tool install solstone` or `pipx install solstone`) install `sol` directly at `~/.local/bin/sol` and rely on `get_journal()` to resolve the default journal location; tests use the autouse fixture; sandboxes set it explicitly. Application code must not set it itself (see §8).
- **Talents** — AI processors (markdown prompt + optional Python post-hook). Each has a config in `solstone/talent/<name>.md` with frontmatter that declares hooks, priority, model, and output. Cortex spawns them as subprocesses.
- **Callosum** — Unix-socket JSON message bus at `journal/health/callosum.sock`. Real-time event distribution across services (`tract` + `event` + payload). If components need to talk asynchronously, they talk through callosum.
- **Cortex** — process manager for talent runs. Listens on callosum (`tract="cortex"`, `event="request"`), spawns `python -m solstone.think.talents` subprocesses, writes `<talent>/<ts>_active.jsonl` then renames to `<talent>/<ts>.jsonl` on completion, broadcasts all events back through callosum. Read `docs/CORTEX.md` before modifying talent execution.
- **Facets** — project/context scopes (`work`, `personal`, …). Group related entities, activities, and relationships. Facet data lives under `journal/facets/<facet>/`.
- **Entities** — tracked people / projects / tools. Extracted from transcripts and accumulated across time. Canonical records in `journal/entities/<slug>/entity.json`.
- **Activities** — scheduled or observed "things that happen" (meetings, deadlines, anticipated events). Per-facet JSONL at `journal/facets/<facet>/activities/<day>.jsonl`. Sources: `anticipated` (from `solstone/talent/schedule.md`), `user` (manual), `cogitate` (talent-inferred).
- **Indexer** — reads journal state, builds SQLite + FTS5 index. **Never** mutates source data (§7 L6). Rerunning on unchanged data is a no-op.
- **Supervisor** — top-level process manager. Starts/restarts services, talks to callosum. `journal supervisor` / `journal start`.

## 4. The sol CLI

Two surfaces:

- **`sol <command>`** — access commands registered in `solstone/think/sol_cli.py`'s `COMMANDS` dict (e.g., `sol import`, `sol indexer`, `sol top`, `sol health`).
- **`journal <command>`** — host/service commands from the same registry (e.g., `journal think`, `journal supervisor`, `journal heartbeat`). `ALIASES` provides shorthand compound commands (`journal start` → `journal supervisor`, `journal up/down` → `journal service up/down`). `doctor` is universal: `sol doctor` checks CLI usability; `journal doctor` checks journal-host health.
- **`sol call <app> <verb>`** — routes to `solstone/think/call.py`, which discovers each `solstone/apps/*/call.py` Typer sub-app and mounts it as a subcommand. Example: `sol call entities list`, `sol call activities create`, `sol call journal search`.

**Adding a top-level command:** add an entry to `COMMANDS` in `solstone/think/sol_cli.py`; ensure the module has a `main()` function.

**Adding a `sol call` sub-verb:** add it to the app's `solstone/apps/<app>/call.py` Typer sub-app. No central registration needed — `solstone/think/call.py` discovers apps automatically.
`sol call journal export` is the CLI entry for portable journal ZIPs; read-only archive validation lives in `solstone/think/importers/journal_archive.py`.

Run `sol` (no args) for live status plus the full grouped command list.

## 5. Make commands

Verified against `Makefile`. Grouped by use.

### Install

| Target | When to use |
|--------|-------------|
| `make install` | First setup and whenever `pyproject.toml` or `uv.lock` changes. Creates `.venv/`, syncs deps, runs `make skills`. |
| `make skills` | After adding or renaming a `SKILL.md` under `solstone/talent/` or `solstone/apps/*/talent/`. Rewrites the `.claude/` + `.agents/` skill symlinks into `journal/`. (`make install` depends on this; rarely run alone.) |
| `make update` | Upgrade all deps to latest, regenerate `uv.lock`. Expect test churn. |
| `make update-prices` | Refresh genai-prices model-cost data when adding a new provider model or when pricing tests fail. |
| `make clean` | Remove build artifacts, caches, and the skill symlinks. Does not touch `.venv/`. |
| `make clean-install` | Nuke `.venv/` and `.installed`, then reinstall. Recovery path when the venv is wedged. |

### Run the stack

| Target | When to use |
|--------|-------------|
| `make dev` | Start the full stack (supervisor + callosum + sense + cortex + convey) against `tests/fixtures/journal/`, no observers, no daily processing. Primary inner-loop for UI work. Ctrl-C to stop. |
| `make sandbox` | Ephemeral background sandbox: copies fixtures to a temp journal, starts supervisor in the background, waits for readiness, writes `.sandbox.pid` / `.sandbox.journal`. Pair with verify targets below. Always follow with `make sandbox-stop`. |
| `make sandbox-stop` | Terminate the backgrounded sandbox and clean up state files. |

### Format, lint, test

| Target | When to use |
|--------|-------------|
| `make format` | Auto-fix formatting and imports with ruff. Safe to run anytime; modifies files. |
| `make format-check` | Format dry-run. Part of `make ci`; rarely run alone. |
| `make test` | Unit tests (`tests/`) without coverage. Format-check runs first; failures block tests. Fast inner loop. |
| `make test-cov` | Unit tests with full-repo terminal coverage; used by `make ci` / `make verify`. |
| `make test-apps` | Run all `solstone/apps/*/tests/` suites. |
| `make test-app APP=<name>` | Run a single app's tests. |
| `make test-only TEST=<path-or-pattern>` | Run a specific test file or pytest node id (`TEST="-k test_name"` also works). |
| `make test-all` | Everything — core + apps. Pre-ship gate. |
| `make coverage` | HTML coverage report under `htmlcov/`. Occasional. |
| `make watch` | pytest-watch — reruns tests on file change. Useful during a test-heavy sprint. |
| `make ci` | Format-check + ruff + layer-hygiene + coverage tests. **Run before every commit.** |
| `make verify` | Same steps as `make ci`. Either name is fine. |
| `make install-checks` | The pre-test half of `make ci` (format-check + ruff + layer-hygiene). Called by `ci` / `verify`. |
| `make check-layer-hygiene` | Run `scripts/check_layer_hygiene.py` alone. Useful when iterating on an L1–L2 violation flagged by CI. |

### Verification against a running sandbox

| Target | When to use |
|--------|-------------|
| `make verify-api` | Start a sandbox, run `tests/verify_api.py` against its convey port, stop the sandbox. API-regression check. |
| `make update-api-baselines` | Same, but update the baseline fixtures instead of failing on diff. Run after intentional API changes. |
| `make verify-browser` | Start a sandbox, run `tests/verify_browser.py` (pinchtab-driven browser scenarios), stop the sandbox. UI-regression check. |
| `make update-browser-baselines` | Browser-baselines equivalent of `update-api-baselines`. |
| `make review` | Full product verification: sandbox + API verify + browser verify, in one command. Pre-ship gate for anything user-visible. Requires pinchtab. |
| `make install-pinchtab` | One-time install of the pinchtab browser driver used by `make review` / `make verify-browser`. |

### Service management (systemd / launchd)

`.venv/bin/journal setup` is the source-checkout runtime install path after `make install`; it installs or refreshes the source-checkout wrappers, installs the Claude Code skill when Claude is configured, and starts the background service on port 5015 by default. After the first run, the wrappers at `~/.local/bin/sol` and `~/.local/bin/journal` let you use `sol` and `journal` from anywhere. Use `journal service <install|start|stop|restart|status|logs>` for manual service operations.

| Target | When to use |
|--------|-------------|
| `make service-logs` | Tail the installed service's logs. |

### Other

| Target | When to use |
|--------|-------------|
| `make pre-commit` | Install pre-commit hooks (optional; most coders rely on `make ci` directly). |
| `make versions` | Print versions of Python, uv, and key deps. Diagnostic. |

### Don't use

| Target | Why not |
|--------|---------|
| `make uninstall` | Disabled by design. Use `journal service uninstall`, `sol skills uninstall`, and `python -m solstone.think.install_guard uninstall` for installed user artifacts, or `make clean-install` to rebuild the local dev env. |

## 6. Testing quickstart

- **Framework:** pytest. Files `test_*.py`, functions `test_*`. Shared fixtures in `tests/conftest.py`.
- **Fixture journal:** `tests/fixtures/journal/` — a complete mock journal with facets, entities, segments, index state. The autouse `set_test_journal_path` fixture in `tests/conftest.py` sets `SOLSTONE_JOURNAL` to this path for unit tests. Individual tests may override it with `monkeypatch.setenv` when they need an isolated tmp journal (see §8).
- **Run one test:** `make test-only TEST=tests/test_utils.py::test_foo` or `TEST="-k test_foo"`.
- **Run app tests:** `make test-apps` or `make test-app APP=<name>`.
- **All tests are fast unit/component tests** — no real browser, no live network, no API keys. There is no integration/e2e test tier; tests that would need those were removed in favor of live verification via `make sandbox` / `make verify-browser`.
- **After editing `solstone/convey/` or `solstone/apps/`:** `sol restart-convey` to reload code in a running stack.
- **`make dev` + `make sandbox`** both write runtime artifacts into the fixtures journal; `tests/fixtures/journal/.gitignore` covers those — never commit them.

Full depth: `docs/testing.md`.

## 7. Layer hygiene — required reading (L1–L9)

**Why this lives here.** A codebase-wide audit in April 2026 found 14 layer-hygiene violations in `solstone/think/` and `solstone/apps/`. Infrastructure modules (indexer, importers, schedulers) were silently writing domain state; CLI read-verbs were mutating; get-prefixed functions were creating records on miss. These invariants encode the rules the audit distilled, so the same landmines don't get re-planted. They're inlined here because a one-click-away invariant is a routinely-skipped invariant.

The low-bar grep enforcement is `scripts/check_layer_hygiene.py`, wired into `make ci`. Known audit-flagged files are allowlisted with audit-reference TODOs; the allowlist shrinks as remediation bundles ship.

### L1 — Layer boundaries are load-bearing

Each module family has a declared responsibility. Infrastructure modules (indexer, importer, scheduler, search, graph, stats) may write **only their own output artifacts**. They may not create, modify, or delete domain state (entities, facets, observations, activities, events, chronicle day content). If an infrastructure module needs to trigger a domain mutation, it emits a callosum event or invokes a `sol call <domain> <verb>` subprocess — never writes domain state directly.

### L2 — Domain write ownership

Each domain has exactly **one** write-owning module (or one tightly-scoped family of modules). No other module may call `atomic_write`, `json.dump`, `open("w")`, `Path.write_text`, `unlink`, `rmtree`, etc. on that domain's on-disk state.

| Domain | Write-owning module(s) |
|--------|------------------------|
| Entities (`entities/*/entity.json`, `entities/*/*.npz`) | `solstone/think/entities/journal.py` + `solstone/think/entities/consolidation.py` + `solstone/think/entities/saving.py` + `solstone/think/entities/merge.py` + `solstone/apps/entities/call.py` |
| Facets (`facets/*/facet.json`, `facets/*/relationships/`) | `solstone/think/facets.py` + `solstone/apps/facets/*` (if/when created) |
| Observations (`observations.jsonl`) | `solstone/think/entities/observations.py` |
| Activities (`facets/*/activities/*.jsonl`) | `solstone/think/activities.py` |
| Chronicle day content (`chronicle/YYYYMMDD/**`) | The capturing module (observer, importer) per its declared outputs |
| Index (SQLite, `indexer/*`) | `solstone/think/indexer/*` |

If you're about to write to a domain from a module not in this table, stop and route through the owner.

### L3 — Naming is a contract

Function and CLI-subcommand verbs signal read vs. write intent.

**Read verbs** (functions and CLI subcommands): `load_*`, `get_*`, `read_*`, `scan_*`, `list_*`, `show_*`, `find_*`, `match_*`, `resolve_*`, `query_*`, `lookup_*`, `status_*`, `check_*`, `validate_*`, `discover_*`, `format_*`, `render_*`, `extract_*`, `parse_*`, `view_*`, `inspect_*`, `info_*`, `describe_*`, `search_*`.

A read-verb function must not mutate on-disk state. No exceptions for caches. No exceptions for "create on miss."

If a function needs create-on-miss semantics, split it:

```python
entity = load_entity(eid) or create_entity(eid, ...)
```

This makes the write visible at every call site.

**Write verbs** are the ones allowed to write — choose the right one: `save_`, `create_`, `add_`, `insert_`, `append_`, `attach_`, `delete_`, `remove_`, `update_`, `rename_`, `move_`, `promote_`, `merge_`, `seed_`, `consolidate_`, `bootstrap_`, `backfill_`, `dispatch_`, `record_`, `ingest_`, `import_`, `rebuild_`.

### L4 — CLI read-verbs are read-only

CLI subcommands with read verbs (list, show, status, get, search, find, check, validate, discover, inspect, info, describe, read, view) must not write to journal domain state under any flag combination. If a command needs a write path, split it into two commands — a read-verb reader and a write-verb writer.

### L5 — Write-verb defaults

CLI subcommands with write verbs default to safe.

- Preferred: no default mutation; an explicit `--commit` (or `--apply`) flag is required to perform the write.
- Acceptable alternative: `--dry-run` defaulting to `False` *only if* the subcommand name is unambiguously a write verb AND the command's primary user journey is the write (e.g., `sol call entities create`).

"Bootstrap", "backfill", and "resolve-names" are not unambiguous — default them to dry-run.

### L6 — Indexers never mutate source data

An indexer's job is to build indexes from source-of-truth data. Indexers may not mutate the source data they read. Re-running `sol indexer --rescan` on an unchanged journal must be a no-op for domain state.

### L7 — Importers only write to imports/

Importers write source material to `imports/` and the raw-content areas of `chronicle/`. They may not create or modify entities, facets, observations, or other cross-cutting state. If an importer needs to create an entity for deduplication, it calls a domain-owned `seed_entity()` function in `solstone/think/entities/` that surfaces the write explicitly.

### L8 — Hooks have declared outputs

Post-processing hooks (`solstone/think/hooks.py`, `solstone/talent/*.py` hook functions) declare every path they will write in their frontmatter. The hook runner validates that all actual writes match the declaration. Writes outside the declared set fail loudly — raise at runtime; assert in tests.

### L9 — Event handlers are idempotent

Any function that handles a callosum event, a scheduled tick, or a supervisor-started automation is idempotent w.r.t. on-disk state. Append-only history records dedupe by a natural key (usually `(day, segment)` or `(day, segment, ts)`). Before adding a write to an event handler, ask: "what happens if this event fires twice?"

## 8. Coding invariants

The rules above govern *where* code lives. The rules below govern *how* code behaves. They exist because we got burned.

- **No backwards-compatibility shims.** All code that depends on this project lives in this repository — never add fallback aliases, re-exports for moved symbols, deprecated-parameter handling, or legacy support code. When renaming or removing something, update every usage directly. For journal data-format changes, write a migration script (see `docs/APPS.md` for `maint` commands); do not add a compatibility layer. Cogitate agents default to adding shims; resist this.
- **Trust `get_journal()` unconditionally.** `get_journal()` from `solstone.think.utils` is the single source of truth for journal path resolution. For source-checkout installs, the managed bash wrappers at `~/.local/bin/sol` and `~/.local/bin/journal` set `SOLSTONE_JOURNAL` before invoking the matching venv binary; packaged installs use `uv tool install` / `pipx install` and rely on `get_journal()` for default-journal resolution; tests use the autouse fixture; Makefile sandboxes set it explicitly. Application code, agent prompts, subprocess environments, and service files must not set `SOLSTONE_JOURNAL` themselves. To rewrite the wrapper's embedded path use `journal config journal <path>`. See `docs/environment.md`.
- **SPDX header on every source file.** All Python (and other source) files begin with:

  ```python
  # SPDX-License-Identifier: AGPL-3.0-only
  # Copyright (c) 2026 sol pbc
  ```

  (`//` for JavaScript.) Markdown, text, and prompt files don't need it.
- **Fail loudly, not silently.** Raise specific exceptions with clear messages; use the `logging` module, not `print`. Validate inputs at module boundaries. A silent swallow in production costs days of forensics — an error at the boundary is free.
- **Trust internal code.** Don't add defensive validation for things internal callers can't violate. Validate at system boundaries (user input, external APIs, imported files) — not between modules you control.

Generic software principles (DRY, KISS, YAGNI, single responsibility, small focused commits) apply; see `docs/coding-standards.md` for the full list.

## 9. File headers, naming, dependencies

- **SPDX header** as above — mandatory on source code files.
- **Naming:** modules / functions / variables `snake_case`; classes `PascalCase`; constants `UPPER_SNAKE_CASE`; private members `_leading_underscore`. Full table in `docs/coding-standards.md`.
- **Imports:** prefer absolute (`from solstone.think.utils import get_journal`), grouped stdlib → third-party → local, one per line.
- **Type hints** on function signatures; `mypy` via `make check`.
- **Dependencies:** managed by [uv](https://docs.astral.sh/uv/). `pyproject.toml` is authoritative; `uv.lock` is committed; `make install` syncs; `make update` refreshes.
- **Python 3.12+.**

## 10. Commit hygiene

- Small, focused commits with descriptive messages.
- Run `make ci` before every commit.
- Run `git` commands directly — not `git -C` — you're already in the repo.
- Don't commit runtime artifacts written under `tests/fixtures/journal/` by `make dev` / `make sandbox` (`.gitignore` covers them; verify with `git status` anyway).

## 11. Where to go deeper

Bare links don't motivate clicking. Each entry below says when you actually need the doc.

| Doc | When to read |
|-----|--------------|
| `docs/APPS.md` | **Required before modifying `solstone/apps/`** — pattern catalog for Convey apps, hook-idempotency guidance, Typer sub-app conventions, `maint` commands for data migrations |
| `docs/THINK.md` | Understanding the think-layer pipeline (importers, indexer, segment/stream processing) |
| `docs/CORTEX.md` | Modifying talent execution, cortex lifecycle, talent process management |
| `docs/CALLOSUM.md` | Adding a new tract/event, debugging message flow |
| `docs/CONVEY.md` | Framework-level web changes (as opposed to an individual app) |
| `docs/OBSERVE.md` | Capture-side work: new modalities, transcription, sensing |
| `docs/SOLCLI.md` | Adding a new `sol <cmd>` or `sol call <app> <verb>` |
| `docs/PROMPT_TEMPLATES.md` | Modifying talent prompt format or frontmatter |
| `docs/PROVIDERS.md` | Adding a new AI provider; debugging model selection |
| `docs/testing.md` | Test structure, fixtures, debugging test isolation |
| `docs/environment.md` | Journal path resolution, managed-wrapper behavior, service install details, and `SOLSTONE_JOURNAL` rules |
| `docs/coding-standards.md` | Full naming conventions, ruff / mypy config, dep-management details — reference for everything not promoted into this file |
| `docs/project-structure.md` | Canonical directory layout; resolving "where does this file go" debates |
| `docs/DOCTOR.md` | Diagnostics and debugging a running system |
| `docs/SCREEN_CATEGORIES.md` | Screen-understanding classifier taxonomy (observe side) |
| `docs/VENDOR.md` | Vendor-level integrations |
| `docs/design/` | Per-subsystem design docs |
| `docs/JOURNAL.md` | **Breadcrumb only** — redirects to `solstone/talent/journal/SKILL.md`, the progressive-disclosure journal-layout reference |
| `solstone/talent/journal/SKILL.md` | Journal layout, vocabulary, and `sol call journal` CLI (loaded by cogitate talents on demand via skills) |
| `solstone/talent/journal/references/cli.md` | Full `sol call journal` reference, including **Talent CLI Boundaries** (which infrastructure commands cogitate talents must not call) |

The live journal also carries `journal/AGENTS.md` as its runtime-facing breadcrumb.

`docs/BACKLOG.md` and `docs/ROADMAP.md` are product-planning docs — not coder reading.

## 12. What this file is NOT

- **Not a runtime guide for cogitate talents.** Runtime CLI restrictions on talents live in `solstone/talent/journal/references/cli.md` § Talent CLI Boundaries. If you're tuning what a talent can or cannot call, look there, not here.
- **Not the journal-layout reference.** `solstone/talent/journal/SKILL.md` + its `references/` is the cogitate-audience entry point. This file describes *how those commands are implemented*, not *which ones talents can't call*.
- **Not an operations manual.** For debugging a live system see `docs/DOCTOR.md`; for setup and service lifecycle, see [INSTALL.md](INSTALL.md) (owner install), [CONTRIBUTING.md](CONTRIBUTING.md) (developer install), `journal setup`, and `journal service`.

## 13. Owner-facing copy: the system-anatomy canon

- **Composition by register: owner-facing two parts, sol the keeper.** In owner-facing copy, name the two parts the owner has — `solstone = observers + journal` — and name sol as the keeper who lives in and tends the journal, not a third enumerated part. Never write a three-part owner-facing enumeration in owner-visible copy. The engineering/architecture register is retained and explicit: in architecture statements, technical docs, system/diagram-internal labels, code-side prose, and this repo's architecture sections, the system is `solstone = observers + sol agent + journal` — the sol agent is the running software that tends the journal. The split is by register, not contradiction: owner-facing → two parts, sol the keeper in the journal; engineering/architecture → the sol agent is the running software that tends the journal.
- **Ban surveillance verbs in branded surfaces.** Never use "capture", "watch", "record", "monitor", "track", or "collect" in template copy, settings labels, error messages, onboarding text, or README / INSTALL prose. Prefer "observe alongside", "experience along with", or "take in what you take in".
- **`capture` is code-only.** Keep it in module names such as `solstone/observe/`, function names, OS subsystem identifiers such as `com.solstone.capture`, and internal architecture diagrams. That is intentional and aligned with the canon.
- **Name artifacts for owners, not pipelines.** In branded prose, say "raw media", "the originals", or "observations". Never say "raw captures" or "screen captures" in owner-facing strings. Code-side artifact names stay as-is.
- **`sol` is one thing.** `sol` is the running software; there is no homunculus behind it. Use two registers for one entity: `sol` in conversation, `sol agent` in technical contexts.
- **`keeper` is a surface-specific edge case.** `voice-terminology.md` makes `keeper` the role noun for `sol` in product copy generally. The `solstone-swift` surface bans `keeper` because the mobile UX uses the owner's chosen identity, default `sol`. When writing copy for a specific surface, follow that surface's terminology covenant.
- **Edit with the right mental model.** Internal architecture vocabulary in this repo stays as-is: `solstone/observe/`, the capture pipeline, and screen capture log subsystems remain correct code language. Apply the canon to owner-facing strings only: UI copy, settings text, install / README prose, error messages, and onboarding. If an owner sees it, follow the canon; if it's code or internal docs about pipelines, `capture` is fine.

| Surface | Terminology rule |
|---------|------------------|
| Code surfaces | `capture` is fine in code, module names, function names, subsystem ids, and internal architecture docs. |
| Branded surfaces | `capture` is banned. Use owner-facing phrasing such as "observe alongside", "experience along with", "take in what you take in", "raw media", "the originals", or "observations". |
