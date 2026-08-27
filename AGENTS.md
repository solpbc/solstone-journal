# solstone Developer Guide

This file is the **developer guide** for the solstone repository. Read it before writing code.

> ⚠️ **§7 has not caught up with the Rust conversion, and §7 is the section this guide calls required
> reading.** The Python under `solstone/` was removed from `main`; the directory now holds only the
> `detect_created` spec. The Swift helper lives next to `solstone-core-transcribe`.
> `authority.toml` lives under `core/native-sol/`. `scripts/`
> still holds working tooling. But **§7's L2 table names a write-owning module per domain, and every one of
> the 64 Python modules it names is gone** — not most of them, all of them. Its crate rows do still
> resolve and are the current owners, so read that table for the domains and their rules, and trust its
> `core/crates/` entries over its `.py` ones.
>
> The directories in §2 all still exist; what has moved is what lives in them. Treat any `solstone/**.py`
> path anywhere below as where a responsibility used to live — **and assume nothing annotates it for
> you**, including in §1's reading list. Its current home is a crate under `core/crates/`, and the
> reliable way to find it is to search `core/crates/` for the behavior rather than to follow a path from
> this document. `docs/PORTING.md` is the remaining Rust workspace rules.
>
> Most of a hundred-odd repository paths cited below no longer resolve, and nearly all of those are in
> §7. ⛔ **No exact count is given here on purpose**: this tree moved twice under a reviewer measuring
> it, so a frozen number would be wrong within hours and would read as authority it does not have.
> Count it yourself if you need it.

Audience:

- **Coders** (cwd = repo root, editing `core/crates/`, `core/native-sol/`, `core/payload/solstone/talent/`) — you're in the right place.
- **Cogitate talents** (cwd = `journal/`, running inside the live system) — your journal-side entry is `core/payload/solstone/talent/journal/SKILL.md`, installed into `journal/.claude/skills/journal/` and `journal/.agents/skills/journal/` alongside the `solstone` router skill. The runtime contract you operate under — tools, reads vs writes, finalization, access tiers, and what is *not* in your context — is `docs/COGITATE.md`.
- **Operators** debugging a running system — see `docs/DOCTOR.md`.

For the journal-side runtime entry point, see `journal/AGENTS.md`.

`CLAUDE.md` and `GEMINI.md` at the repo root are symlinks to this file.

## 1. Start here

Read, in order, when you enter the repo for a coding task:

1. **This file through §8** — the invariants must be in working memory before your first edit.
2. **`docs/SOLCLI.md`** — the CLI routing map. `solstone` and `journal` are separate native Rust executables with different authority.
3. ⛔ **`solstone/think/top.py` is gone** — it was the interactive TUI, and reading it was the "oh, this is how it connects" moment because it tied callosum, supervisor, and service status together in one vantage point. Nothing has replaced it as a single reading target; the equivalent orientation is now spread across the callosum, supervisor, and system crates under `core/crates/`.
4. **The area you're about to touch:**
   - User-visible feature or `solstone call <app> <verb>` → `core/native-sol/apps/<name>/` + the matching `*-web` crate or `convey-shell/assets/<name>/`. See `docs/APPS.md`.
   - Think pipeline → the matching crate under `core/crates/solstone-core-*`.
   - AI talent prompt or behavior → `core/payload/solstone/talent/<name>.md`.
   - Capture / observe → `solstone-core-ingest`, `solstone-core-transcribe`, `solstone-core-describe`.
5. **Run `solstone`** (no args) — prints the static grouped command list. Orients you to the public CLI surface.
6. **`make dev`** or **`make sandbox`** when you need a running stack to iterate against.

> If you cannot state in one sentence **which module owns the data your change touches**, stop and re-read §7 L2 (the domain ownership table). Writing to a domain from the wrong module is how we got the 14 layer violations the April 2026 audit catalogued.

## 2. Repo map

| Dir | Purpose | Go here when | Depth doc |
|-----|---------|--------------|-----------|
| `core/crates/solstone-core-journal-cli/` | Owns native journal parsing, local authorities, and the closed service-process table | adding a `journal <cmd>` or same-device authority | `docs/SOLCLI.md` |
| `core/crates/solstone-core-{ingest,transcribe,describe}/` | Multimodal capture — ingest, transcribe, describe, sense | capture-side bugs, new input modalities | `docs/OBSERVE.md` |
| `core/crates/solstone-core-*/` | Post-processing core — cortex, talent, callosum, indexer, entities, facets, activities, scheduler, heartbeat, supervisor | anything downstream of capture; most coder work lives here | `docs/THINK.md`, `docs/CORTEX.md`, `docs/COGITATE.md`, `docs/CALLOSUM.md` |
| `core/crates/solstone-core-convey-shell/` | Web app framework — shell, session gate, app registry | layout / framework-level UI changes | `docs/CONVEY.md` |
| `core/crates/solstone-core-*-web/` + `convey-shell/assets/` | Convey apps — registered in `APP_REGISTRY`, served by a `*-web` crate or shell assets | adding a user-facing feature, a `solstone call <app>` verb, a UI surface | `docs/APPS.md` |
| `core/payload/solstone/talent/` | AI talent configs (markdown prompts) + installed router skills (`solstone`, `journal`); app fragments feed generated router references. **The `.py` post-hooks are not here** — they are not shipped data and stay at `solstone/talent/` | defining or tuning a talent; updating router guidance | `core/payload/solstone/talent/journal/SKILL.md`, `docs/PROMPT_TEMPLATES.md` |
| `core/` | Rust workspace — thin `solstone-core` bin plus library-first adapter crates | Rust scaffold, gates, workspace rules | `docs/PORTING.md` |
| `scripts/` | Repo maintenance scripts. ⚠ Reduced with the Python reference cut — anything whose oracle was the Python implementation is gone, so treat a script here as build tooling, not as a source of truth about behaviour | tooling that guards the codebase; reached by `make install-checks`, never by `make ci` | channel adapters: `docs/CHANNEL_ADAPTERS.md` |
| `tools/journal_device_sim/` | Dependency-free linked-device fixture simulator; native `solstone link` remains the PL/SPL and identity boundary | composed ingest, reconciliation, recovery, and field-journal validation through a disposable receiver | `tools/journal_device_sim/README.md` |
| `tests/` | `tests/fixtures/journal/` mock journal. ⚠ **No Python suites remain** — the pytest tree went with the reference cut, and Rust tests live beside their crates under `core/crates/*/tests/` | `make dev` / `make sandbox` use the fixtures as the journal | `docs/testing.md` |
| `tests/js/` | JavaScript harnesses driven by Python node tests | testing browser scripts without a real browser | `docs/testing.md` |
| `docs/` | All longform documentation | reference lookups; never your first stop | §10 below |
| `journal/` | The live journal (user data). Git-ignored content; checked-in template (`AGENTS.md`, skills symlinks) | **rarely as a coder** — modify `core/crates/` or `core/payload/solstone/talent/`, not journal data | `core/payload/solstone/talent/journal/SKILL.md` |

Top-level dirs intentionally not in the table: `.venv/`, `scratch/`, `logs/`, `tmp/`, `observers/`, `routines/`, `skills/` — not active coder surfaces.

## 3. Mental model

**The pipeline:** `observe` (capture) → JSON transcripts in `journal/chronicle/YYYYMMDD/` → `think` (analyze) → SQLite index + derived artifacts → `convey` (web UI) and `solstone call` CLIs.

**Think is the center.** observe feeds it raw material; convey + apps render its outputs; talent prompts + cortex run AI against it; indexer makes it searchable. A change in `solstone/think/` usually ripples outward.

**Key concepts, priority-ordered:**

- **Journal** — the on-disk record rooted at `journal/` in the repo. Every day is a `journal/chronicle/YYYYMMDD/` directory. Segments (timestamped capture windows) are anchored to creation/modification time, not content "about" time. `solstone-core-journal::resolve_journal_path` is the resolver. Source-checkout installs inherit `SOLSTONE_JOURNAL` from the managed wrapper at `~/.local/bin/solstone`; a tree install puts `solstone` and `journal` on PATH (see `INSTALL.md`). Tests and sandboxes set the env explicitly. Application code must not set it itself. See `docs/environment.md`.
- **Talents** — AI processors (markdown prompt + optional Python post-hook). Each has a config in `core/payload/solstone/talent/<name>.md` with frontmatter that declares hooks, priority, model, and output. Cortex spawns them as subprocesses.
- **Callosum** — Unix-socket JSON message bus at `journal/health/callosum.sock` on Unix, with an authenticated Windows named-pipe transport derived from the same endpoint. Its Windows boundary protects cross-user/cross-identity and remote-network access, not malware already running as the same user/SID. Real-time event distribution across services (`tract` + `event` + payload). If components need to talk asynchronously, they talk through callosum.
- **Cortex** — process manager for talent runs. Listens on callosum (`tract="cortex"`, `event="request"`), resolves the sibling `solstone-core` binary and spawns it with `__talent-worker`, writes `<talent>/<ts>_active.jsonl` then renames to `<talent>/<ts>.jsonl` on completion, broadcasts all events back through callosum. Read `docs/CORTEX.md` before modifying talent execution.
- **Facets** — project/context scopes (`work`, `personal`, …). Group related entities, activities, and relationships. Facet data lives under `journal/facets/<facet>/`.
- **Entities** — tracked people / projects / tools. Extracted from transcripts and accumulated across time. Canonical records in `journal/entities/<slug>/entity.json`.
- **Activities** — scheduled or observed "things that happen" (meetings, deadlines, anticipated events). Per-facet JSONL at `journal/facets/<facet>/activities/<day>.jsonl`. Sources: `anticipated` (from `core/payload/solstone/talent/schedule.md`), `user` (manual), `cogitate` (talent-inferred).
- **Indexer** — reads journal state, builds SQLite + FTS5 index. **Never** mutates source data (§7 L6). Rerunning on unchanged data is a no-op.
- **Supervisor** — top-level process manager. Starts/restarts services, talks to callosum. `journal supervisor` / `journal start`.

## 4. The solstone CLI

Two surfaces:

- **`solstone <command>`** — native access commands declared under `core/native-sol/think/native/<command>/authority.toml` and implemented by `core/crates/solstone-core-sol-client/native/think/<command>/command.rs` (e.g., `solstone import`).
- **`journal <command>`** — same-device commands owned by `solstone-core-journal`. Local writers execute in Rust; service commands use the native closed process table (e.g., `journal think`, `journal supervisor`, `journal heartbeat`). `journal up/down` are fixed aliases for `journal service up/down`.
- **`solstone call <app> <verb>`** — native app commands declared under `solstone/apps/<app>/native/` or `solstone/think/tools/native/` and implemented under `core/crates/solstone-core-sol-client/native/{apps,tools}/`. `solstone call journal` exposes its native 17-leaf journal group through this boundary.

**Adding a top-level `solstone` command:** add a native authority under `core/native-sol/think/native/<command>/authority.toml` and implement the handler at `core/crates/solstone-core-sol-client/native/think/<command>/command.rs`. Use `core/native-sol/think/native/import/authority.toml` with `core/crates/solstone-core-sol-client/native/think/import/command.rs` as the current pattern.

**Adding a `solstone call` sub-verb:** update `core/native-sol/apps/<app>/native/authority.toml`, implement the handler in `core/crates/solstone-core-sol-client/native/apps/<app>/command.rs`, and regenerate the native inventory.
Portable journal archives can be downloaded from the Journal archive flow on the Import screen and previewed there before merge.

Run `solstone` (no args) for the static grouped command list.

## 5. Make commands

Verified against `Makefile`. Grouped by use.

### Install

| Target | When to use |
|--------|-------------|
| `make install` | First setup and whenever `pyproject.toml` or `uv.lock` changes. Creates `.venv/`, syncs deps, runs `make skills`. |
| `make skills` | Regenerate generated router references, then rewrite the `solstone` + `journal` router skill symlinks into `journal/`. (`make install` depends on this; rarely run alone.) |
| `make update` | Upgrade all deps to latest, regenerate `uv.lock`. Expect test churn. |
| `make update-prices` | Refresh genai-prices model-cost data when adding a new provider model or when pricing tests fail. |
| `make clean` | Remove build artifacts, caches, and the skill symlink dirs (`journal/.agents/`, `journal/.claude/`). Does not touch `.venv/`. Before `cargo clean`, refuses if a live process has an open file, mapping, cwd, or executable under this checkout's `RUST_TARGET_DIR` (`core/target`, or `CARGO_TARGET_DIR` if set) and prints blocker pids+paths. Override with `CLEAN_FORCE=1`. |
| `make clean-install` | Runs `clean` first (same live-use refuse / `CLEAN_FORCE=1`), then deletes `.venv/` and `.installed`, then exits 1 as retired. Recreate a Python tooling venv with `uv sync --group dev` only if a remaining script still needs it. |

### Run the stack

| Target | When to use |
|--------|-------------|
| `make dev` | Start the full stack (supervisor + callosum + sense + cortex + convey) against `tests/fixtures/journal/`, no observers, no daily processing. Primary inner-loop for UI work. Ctrl-C to stop. |
| `make sandbox` | Ephemeral background sandbox: copies fixtures to a temp journal, starts supervisor in the background, waits for readiness, writes `.sandbox.pid` / `.sandbox.journal`. Pair with verify targets below. Always follow with `make sandbox-stop`. |
| `make sandbox-stop` | Terminate the backgrounded sandbox and clean up state files. |

### Format, lint, test

| Target | When to use |
|--------|-------------|
| `make` / `make all` / `make build` | Build the native Rust workspace, excluding the three host-native helper packages during the conversion freeze. |
| `make build-sandbox-processing` | Opt in to build the two native processing helpers and their shared runtime bundle into the effective Cargo target directory. |
| `make check-rust-sandbox-processing-build` | Verify an existing processing bundle and both helpers’ loader-independent startup; it never builds or repairs. |
| `make format` | Format the Rust workspace with Cargo fmt; modifies Rust source. |
| `make format-check` | Cargo fmt dry-run (`cargo fmt --all -- --check`); one of the Rust-only CI checks. |
| `make test` | Alias for `make check-rust-test`: Rust workspace tests only, excluding the three host-native helper packages covered by the default `onnx-host-tests` full-gate leg. |
| `make check-journal-device-sim` | Standard-library tests for the repository-local Python device simulator; no journal, external network, credentials, or product runtime. |
| `make test-cov` / `test-integration` / `test-performance` / `test-app` / `test-only` / `coverage` / `watch` | Gone with the Python suite. Use `make ci` or `make ci-full`. |
| `make ci` | Efficient Rust-only routine gate: formatting, topology validation, library/binary Clippy, and serialized library/binary unit tests. It does not run Cargo integration-test targets or heavyweight native/platform/policy legs. |
| `make ci-full` | Registry-driven full operator gate. It runs selected entries independently, continues after failures, applies per-entry timeouts, and writes a revision-bound receipt. Run it on the exact final-tree SHA after `make ci-full-prep`. |
| `WIN_REMOTE_HOST=user@host make win-host-ci` | Transfer an exact, source-bound snapshot to the configured Windows build host and run the first native MSVC journal substrate gate. A pass covers builds of `solstone-core-journal` and `solstone-core-journal-config` plus the config crate's unit tests; the success line lists the Windows behavior this gate has not run. |
| `make verify` | Alias for `make ci` during the Rust-conversion freeze. |
| `make install-checks` | Directly runnable full Python-and-Rust preflight chain (format, ruff, layer hygiene, and related checks); no longer called by `ci` or `verify`. |

During the Rust-conversion freeze, use the narrowest applicable
`make check-rust-*` target, then efficient `make ci` for routine validation.
An operator runs `make ci-full` on the exact final-tree SHA for full host
evidence. These paths are enforced by the
[`ci_gate_purity` contract tests](core/crates/solstone-core-repository-contracts/src/contracts/ci_gate_purity.rs).
The native Windows rail is operator-run and deliberately separate from
`ci-full`. `make win-host-ci` refuses untracked, non-ignored files, binds the
transferred Git snapshot to the workspace lockfile digest, and reports success
only after the remote checkout acknowledges both values. Treat the runner's explicit
not-run list as the evidence boundary; a transport pass is not filesystem,
Callosum, packaging, install, signing, or smoke evidence.
The focused Python Make targets are frozen; run the Python suite directly when
it is needed.
Do not rerun an unchanged failure merely to seek green.

⚠ On Fedora, bare `cargo build`, `cargo test`, or `cargo clippy` can die in
`ffmpeg-sys-next`'s build script with `fatal error: 'limits.h' file not found`,
surfacing as an exit-101 crate build/test failure that reads like broken code
rather than an environment gap. The clang toolchain is correctly installed,
but the `Makefile` exports `BINDGEN_EXTRA_CLANG_ARGS` per target from
`CLANG_BUILTIN_INCLUDE`, so it never reaches a bare Cargo invocation. This is
not a missing or misconfigured system package, so no package installation or
elevated permissions are needed. Before invoking Cargo directly, export this
variable yourself to supply the missing clang builtin include directory:
`export BINDGEN_EXTRA_CLANG_ARGS=-I$(find /usr/lib{,64}/clang/*/include -print -quit 2>/dev/null)`.
`make`, `make ci`, `make test`, and `make check-rust-*` set it automatically.

### Rust test topology

Treat every Cargo test target as a build-cost decision. [By default, Cargo
builds each top-level integration-test file as a separate
executable](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests),
so prefer the narrowest owning crate and a grouped harness; explicit `[[test]]`
targets have the same cost. Preserve black-box tests when the process boundary
is the behavior; the goal is fewer binaries, not relabeling integration tests.

- Put same-crate behavior tests in `#[cfg(test)]` modules beside the owning
  code. Put public API and process contracts in the narrowest owning leaf crate
  unless the behavior genuinely belongs to the aggregate `solstone-core`
  composition boundary.
- Add cases to an existing grouped domain harness; if none fits, create one.
  Any additional top-level target must state its Cargo-level reason, such as
  irreconcilable process-global setup, `required-features`, or custom harness
  configuration. Check [`autotests`](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#target-auto-discovery)
  before adding a file and confirm new targets in `cargo metadata`; packages
  with discovery disabled need explicit wiring.
  Use OS-assigned ephemeral ports; environment mutation, shared state, and
  subprocess lifecycle do not alone justify another target.
- Put repository, source, manifest, Makefile, packaging, and workspace-policy
  assertions in a dedicated lightweight repository-contract crate or target so
  they do not link the product graph. Establish that home from a concrete
  inventory; do not broaden a product crate or add an empty architectural
  placeholder.
- Iterate with the narrowest command, such as `cargo test --manifest-path
  core/Cargo.toml -p <package> --lib` or the affected `--test <harness> <test>`.
  [`make ci`](Makefile) is the routine gate: a formatting check, topology
  validation, library/binary Clippy, and library/binary unit harnesses. It
  does not link or execute integration-test binaries, so run affected harnesses
  directly.
- Run [`make ci-full`](Makefile) once on the exact final tree before merge or
  release. This host-conditional gate reports unsupported platform legs as
  skipped, so run affected platform lanes on their supported hosts. Separately
  run `make check-rust-race` for concurrency-sensitive supervisor changes.

### Verification against a running sandbox

| Target | When to use |
|--------|-------------|

### Service management (systemd / launchd)

`journal setup` is the runtime install path once you have a `journal` binary, from a tree install or from `cargo build` in this checkout. `make install` is retired. It installs or refreshes the managed wrappers, installs the Claude Code skill when Claude is configured, and starts the background service on port 5015 by default. After the first run, the wrappers at `~/.local/bin/solstone` and `~/.local/bin/journal` let you use `solstone` and `journal` from anywhere. Use `journal service <install|start|stop|restart|status|logs>` for manual service operations.

| Target | When to use |
|--------|-------------|
| `make service-logs` | Tail the installed service's logs. |

### Other

| Target | When to use |
|--------|-------------|
| `make pre-commit` | Install pre-commit hooks (optional). |
| `make versions` | Print versions of Python, uv, and key deps. Diagnostic. |

### Release and transparency

The Python wheel/release targets and `scripts/release.sh` are gone. See
[`docs/PORTING.md`](docs/PORTING.md) and
[`docs/release-evidence-contract.md`](docs/release-evidence-contract.md).

### Don't use

| Target | Why not |
|--------|---------|
| `make uninstall` | Disabled by design. Use `journal service uninstall`, `solstone skills uninstall`, and `python -m solstone.think.install_guard uninstall` for installed user artifacts, or `make clean-install` to rebuild the local dev env. |

## 6. Testing quickstart

- **Rust gates:** `make` / `make all`, `make ci`, `make ci-full`, `make test`, `make verify`, and `make build` operate only on the native `core/` Cargo workspace during the Rust-conversion freeze. Per the [Makefile](Makefile), `make ci` is the efficient routine path with formatting, topology validation, library/binary Clippy, and library/binary unit tests. `make ci-full` is the selectable, registry-driven final-tree gate; prepare it with `make ci-full-prep`.
- **Python suite:** ⚠ **there is none.** The pytest tree, `tests/conftest.py`, and the marked integration/performance/release suites were all removed with the Python reference cut. `tests/` now holds only the fixture journal at `tests/fixtures/journal/`. Live product verification still uses `make sandbox`.
- **API baselines:** ⚠ **`make verify-api`, `make update-api-baselines` and `make verify-schemathesis` are gone**, because each drove a deleted Python file. Nothing checks SPA/API response baselines or fuzzes the OpenAPI contract today.
- **After editing `solstone/convey/` or `solstone/apps/`:** `journal restart-convey` to reload code in a running stack.
- **Runtime artifacts:** `make dev` writes them into the fixtures journal, where `tests/fixtures/journal/.gitignore` covers them. `make sandbox` uses an ephemeral copy and leaves only its `.sandbox.pid` and `.sandbox.journal` state files until `make sandbox-stop` removes them.
- **Test invariants, not snapshots.** A test asserts what must hold in *every* valid state of the system — not what happens to be true today. Never pin a test to hand-edited prose (CHANGELOG / README / docs), to a value the system is *designed* to change (a version, a date, a growing count), or to a transient state. The tell: if doing the correct next thing — cut a release, rename a label, graduate a shipped changelog entry — turns the test red, the test is wrong, not the system. And test the code that *produces* a fact, never the rendered text about it. (A `[Unreleased]`-pinned changelog test was exactly this anti-pattern — its pass condition required the release process to *not* run; removed 2026-05-30.)

Full depth: `docs/testing.md`.

## 7. Layer hygiene — required reading (L1–L9)

**Why this lives here.** A codebase-wide audit in April 2026 found 14 layer-hygiene violations in `solstone/think/` and `solstone/apps/`. Infrastructure modules (indexer, importers, schedulers) were silently writing domain state; CLI read-verbs were mutating; get-prefixed functions were creating records on miss. These invariants encode the rules the audit distilled, so the same landmines don't get re-planted. They're inlined here because a one-click-away invariant is a routinely-skipped invariant.

⚠ **These invariants currently have NO automated enforcement.** The low-bar grep checker was `scripts/check_layer_hygiene.py`, which read the Python tree the reference cut deleted; it was removed rather than left as a check that could only pass vacuously. The rules below still bind — they are now held by review, and by the Rust type and module boundaries, rather than by a gate.

### L1 — Layer boundaries are load-bearing

Each module family has a declared responsibility. Infrastructure modules (indexer, importer, scheduler, search, graph, stats) may write **only their own output artifacts**. They may not create, modify, or delete domain state (entities, facets, observations, activities, events, chronicle day content). If an infrastructure module needs to trigger a domain mutation, it emits a callosum event or invokes a `solstone call <domain> <verb>` subprocess — never writes domain state directly.

### L2 — Domain write ownership

Each domain has exactly **one** write-owning module (or one tightly-scoped family of modules). No other module may call `atomic_write`, `json.dump`, `open("w")`, `Path.write_text`, `unlink`, `rmtree`, etc. on that domain's on-disk state.

| Domain | Write-owning module(s) |
|--------|------------------------|
| Entities (`entities/*/entity.json`) | `solstone/think/entities/journal.py` + `solstone/think/entities/relationships.py` + `solstone/think/entities/saving.py` + `solstone/think/entities/merge.py` |
| Speaker-identity entity artifacts (`entities/*/{voiceprints,owner_centroid}.npz`) | `core/crates/solstone-core-speaker-resolve/` via `solstone-core speaker-resolve <verb>`; `solstone/apps/speakers/speaker_resolve_transport.py` is the sole Python transport. The Python transport, the entity-merge module, and the `scripts/entity_corpus.py` fixture-oracle builder were all removed with the reference cut; `core/fixtures/` still holds the vectors it produced. Those oracles are frozen pins — see [`core/fixtures/FROZEN.md`](core/fixtures/FROZEN.md). |
| Entity history content (`entities/*/history/{events,prepared,private}/**`) | `solstone/think/entities/history.py` is the sole writer of history events, prepared staging, and private merge payloads. Whole-entity deletion by entity owners removes `history/` only as part of removing `entities/<id>/`. |
| Owner voice candidate (`awareness/owner_candidate.npz`) | `core/crates/solstone-core-speaker-resolve/` via `solstone-core speaker-resolve <verb>`. |
| Speaker discovery clusters (`awareness/discovery_clusters.json`, `awareness/discovery_clusters.resolved.json`) | `core/crates/solstone-core-convey-shell/` (`speakers_discovery_write.rs`) |
| Speaker candidate pool (`awareness/speaker_candidates.json`) | `solstone/apps/speakers/candidate_tracker.py` |
| Speaker identify operation ledger (`speakers/identify-operations.jsonl`) | `core/crates/solstone-core-speaker-resolve/` via `solstone-core speaker-resolve identify`. |
| Speaker backfill operation ledger (`speakers/backfill-operations.jsonl`) | `core/crates/solstone-core-speaker-resolve/` via `solstone-core speaker-resolve backfill`. |
| Support portal operation ledger and local fingerprint key (`apps/support/portal/operations/*.json`, `apps/support/portal/operation-fingerprint.key`) | `solstone/apps/support/operations.py` |
| Entity resolution ambiguities (`entities/ambiguities.jsonl`) | `solstone/think/entities/ambiguities.py` |
| Entity merge candidates (`entities/review-candidates.jsonl`) | `solstone/think/entities/review_candidates.py` |
| Facet review candidates (`facets/review-candidates.jsonl`) | `solstone/think/facet_review_candidates.py` |
| Speaker review candidates (`speakers/review-candidates.jsonl`) | `solstone/think/speaker_review_candidates.py` |
| Speaker candidate-pair review candidates (`speakers/candidate-pair-review-candidates.jsonl`) | `solstone/think/speaker_candidate_pair_review_candidates.py` |
| Speaker discovery cluster dismissals (`speakers/cluster-dismissals.jsonl`) | `solstone/think/speaker_cluster_dismissals.py` |
| Speaker keep-separate assertions (`speakers/keep-separate.jsonl`) | `solstone/think/speaker_keep_separate.py` |
| Facets (`facets/*/facet.json`, `facets/*/relationships/`) | `solstone/think/facets.py` + `core/crates/solstone-core-facets/` for native Settings writes + `solstone/apps/facets/*` (if/when created) |
| Observations (`observations.jsonl`) | `solstone/think/entities/observations.py` |
| Activities (`facets/*/activities/*.jsonl`) | `solstone/think/activities.py` + `core/crates/solstone-core-facets/` for native Settings writes |
| Activity records (`facets/*/activities/{day}.jsonl`) | `core/crates/solstone-core-facets/src/store/activity_records.rs` |
| Action logs (`config/actions/*.jsonl`, `facets/*/logs/*.jsonl`) | `solstone/apps/utils.py` + `core/crates/solstone-core-facets/` for native Settings writes |
| Facet newsletters (`facets/*/news/*.md`) | `core/crates/solstone-core-facets/src/store/news.rs` |
| Entity talent outcome sidecars (`chronicle/**/<seg>/talents/detection_outcome.json`, `facets/*/entities/*_{observer,review}_outcome.json`) | `core/crates/solstone-core-talent-runtime/src/entities/{detection,observer,review}.rs` |
| Timeline (`chronicle/<day>/timeline.json`, `chronicle/**/<seg>/timeline.json`, root `timeline.json`) | `solstone/apps/timeline/maintenance.py` + `solstone/apps/timeline/talent/segment_summary.py` + `core/crates/solstone-core-maintenance/src/bodies/timeline.rs` |
| Per-segment sense outputs (`chronicle/**/<seg>/talents/{sense.json,facets.json,speakers.json,density.json,change.json,activity.md,sense.md}`) | `solstone/think/sense_splitter.py` |
| `_solstone_processing` records on header-only native describe/transcribe outputs (`chronicle/**/<seg>/{screen,*_screen,audio,*_audio}.jsonl`) | Primary, automatic: sense re-entry via `should_reenter_analysis_output` in `solstone/observe/processing_record.py` — a record-less screen output is re-attempted and the handler *determines* the verdict. Operator bulk tool: `solstone/think/backfill_processing_records.py`, which *stamps a guessed* `state=empty` and is CLI-only, for marker-less, chunk-less legacy fleets; it declines anything carrying a marker or an existing record (`SKIP_MARKER` / `SKIP_HAS_RECORD`, unchanged). |
| Awareness (`awareness/current.json`, `awareness/YYYYMMDD.jsonl`) | `solstone/think/awareness.py` |
| Awareness activity state (`awareness/activity_state.json`) | `solstone/think/thinking.py` |
| Identity (`identity/*.md`, `identity/history.jsonl` audit log) | `solstone/think/identity.py` |
| Day talent-output accumulator (`chronicle/<day>/talents/<name>.jsonl`) | `solstone/think/day_accumulator.py` |
| Talent provenance sidecars (`chronicle/<day>/health/talent-provenance/**`) | `solstone/think/talent_provenance.py` |
| Config (`config/journal.json`) | `solstone/think/journal_config.py` |
| Schedules (`config/schedules.json`) | `solstone/think/schedule_config.py` + `core/crates/solstone-core-system/src/schedule/config.rs` (`mutate_schedule_entries` / `set_schedule_metadata`) |
| Push devices (`config/push-registry.json`) | `core/crates/solstone-core-push/` |
| Local inference operational telemetry (`health/local-inference/YYYYMMDD.jsonl`) | `solstone/think/providers/local_admission.py` |
| Direct-door operational record (`health/direct-door.json`) | `core/crates/solstone-core-system/` (`direct_door.rs`) via `publish_direct_door` / `withhold_direct_door`. `solstone-core-convey-shell` and `solstone-core/src/supervisor/runtime.rs` are callers only; they must not write this path directly. |
| Active-brain state (`health/brain.json`, `health/brain-fingerprint.key`, `health/brain-refresh.lease`) | `core/crates/solstone-core-brain/` via `solstone-core brain <verb>`; `solstone/think/providers/brain_state.py` is transport only |
| Provider install status records and proof cache (`health/providers/{local,parakeet}.json`, `health/providers/{local,parakeet}.proof-cache.json`) | `solstone/think/providers/install_state.py` + `solstone/think/providers/artifact_proof.py` |
| Provider install leases (`health/providers/{local,parakeet}.lease`) | `solstone/think/providers/install_lease.py` |
| Provider runtime health and retry-token records (`health/providers/runtime/{local,parakeet}.json`, `health/providers/runtime/{local,parakeet}.retry-token.json`, `health/providers/runtime/{local,parakeet}.operation.lock`) | `solstone/think/providers/runtime_health.py` |
| Native speakers-analyze install generation (`health/speakers-analyze/install-generation.json`, `health/speakers-analyze/install-generation.lock`) | `core/crates/solstone-core-transcribe/` via `speakers_installation.rs` |
| Provider artifact manifests (`cache/providers/**/.solstone-provider-manifest.json`) | `core/crates/solstone-core-local/src/install/manifest.rs` |
| nvattest appraiser cache (`cache/providers/nvattest/**`) | `solstone/think/providers/nvattest_install.py` |
| Media offload ledger (`health/offload/<YYYYMMDD>.jsonl`) | `solstone/think/offload_ledger.py` + `core/crates/solstone-core-offload/` during native migration; the retained Python writer stays reachable through the Flask backup app until it is retired |
| Pruning-run audit (`health/pruning-runs/<YYYYMMDD>.jsonl`) | `solstone/think/pruning_audit.py` owns `journal_logs` and `raw_media`; `core/crates/solstone-core-offload/` owns `raw_media_offload` — both append to this shared audit file |
| Parakeet server placement record (`health/parakeet-cpp.placement`) | `solstone/think/providers/parakeet_server.py` |
| Hosted backup binding (`backup/hosted/binding.json`) | `solstone/think/backup/hosted.py` |
| Convey config (`config/convey.json`) | `solstone/convey/config.py` + `solstone/think/facets.py` |
| Speaker labels (`chronicle/**/talents/speaker_labels.json`) | `core/crates/solstone-core-speaker-resolve/` via `solstone-core speaker-resolve <verb>`; `solstone/apps/speakers/speaker_resolve_transport.py` is the sole Python transport. `solstone/apps/speakers/attribution.py` prepares requests only; `attribution.py` remains only for the entity-merge flow through `update_speaker_labels`. |
| Speaker corrections (`chronicle/**/talents/speaker_corrections.json`) | `core/crates/solstone-core-speaker-resolve/` via `solstone-core speaker-resolve <verb>`; `solstone/apps/speakers/speaker_resolve_transport.py` is the sole Python transport. `solstone/apps/speakers/attribution.py` prepares requests only; `attribution.py` remains only for the entity-merge flow through `remap_speaker_corrections_for_entity_merge` and `apply_entity_merge_segment_inverse`. |
| Stream identity (`chronicle/**/<seg>/stream.json` marker + `streams/<name>.json` state) | `solstone-core-segment` (`advance_unbound_stream` / `advance_bound_stream`); observer prune repairs a survivor's predecessor pointers locally |
| Observer ingest manifest (`chronicle/**/<seg>/ingest.json`) | `solstone-core-ingest-resolve` (`write_ingest_manifest`) |
| Link service state (`link/ca/cert.pem`, `link/ca/private.pem`, `link/ca-staging/**`, `link/authorized_clients.json`, `link/state.json` including optional `locked_at`, `link/tokens/account.json`, `link/totp.json`) | `solstone/think/link/ca.py` + `solstone/think/link/establish.py` + `solstone/think/link/auth.py` + `solstone/think/link/paths.py` |
| Native pairing nonces (`link/nonces.json`) | `core/crates/solstone-core-sol-link/` |
| Chronicle day content (`chronicle/YYYYMMDD/**`) | The capturing module (observer, importer) per its declared outputs |
| Index (SQLite, `indexer/*`) | `solstone/think/indexer/*` |
| Observer registry and sync history (`apps/observer/observers/*.json`, `apps/observer/observers/*/hist/*.jsonl`) | `solstone/apps/observer/utils.py` |
| Import ingest/resolve staging (`imports/**`, excluding native body state and `imports/oura.json`) | `solstone/apps/import/ingest.py` + `solstone/apps/import/resolve.py` + `solstone/apps/import/facet_ingest.py` + `solstone/apps/import/journal_sources.py` — HTTP-ingest + resolve staging state, plus the remote-ingest bundle under `imports/<id>/`. `journal_sources.py` owns only its `create_state_directory` `imports/` initializers; its source registry is app-storage. Non-body import-bundle and sync-cursor content under `imports/<id>/` and `imports/<backend>.json` is written by `solstone/think/importers/{utils,cli,shared,sync}.py` (local/CLI import flows + sync cursor), and `solstone/think/importers/plaud.py` installs streamed imported audio onto `imports/<id>/<name>` via journal_io's `install_file` primitive, as importer declared outputs (L7). |
| Body import state (`imports/health-dedupe.sqlite`, `imports/oura.json`, Oura token fields under `config/journal.json`, `imports/_approvals/**`, native `imports/body-*` bundles with manifests, envelopes, normalized shards, ledgers, and approved raw assets) | `core/crates/solstone-core-body-{ingest,source,store,rebuild}/` via `solstone-core body <verb>` owns Apple and Oura reads, Oura network/token/cursor mutation through the native journal-config CAS door, approval enforcement, publication, dedupe, and rebuild. `solstone/think/body_native.py` is process transport only; the retained Python Apple/Oura readers are read-only detect/preview parsers and must not write body state. Creation of `_approvals/**` remains a separately approved setup action. |
| Operational-log/cache pruning and root task-log compaction — deletion/compaction only, across the dated allowlist (`chronicle/<day>/health/*.{log,jsonl}`, top-level `talents/<talent>/<epoch_ms>.jsonl` run logs + `talents/<YYYYMMDD>.jsonl` day indexes, `tokens/`, `health/local-inference/`, `awareness/`, `config/actions/`, `facets/*/logs/`, `apps/observer/observers/*/hist/`, `.cache/cogitate-history/`) plus root `task_log.txt` epoch lines older than the same retention window | `solstone/think/retention_executor.py` loads configuration, short-circuits disabled retention, and adapts receipts; `core/crates/solstone-core-retention` + `core/crates/solstone-core-retention-cli` plan, remove, and compact this domain. No Python code writes this domain anymore. |

If you're about to write to a domain from a module not in this table, stop and route through the owner.

**Native `solstone call <app> <verb>` handlers keep the HTTP boundary.** Each
journal-data command is declared by an app-local native authority and reaches the
journal through the generated native HTTP client. The positive native-sol
inventory, coverage, architecture, and conformance gates enforce that boundary.
The `solstone call journal` group is a native HTTP surface declared under
`solstone/think/tools/native/journal/`.

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
- Acceptable alternative: `--dry-run` defaulting to `False` *only if* the subcommand name is unambiguously a write verb AND the command's primary user journey is the write (e.g., `solstone call entities create`).

"Bootstrap", "backfill", and "resolve-names" are not unambiguous — default them to dry-run.

### L6 — Indexers never mutate source data

An indexer's job is to build indexes from source-of-truth data. Indexers may not mutate the source data they read. Re-running `journal indexer --rescan` on an unchanged journal must be a no-op for domain state.

### L7 — Importers only write to imports/

Importers write source material to `imports/` and the raw-content areas of `chronicle/`. They may not create or modify entities, facets, observations, or other cross-cutting state. If an importer needs to create an entity for deduplication, it calls a domain-owned `seed_entity()` function in `solstone/think/entities/` that surfaces the write explicitly.

### L8 — Hooks have declared outputs

Post-processing hooks (`solstone/think/hooks.py`, `solstone/talent/*.py` hook functions) declare every path they will write in their frontmatter. The hook runner validates that all actual writes match the declaration. Writes outside the declared set fail loudly — raise at runtime; assert in tests.

### L9 — Event handlers are idempotent

Any function that handles a callosum event, a scheduled tick, or a supervisor-started automation is idempotent w.r.t. on-disk state. Append-only history records dedupe by a natural key (usually `(day, segment)` or `(day, segment, ts)`). Before adding a write to an event handler, ask: "what happens if this event fires twice?"

## 8. Coding invariants

The rules above govern *where* code lives. The rules below govern *how* code behaves. They exist because we got burned.

- **No backwards-compatibility shims.** All code that depends on this project lives in this repository — never add fallback aliases, re-exports for moved symbols, deprecated-parameter handling, or legacy support code. When renaming or removing something, update every usage directly. For journal data-format changes, update the owning writer; do not add a compatibility layer. One-time `journal maint` migrations are retired — new journals are clean installs. Cogitate agents default to adding shims; resist this.
- **Trust journal resolution.** `solstone-core-journal::resolve_journal_path` is the resolver. Application code, agent prompts, subprocess environments, and service files must not set `SOLSTONE_JOURNAL`. Use `journal config journal <path>` to rewrite the wrapper path. See `docs/environment.md`.
- **SPDX header on every source file.** Rust files begin with:

  ```
  // SPDX-License-Identifier: AGPL-3.0-only
  // Copyright (c) 2026 sol pbc
  ```

  (`//` for JavaScript.) Markdown, text, and prompt files don't need it.
- **Fail loudly, not silently.** Raise specific exceptions with clear messages; use the `logging` module, not `print`. Validate inputs at module boundaries. A silent swallow in production costs days of forensics — an error at the boundary is free.
- **Trust internal code.** Don't add defensive validation for things internal callers can't violate. Validate at system boundaries (user input, external APIs, imported files) — not between modules you control.

Generic software principles (DRY, KISS, YAGNI, single responsibility, small focused commits) apply; see `docs/coding-standards.md` for the full list.

## 9. File headers, naming, dependencies

- **SPDX header** as above — mandatory on source code files.
- **Naming:** modules / functions / variables `snake_case`; classes `PascalCase`; constants `UPPER_SNAKE_CASE`; private members `_leading_underscore`. Full table in `docs/coding-standards.md`.
- **Dependencies:** workspace crates inherit from `core/Cargo.toml`. `make install` is retired.

## 10. Commit hygiene

- Small, focused commits with descriptive messages.
- Validate each commit with focused checks appropriate to its diff. Use efficient `make ci` for routine validation. The full `make ci-full` gate belongs on the exact final tree before merge or release, not before every intermediate commit.
- Run `git` commands directly — not `git -C` — you're already in the repo.
- Don't commit runtime artifacts written under `tests/fixtures/journal/` by `make dev` / `make sandbox` (`.gitignore` covers them; verify with `git status` anyway).

## 11. Where to go deeper

Bare links don't motivate clicking. Each entry below says when you actually need the doc.

| Doc | When to read |
|-----|--------------|
| `docs/APPS.md` | **Required before adding or moving a Convey app** — native registry, `*-web` crates, journal `solstone/apps/` storage |
| `docs/THINK.md` | Understanding the think-layer pipeline (importers, indexer, segment/stream processing) |
| `docs/CORTEX.md` | Modifying talent execution, cortex lifecycle, talent process management |
| `docs/COGITATE.md` | The cogitate talent runtime contract — cwd/workspace, the `solstone`-CLI-authoritative journal access, raw-read bound, access tiers, finalization, disallowed assumptions, and the in-context preamble constant. Read before authoring/editing a talent prompt. |
| `docs/GENERATE.md` | The `generate` contract — the record vocabulary for asking the model boundary for one completion, its two framings, and the invariants it guarantees. Read before writing anything that calls a model, or that consumes a completion's outcome. |
| `docs/CALLOSUM.md` | Adding a new tract/event, debugging message flow |
| `docs/CONVEY.md` | Framework-level web changes (as opposed to an individual app) |
| `docs/OBSERVE.md` | Capture-side work: new modalities, transcription, sensing |
| `docs/SOLCLI.md` | Adding a new `solstone <cmd>` or `solstone call <app> <verb>` |
| `docs/PORTING.md` | Rust workspace rules: edition, iOS canary, native-dep proof |
| `docs/PROMPT_TEMPLATES.md` | Modifying talent prompt format or frontmatter |
| `docs/PROVIDERS.md` | Three-lane provider architecture: active-brain resolution, local/BYO/confidential lanes, and honest no-fallback failure semantics |
| `docs/testing.md` | Test structure, fixtures, debugging test isolation |
| `docs/environment.md` | Journal path resolution, managed-wrapper behavior, service install details, and `SOLSTONE_JOURNAL` rules |
| `docs/CHANNEL_ADAPTERS.md` | Release channel adapter config, scrub-gate expectations, and operator-safe placeholders |
| `docs/release-evidence-contract.md` | **Required before changing the retained release ledger schema registry** — why such a change breaks already-cut candidates, what `schema_version` does and does not tolerate here, and the frozen-fixture rule |
| `docs/journal-format-contract-maintenance.md` | Changing a committed journal at-rest format (observer ingest envelopes, `stream.json`, `audio.jsonl`, `screen.jsonl`) — schema floor vs producer-local requirements, and which relaxations are safe |
| `docs/coding-standards.md` | Full naming conventions, ruff config, dep-management details — reference for everything not promoted into this file |
| `docs/project-structure.md` | Canonical directory layout; resolving "where does this file go" debates |
| `docs/DOCTOR.md` | Diagnostics and debugging a running system |
| `docs/SCREEN_CATEGORIES.md` | Screen-understanding classifier taxonomy (observe side) |
| `docs/VENDOR.md` | Vendor-level integrations |
| `docs/design/` | Per-subsystem design docs |
| `docs/JOURNAL.md` | **Breadcrumb only** — redirects to `core/payload/solstone/talent/journal/SKILL.md`, the progressive-disclosure journal-layout reference |
| `core/payload/solstone/talent/journal/SKILL.md` | Journal layout, vocabulary, and `solstone call journal` CLI (loaded by cogitate talents on demand via skills) |
| `core/payload/solstone/talent/journal/references/cli.md` | Full `solstone call journal` reference, including **Talent CLI Boundaries** (which infrastructure commands cogitate talents must not call) |

The live journal also carries `journal/AGENTS.md` as its runtime-facing breadcrumb.

`docs/BACKLOG.md` and `docs/ROADMAP.md` are not the product SOT.

## 12. What this file is NOT

- **Not a runtime guide for cogitate talents.** Runtime CLI restrictions on talents live in `core/payload/solstone/talent/journal/references/cli.md` § Talent CLI Boundaries. If you're tuning what a talent can or cannot call, look there, not here.
- **Not the journal-layout reference.** `core/payload/solstone/talent/journal/SKILL.md` + its `references/` is the cogitate-audience entry point. This file describes *how those commands are implemented*, not *which ones talents can't call*.
- **Not an operations manual.** For debugging a live system see `docs/DOCTOR.md`; for setup and service lifecycle, see [INSTALL.md](INSTALL.md) (owner install), [CONTRIBUTING.md](CONTRIBUTING.md) (developer install), `journal setup`, and `journal service`.
