# solstone Developer Guide

This file is the **developer guide** for the solstone-journal repository. Read it before writing code. The Rust conversion is closed: there is no Python reference implementation and no Python product code remains in this repository. Every write path lives in a crate under `core/crates/`.

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
3. **For system orientation** — there is no single file or TUI that ties callosum, supervisor, and service status together in one read. The closest thing is `journal top` (`core/crates/solstone-core-top/`), a live rendered view of supervised services. For the underlying wiring, read the callosum, supervisor, and system crates under `core/crates/` (`solstone-core-callosum`, the supervisor module in `solstone-core`, `solstone-core-system`).
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
| `core/crates/solstone-core-convey-shell/` | Web app framework — shell, session gate, app registry | layout / framework-level UI changes | `docs/CONVEY.md`, `docs/CONVEY-FRONTEND.md` |
| `core/crates/solstone-core-*-web/` + `convey-shell/assets/` | Convey apps — registered in `APP_REGISTRY`, served by a `*-web` crate or shell assets | adding a user-facing feature, a `solstone call <app>` verb, a UI surface | `docs/APPS.md` |
| `core/payload/solstone/talent/` | The shipped payload's talent tree: AI talent configs (markdown prompts) + installed router skills (`solstone`, `journal`); app fragments feed generated router references. This is the checkout's stand-in for the installed `share/` prefix — see `core/payload/README.md` | defining or tuning a talent; updating router guidance | `core/payload/solstone/talent/journal/SKILL.md`, `docs/PROMPT_TEMPLATES.md` |
| `core/` | Rust workspace — thin `solstone-core` bin plus library-first adapter crates | Rust scaffold, gates, workspace rules | `docs/PORTING.md` |
| `scripts/` | Repo maintenance scripts, most still Python. Reached by `make install-checks`, never by `make ci`; several are hygiene/inventory checkers over the Rust tree (native-sol architecture, root contract, journal access rejection inventory), not remnants of a deleted implementation | tooling that guards the codebase | channel adapters: `docs/CHANNEL_ADAPTERS.md` |
| `tools/journal_device_sim/` | Dependency-free linked-device fixture simulator; native `solstone link` remains the PL/SPL and identity boundary | composed ingest, reconciliation, recovery, and field-journal validation through a disposable receiver | `tools/journal_device_sim/README.md` |
| `tests/` | `tests/fixtures/journal/` mock journal. Rust tests live beside their crates under `core/crates/*/tests/`; there is no separate top-level Python test tree | `make dev` / `make sandbox` use the fixtures as the journal | `docs/testing.md` |
| `tests/js/` | Three JavaScript harness files (`modal_layer_harness.js`, `shell_boot_menu_harness.js`, `speakers_deeplink_harness.js`) left over from the Python-era browser test tree. Nothing in the current Makefile, scripts, or Rust harnesses invokes them — they are orphaned, not a live testing surface. Treat as a cleanup candidate, not a pattern to add to | you're deciding whether to add a JS test here — don't, until something actually runs this directory again | — |
| `docs/` | All longform documentation | reference lookups; never your first stop | §11 below |
| `journal/` | The live journal (user data). Git-ignored content; checked-in template (`AGENTS.md`, skills symlinks) | **rarely as a coder** — modify `core/crates/` or `core/payload/solstone/talent/`, not journal data | `core/payload/solstone/talent/journal/SKILL.md` |

Top-level dirs intentionally not in the table: `.venv/`, `scratch/`, `logs/`, `tmp/`, `observers/`, `routines/`, `skills/` — not active coder surfaces. `solstone/` is not a coder surface either: it now holds exactly three files (`solstone/apps/devices/ingest.schema.json`, `solstone/think/detect_created.md`, `solstone/think/detect_created.schema.json`) and no product code.

## 3. Mental model

**The pipeline:** `observe` (capture) → JSON transcripts in `journal/chronicle/YYYYMMDD/` → `think` (analyze) → SQLite index + derived artifacts → `convey` (web UI) and `solstone call` CLIs.

**Think is the center.** observe feeds it raw material; convey + apps render its outputs; talent prompts + cortex run AI against it; indexer makes it searchable. A change in the think-layer crates usually ripples outward.

**Key concepts, priority-ordered:**

- **Journal** — the on-disk record rooted at `journal/` in the repo. Every day is a `journal/chronicle/YYYYMMDD/` directory. Segments (timestamped capture windows) are anchored to creation/modification time, not content "about" time. `solstone-core-journal::resolve_journal_path` is the resolver. Source-checkout installs inherit `SOLSTONE_JOURNAL` from the managed wrapper at `~/.local/bin/solstone`; a tree install puts `solstone` and `journal` on PATH (see `INSTALL.md`). Tests and sandboxes set the env explicitly. Application code must not set it itself. See `docs/environment.md`.
- **Talents** — AI processors (markdown prompt + a closed set of typed Rust hook stages, never an arbitrary plugin). Each has a config in `core/payload/solstone/talent/<name>.md` with frontmatter that declares hooks, priority, model, and output. Cortex spawns them as subprocesses.
- **Callosum** — Unix-socket JSON message bus at `journal/health/callosum.sock` on Unix, with an authenticated Windows named-pipe transport derived from the same endpoint. Its Windows boundary protects cross-user/cross-identity and remote-network access, not malware already running as the same user/SID. Real-time event distribution across services (`tract` + `event` + payload). If components need to talk asynchronously, they talk through callosum.
- **Cortex** — process manager for talent runs. Listens on callosum (`tract="cortex"`, `event="request"`), resolves the sibling `solstone-core` binary and spawns it with `__talent-worker`, writes `<talent>/<ts>_active.jsonl` then renames to `<talent>/<ts>.jsonl` on completion, broadcasts all events back through callosum. Read `docs/CORTEX.md` before modifying talent execution.
- **Facets** — project/context scopes (`work`, `personal`, …). Group related entities, activities, and relationships. Facet data lives under `journal/facets/<facet>/`, fully owned by `core/crates/solstone-core-facets/`.
- **Entities** — tracked people / projects / tools. Extracted from transcripts and accumulated across time. Canonical records in `journal/entities/<slug>/entity.json`, owned by `core/crates/solstone-core-entity/`.
- **Activities** — scheduled or observed "things that happen" (meetings, deadlines, anticipated events). Per-facet JSONL at `journal/facets/<facet>/activities/<day>.jsonl`. Sources: `anticipated` (from `core/payload/solstone/talent/schedule.md`), `user` (manual), `cogitate` (talent-inferred).
- **Indexer** — reads journal state, builds SQLite + FTS5 index. **Never** mutates source data (§7 L6). Rerunning on unchanged data is a no-op. Ownership is split: `solstone-core-indexer-store` owns the schema, connection, and writes; `solstone-core-indexer` computes what to persist (discovery, edges, entity search, metadata) and calls into indexer-store; `solstone-core-indexer-query` is read-only.
- **Supervisor** — top-level process manager. Starts/restarts services, talks to callosum. `journal supervisor` / `journal start`. A supervised service is retried indefinitely with backoff rather than permanently given up on; there is no give-up state.

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
| `make install` | **Retired.** Install the journal from the distribution tree; develop against this checkout with Cargo directly. |
| `make skills` | Regenerate generated router references, then rewrite the `solstone` + `journal` router skill symlinks into `journal/`. |
| `make update` | Upgrade all deps to latest, regenerate `uv.lock` (the remaining `scripts/` Python tooling's own dependencies — not a product dependency set). |
| `make update-prices` | Refresh genai-prices model-cost data when adding a new provider model or when pricing tests fail. |
| `make clean` | Remove build artifacts, caches, and the skill symlink dirs (`journal/.agents/`, `journal/.claude/`). Does not touch `.venv/`. Before `cargo clean`, refuses if a live process has an open file, mapping, cwd, or executable under this checkout's `RUST_TARGET_DIR` (`core/target`, or `CARGO_TARGET_DIR` if set) and prints blocker pids+paths. Override with `CLEAN_FORCE=1`. |
| `make clean-install` | Runs `clean` first (same live-use refuse / `CLEAN_FORCE=1`), then deletes `.venv/` and `.installed`, then exits 1 as retired. |

### Run the stack

| Target | When to use |
|--------|-------------|
| `make dev` | Start the full stack (supervisor + callosum + sense + cortex + convey) against `tests/fixtures/journal/`, no observers, no daily processing. Primary inner-loop for UI work. Ctrl-C to stop. |
| `make sandbox` | Ephemeral background sandbox: copies fixtures to a temp journal, starts supervisor in the background, waits for readiness, writes `.sandbox.pid` / `.sandbox.journal`. Always follow with `make sandbox-stop`. |
| `make sandbox-stop` | Terminate the backgrounded sandbox and clean up state files. |

### Format, lint, test

| Target | When to use |
|--------|-------------|
| `make` / `make all` / `make build` | Build the native Rust workspace, excluding the three host-native ONNX-linked helper packages (`solstone-core-speakers-analyze`, `solstone-core-speakers-onnx`, `solstone-core-vad-analyze`). |
| `make build-sandbox-processing` | Opt in to build the two native processing helpers and their shared runtime bundle into the effective Cargo target directory. |
| `make check-rust-sandbox-processing-build` | Verify an existing processing bundle and both helpers' loader-independent startup; it never builds or repairs. |
| `make format` | Format the Rust workspace with Cargo fmt; modifies Rust source. |
| `make format-check` | Cargo fmt dry-run (`cargo fmt --all -- --check`); one of the Rust-only CI checks. |
| `make test` | Focused code-test path for contributors. Runs the selected library/binary unit harnesses, then prints its source-derived evidence boundary. It does not run routine Clippy. The report names the three native ONNX-linked packages excluded from unit execution, the classified same-crate product-test modules not run by this command and defaulted in `ci-full`, and the broader integration, doctest, native/runtime/platform, dependency-policy, package/release, and other full-gate evidence it did not run. |
| `make check-journal-device-sim` | Standard-library tests for the repository-local Python device simulator; no journal, external network, credentials, or product runtime. |
| `make ci` | Routine code-landing gate: formatting, CI-topology validation, library/binary Clippy, and the same selected library/binary unit harnesses. Its report derives the evidence boundary from the Makefile: the three native ONNX-linked packages neither routine check runs, the four packages whose classified product-test modules require `full-tests`, and broader `ci-full` evidence. |
| `make ci-full` | Registry-driven (`core/ci/suites.toml`) final-tree operator gate. It runs selected entries independently, including package-specific classified full-test legs and the existing `clippy-full` entry with feature-enabled linting for same-crate product-test modules, continues after entry failures, applies per-entry timeouts, and writes a revision-bound receipt under `target/ci-receipts/`. It owns the broader integration, native, platform, policy, package, release, and host evidence. Requires a clean Git worktree and a prior `make ci-full-prep`; run it on the exact final-tree SHA, never an in-progress diff. |
| `WIN_REMOTE_HOST=user@host make win-host-ci` | Transfer an exact, source-bound snapshot to the configured Windows build host and run the first native MSVC journal substrate gate. It builds `solstone-core-journal`, `solstone-core-journal-config`, `solstone-core-journal-io`, `solstone-core-system`, and `solstone-core-win-owner-rail`; runs journal-config, journal-io (library, lock-component, detailed-atomic-publication), and the required journal library tests; then proves NTFS and ReFS publication, Cortex-use recovery, managed-log reference, and stale-heartbeat cleanup through separate native receipt children, plus the mandatory ordinary-owner inventory control. The final `JOURNAL_WIN_HOST_CI_VERIFIED` line reports each of those as `executed/pass`; Cloud Files sync-root registration is the one leg that stays opt-in and reads `skipped` unless `JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST=1`. |
| `make verify` | Alias for `make ci`. |
| `make install-checks` | Directly runnable preflight chain over the remaining Python-era hygiene scripts (formatting, several `check-native-sol-*` architecture/inventory checks, journal-format-contract, dependency-policy) plus the Rust gates; no longer called by `ci` or `verify`. Several of its own listed steps are dead scaffolding for checks that were removed — a blank banner line with nothing following it means that particular check no longer exists, not that it silently passed. |

Use the narrowest applicable `make check-rust-*` target for a focused change, then `make test` for selected unit evidence and `make ci` before commit. Run `make ci-full` on the exact final-tree SHA before merge or release for full host evidence. These paths are enforced by the
[`ci_gate_purity` contract tests](core/crates/solstone-core-repository-contracts/src/contracts/ci_gate_purity.rs).
The native Windows rail is operator-run and deliberately separate from
`ci-full`. `make win-host-ci` refuses untracked, non-ignored files, binds the
transferred Git snapshot to the workspace lockfile digest, and reports success
only after the remote checkout acknowledges both values. Treat the runner's explicit
not-run list as the evidence boundary; a transport pass is not filesystem,
Callosum, packaging, install, signing, or smoke evidence.
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

- Put same-crate behavior tests in `#[cfg(test)]` modules beside the owning code. Cargo's target classification (`--lib`/`--bins` versus a `--test` binary) is a build-cost decision, not a claim about validation scope. Deterministic same-crate logic and cheap source/static contracts belong in the routine library/binary gate. In a package that exposes classified same-crate product tests, a module that crosses a product filesystem, process, service, network, platform, or native-runtime boundary uses the package's non-default `full-tests` feature and an explicit classified route; its location under `src/` does not make it routine evidence. Keep a mixed module wholly broad or split it only when the smaller group has a coherent boundary of its own. Put public API and process contracts in the narrowest owning leaf crate unless the behavior genuinely belongs to the aggregate `solstone-core` composition boundary.
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
- Iterate with the narrowest command, such as `cargo test --manifest-path core/Cargo.toml -p <package> --lib` or the affected `--test <harness> <test>`. For `solstone-core-sol-link`, `solstone-core-convey-body`, `solstone-core-facets`, and `solstone-core-describe`, a default-feature `--lib`/`--bins` selection contains only routine same-crate evidence; an explicit `--test` harness keeps its declared validation scope. Run the matching `make check-rust-classified-full-tests-<suffix>` target for broader `full-tests` same-crate evidence and `make check-rust-classified-full-clippy-<suffix>` for its feature-enabled lint evidence. `make ci` is the routine gate: a formatting check, topology validation, library/binary Clippy, and library/binary unit harnesses. It does not link or execute integration-test binaries, so run affected harnesses directly.
- Run [`make ci-full`](Makefile) once on the exact final tree before merge or
  release. This host-conditional gate reports unsupported platform legs as
  skipped, so run affected platform lanes on their supported hosts. Separately
  run `make check-rust-race` for concurrency-sensitive supervisor changes.

### Service management (systemd / launchd)

`journal setup` is the runtime install path once you have a `journal` binary, from a tree install or from `cargo build` in this checkout. `make install` is retired. It installs or refreshes the managed wrappers, installs the Claude Code skill when Claude is configured, and starts the background service on port 5015 by default. After the first run, the wrappers at `~/.local/bin/solstone` and `~/.local/bin/journal` let you use `solstone` and `journal` from anywhere. Use `journal service <install|start|stop|restart|status|logs>` for manual service operations.

| Target | When to use |
|--------|-------------|
| `make service-logs` | Tail the installed service's logs. |

### Other

| Target | When to use |
|--------|-------------|
| `make pre-commit` | Install pre-commit hooks (optional). |
| `make versions` | Print versions of Python, uv, and key deps (the remaining `scripts/` tooling's own environment, not a product dependency). Diagnostic. |

### Release and transparency

See [`docs/PORTING.md`](docs/PORTING.md) and
[`docs/release-evidence-contract.md`](docs/release-evidence-contract.md).

### Don't use

| Target | Why not |
|--------|---------|
| `make uninstall` | Disabled by design. Use `journal service uninstall`, `solstone skills uninstall`, or `make clean-install` to rebuild the local dev env. |

## 6. Testing quickstart

- **Test hierarchy, narrowest to broadest:** start with `cargo test --manifest-path core/Cargo.toml -p <crate> --lib` (or the affected `--test <harness>`) for the area you are touching. For the four behavior-classified packages named in the Rust test topology section, the default-feature `--lib`/`--bins` selection contains only routine same-crate evidence; explicit integration harnesses keep their declared validation scope, and the package's classified full-test and full-Clippy Make targets cover broader `full-tests` same-crate evidence. `make test` is selected library/binary unit evidence, not a full workspace sweep; read its source-derived omission report. `make ci` is the routine code-landing gate, adding formatting, topology validation, and Clippy to that same unit boundary. `make ci-full` is the final-tree operator gate covering the broader integration, native, platform, policy, package, release, and host evidence.
- **There is no Python product test suite.** Rust tests live beside their crates under `core/crates/*/tests/` and in `#[cfg(test)]` modules. `tests/` holds only the fixture journal (`tests/fixtures/journal/`) and three orphaned JS harness files under `tests/js/` that nothing currently invokes (§2).
- **After editing `solstone/convey/` or `solstone/apps/`:** these paths no longer exist — convey and app code lives under `core/crates/solstone-core-convey-shell/` and the matching `*-web` crates. Run `journal down && journal up` to fully restart the stack after a native change.
- **Runtime artifacts:** `make dev` writes them into the fixtures journal, where `tests/fixtures/journal/.gitignore` covers them. `make sandbox` uses an ephemeral copy and leaves only its `.sandbox.pid` and `.sandbox.journal` state files until `make sandbox-stop` removes them.
- **Test invariants, not snapshots.** A test asserts what must hold in *every* valid state of the system — not what happens to be true today. Never pin a test to hand-edited prose (CHANGELOG / README / docs), to a value the system is *designed* to change (a version, a date, a growing count), or to a transient state. The tell: if doing the correct next thing — cut a release, rename a label, graduate a shipped changelog entry — turns the test red, the test is wrong, not the system. And test the code that *produces* a fact, never the rendered text about it.

Full depth: `docs/testing.md`.

## 7. Layer hygiene — required reading (L1–L9)

**Why this lives here.** A codebase-wide audit in April 2026 found 14 layer-hygiene violations in the Python think/apps trees that predated the Rust conversion. Infrastructure modules (indexer, importers, schedulers) were silently writing domain state; CLI read-verbs were mutating; get-prefixed functions were creating records on miss. These invariants encode the rules the audit distilled, so the same landmines don't get re-planted in the Rust crates. They're inlined here because a one-click-away invariant is a routinely-skipped invariant.

⚠ **L1/L2 domain-boundary discipline has no automated grep check today.** The old low-bar checker, `scripts/check_layer_hygiene.py`, read the Python tree the conversion deleted; it was removed rather than left passing vacuously. L1/L2 are held by review and by Rust module/crate boundaries, not by a gate. **L8 is the exception** — see below.

### L1 — Layer boundaries are load-bearing

Each module family has a declared responsibility. Infrastructure modules (indexer, importer, scheduler, search, graph, stats) may write **only their own output artifacts**. They may not create, modify, or delete domain state (entities, facets, observations, activities, events, chronicle day content). If an infrastructure module needs to trigger a domain mutation, it emits a callosum event or invokes a `solstone call <domain> <verb>` subprocess — never writes domain state directly.

### L2 — Domain write ownership

Each domain has exactly **one** write-owning module (or one tightly-scoped family of modules), or is called out below as split by operation type, or as a currently-real gap with no writer at all. No module may write another domain's on-disk state.

Verified directly against source, not against this table's own history — a stale row here is worse than no row. Where a domain has genuinely lost its writer since the Python cutover, it is marked as a gap rather than assigned a plausible-sounding crate; do not "fix" a gap row by inventing an owner.

| Domain | Write-owning module(s) |
|--------|------------------------|
| Entities (`entities/*/entity.json`) | `core/crates/solstone-core-entity/` (`store/write.rs`, `store/create.rs`, `store/merge.rs`, `store/lifecycle.rs`) |
| Entity voiceprints (`entities/*/voiceprints.npz`) | `core/crates/solstone-core-entity/src/store/voiceprints.rs` (`save_voiceprints_batch`), called by `core/crates/solstone-core-speaker-resolve/` |
| Entity owner-centroid (`entities/*/owner_centroid.npz`) | `core/crates/solstone-core-speaker-resolve/src/owner_centroid.rs` (a different crate from the voiceprints write above; don't conflate the two files) |
| Entity history content (`entities/*/history/{events,prepared,private}/**`) | `core/crates/solstone-core-entity/` (`store/write.rs`, `store/history.rs`, `store/merge.rs`, `store/merge_payload.rs`, `store/undo.rs`) |
| Owner voice candidate (`awareness/owner_candidate.npz`) | `core/crates/solstone-core-speaker-resolve/src/owner_candidate.rs` |
| Speaker discovery clusters (`awareness/discovery_clusters.json`) | `core/crates/solstone-core-convey-shell/src/speakers_discovery_write.rs` (`write_discovery_cache`) |
| Speaker discovery clusters, resolved (`awareness/discovery_clusters.resolved.json`) | `core/crates/solstone-core-speaker-resolve/src/identify_forward_phases.rs` (`replace_resolved_clusters`), a **different crate** from the unresolved file above despite sharing a directory. `solstone-core-entity`'s merge path only deletes/rolls back the unresolved file during an entity merge; it does not write either. |
| Speaker candidate pool (`awareness/speaker_candidates.json`) | `core/crates/solstone-core-speaker-resolve/src/candidate_tracker.rs` |
| Speaker identify operation ledger (`speakers/identify-operations.jsonl`) | `core/crates/solstone-core-speaker-resolve/src/identify_operations.rs` |
| Speaker backfill operation ledger (`speakers/backfill-operations.jsonl`) | `core/crates/solstone-core-speaker-resolve/src/backfill_operations.rs` |
| Support portal operation ledger and local fingerprint key (`apps/support/portal/operations/*.json`, `apps/support/portal/operation-fingerprint.key`) | `core/crates/solstone-core-support-portal/src/ledger.rs` |
| Entity resolution ambiguities (`entities/ambiguities.jsonl`) | `core/crates/solstone-core-entity/src/store/write.rs` (`record_ambiguity_observation`, `record_ambiguity_choice`, `mutate_ambiguities`) |
| Entity merge candidates (`entities/review-candidates.jsonl`) | `core/crates/solstone-core-entity/src/store/review_candidates.rs` |
| Facet review candidates (`facets/review-candidates.jsonl`) | `core/crates/solstone-core-facets/src/store/review_candidates.rs` |
| Speaker review candidates (`speakers/review-candidates.jsonl`) | `core/crates/solstone-core-speaker-resolve/src/speaker_review_candidates.rs` |
| Speaker candidate-pair review candidates (`speakers/candidate-pair-review-candidates.jsonl`) | `core/crates/solstone-core-speaker-resolve/src/speaker_candidate_pair_review_candidates.rs` |
| Speaker discovery cluster dismissals (`speakers/cluster-dismissals.jsonl`) | `core/crates/solstone-core-convey-shell/src/speakers_discovery_write.rs` (same crate as the discovery-cluster write above, not speaker-resolve) |
| Speaker keep-separate assertions (`speakers/keep-separate.jsonl`) | `core/crates/solstone-core-speaker-resolve/src/keep_separate.rs` |
| Facets (`facets/*/facet.json`, `facets/*/relationships/`) | `core/crates/solstone-core-facets/src/store/write.rs` — full lifecycle: create/update/mute/delete/rename plus relationship link/detach. A second, legitimate producer, `core/crates/solstone-core-import-web/src/facet_ingest.rs`, writes `facet.json` directly for bulk journal-archive import/sync, a different flow (bulk import vs. interactive) rather than competing ownership. |
| Observations (`observations.jsonl`) | Ordinary record/append: `core/crates/solstone-core-facets/src/store/observations.rs`. Cross-entity merge/undo relocation: `core/crates/solstone-core-entity/src/store/{merge,undo}.rs`. Split by operation type, not drift. |
| Activity definitions (`facets/*/activities.jsonl`) | `core/crates/solstone-core-facets/src/store/activities.rs` |
| Activity records (`facets/*/activities/{day}.jsonl`) | `core/crates/solstone-core-facets/src/store/activity_records.rs` (a sibling module to activity definitions above, same crate) |
| Action logs (`config/actions/*.jsonl`, `facets/*/logs/*.jsonl`) | `core/crates/solstone-core-facets/src/action_log.rs` (`append_action_log`, `append_action_log_for_day`) |
| Facet newsletters (`facets/*/news/*.md`) | `core/crates/solstone-core-facets/src/store/news.rs` (`write_news_file`), called by the CLI (`journal news`) and the auto-generated newsletter talent |
| Entity talent outcome sidecars (`chronicle/**/<seg>/talents/detection_outcome.json`, `facets/*/entities/*_{observer,review}_outcome.json`) | `core/crates/solstone-core-talent-runtime/src/entities/{detection,observer,review}.rs` |
| Timeline (`chronicle/<day>/timeline.json`, `chronicle/**/<seg>/timeline.json`, root `timeline.json`) | `core/crates/solstone-core-maintenance/src/bodies/timeline.rs` (sole code-owner for all three variants) (`rollup_day`, `rollup_master`, `write_segment_timeline`). `solstone-core-talent-runtime`'s `timeline:segment_summary` talent stage calls into this same primitive; it is an orchestration caller, not a separate writer |
| Per-segment sense outputs (`chronicle/**/<seg>/talents/{sense.json,facets.json,speakers.json,density.json,change.json,activity.md,sense.md}`) | `core/crates/solstone-core-think-cli/src/segment.rs` (`write_sense_outputs`, `write_change`) |
| `_solstone_processing` records on header-only native describe/transcribe outputs (`chronicle/**/<seg>/{screen,*_screen,audio,*_audio}.jsonl`) | Shared judgment/vocabulary only (not itself a writer): `core/crates/solstone-core-processing-record/`. Per-handler header writes: `core/crates/solstone-core-transcribe/`, `core/crates/solstone-core-describe/`, `core/crates/solstone-core-depict/`. Bulk repair CLI: `journal backfill-processing-records` (`core/crates/solstone-core-backfill-cli/`) |
| Awareness (`awareness/current.json`, `awareness/YYYYMMDD.jsonl`) | `core/crates/solstone-core-facets/src/store/awareness.rs` |
| Awareness activity state (`awareness/activity_state.json`) | `core/crates/solstone-core-think-cli/src/segment.rs` (`persist_activity_state`). The state machine itself lives in `solstone-core-system::activity_state`, but that module only models the state; it never writes the file. |
| Identity (`identity/*.md`, `identity/history.jsonl` audit log) | `core/crates/solstone-core-identity/src/store.rs` |
| Day talent-output accumulator (`chronicle/<day>/talents/<name>.jsonl`) | `core/crates/solstone-core-talent-runtime/src/writers.rs` (`append_day_record`, via the closed `WriteIntent::DayAccumulator` contract; see L8) |
| Talent provenance sidecars (`chronicle/<day>/health/talent-provenance/**`) | `core/crates/solstone-core-think-cli/src/segment.rs` (`write_activity_provenance`) |
| Config (`config/journal.json`) | `core/crates/solstone-core-journal-config-write/` (`config.rs::mutate_journal_config`, `commit.rs`). `solstone-core-journal-config` (no `-write` suffix) is read/schema-only, a deliberate two-crate split rather than drift. |
| Schedules (`config/schedules.json`) | `core/crates/solstone-core-system/src/schedule/config.rs` (`mutate_schedule_entries`, `set_schedule_metadata`) |
| Push devices (`config/push-registry.json`) | `core/crates/solstone-core-push/src/store.rs` |
| Local inference operational telemetry (`health/local-inference/YYYYMMDD.jsonl`) | **No current writer — a real gap, not a doc error.** The only call site (`record_local_inference`, invoked from the Python `run_cogitate`'s cleanup path) was deleted with the rest of the Python tree, and nothing replaced it in Rust. `docs/conversion/strands.md` records an earlier disposition to retain the artifact and not restore a writer, but that disposition predates the Python deletion that actually zeroed out every writer; treat the current state as "nothing writes this file," full stop, rather than re-deriving a Rust owner. |
| Direct-door operational record (`health/direct-door.json`) | `core/crates/solstone-core-system/src/direct_door.rs` (`publish_direct_door` / `withhold_direct_door`). `solstone-core-convey-shell` and the supervisor in `solstone-core` are callers only. |
| Hosted service parent-loss coordination (`health/parent-loss/**`) | `core/crates/solstone-core-system/src/lifecycle/parent_loss_ledger.rs` owns the active pointer, sealed ledgers, and generation records; `parent_loss_admission.rs` owns isolated per-launch admission drops and per-service witness drops; `parent_loss_coordinator.rs` is the sole terminal adjudicator |
| Active-brain state (`health/brain.json`, `health/brain-fingerprint.key`, `health/brain-refresh.lease`) | `core/crates/solstone-core-brain/src/writer.rs` (the crate's own doc comment names it "the single native write authority" for `health/brain.json`) |
| Provider install status records (`health/providers/{local,parakeet}.json`) | `core/crates/solstone-core-local/src/install/status.rs` (`write_status`) |
| Provider install proof cache (`health/providers/{local,parakeet}.proof-cache.json`) | **No current writer — a real gap.** The Python `artifact_proof.py` module was deleted with no Rust replacement; nothing in `core/crates` writes this path today. |
| Provider install leases (`health/providers/{local,parakeet}.lease`) | `core/crates/solstone-core-local/src/install/lease.rs` |
| Provider runtime health and retry-token records (`health/providers/runtime/{local,parakeet}.json`, `.retry-token.json`, `.operation.lock`) | Primary: `core/crates/solstone-core-system/src/provider_runtime/store.rs` (`FileRuntimeStore`). A second call path, `core/crates/solstone-core-brain/src/runtime_health.rs` (`request_runtime_retry`), also writes `.retry-token.json` directly. This looks like two independent writers to the same file rather than a documented split; confirm intent (internal supervisor vs. user-initiated retry, or genuine duplication) before treating either as sole owner. |
| Native speakers-analyze install generation (`health/speakers-analyze/install-generation.json`, `.lock`) | `core/crates/solstone-core-transcribe/src/speakers_installation.rs` (`enter_speakers_analyze_generation`) |
| Provider artifact manifests (`cache/providers/**/.solstone-provider-manifest.json`) | `core/crates/solstone-core-local/src/install/manifest.rs` |
| nvattest appraiser cache (`cache/providers/nvattest/**`) | `core/crates/solstone-core-spp-ratls/src/nvattest_install.rs` (`ensure_nvattest_installed`). `solstone-core-spp-attest` still only locates/appraises the installed payload. |
| Media offload ledger (`health/offload/<YYYYMMDD>.jsonl`) | `core/crates/solstone-core-offload/src/ledger.rs` (`append_offload_event`) |
| Pruning-run audit (`health/pruning-runs/<YYYYMMDD>.jsonl`) | `raw_media_offload`-kind entries: `core/crates/solstone-core-offload/src/pruning_audit.rs`. The `journal_logs`-kind half that used to record chronicle-health-log pruning has **no current writer**: `core/crates/solstone-core-retention/` only deletes old rows from this file under its own retention policy; it never appends a new audit line for a log-pruning run. |
| Parakeet server placement record (`health/parakeet-cpp.placement`) | **No current writer — a real gap.** `core/crates/solstone-core-transcribe/src/backend/parakeet_cpp.rs` only reads this file (as "supervisor-published"); the placement decision itself (`solstone-core-system::provider_runtime::placement`) is computed in memory and is never persisted anywhere. |
| Hosted backup binding (`backup/hosted/binding.json`) | `core/crates/solstone-core-backup/src/hosted.rs` (`save_hosted_binding`), called from several orchestrators (`-backup-runtime`, `-backup-cli`, `-backup-web`, `-offload::restore`): one low-level writer, several legitimate callers |
| Convey config (`config/convey.json`) | `core/crates/solstone-core-convey-config/src/navigation.rs` |
| Speaker labels (`chronicle/**/talents/speaker_labels.json`) | `core/crates/solstone-core-speaker-id/src/labels.rs` (`write_full_labels`, `write_stub_labels`, `patch_labels`, `restore_label_rows`) (the lower-level crate that owns the file constant and every write function). `solstone-core-speaker-resolve` calls into it as an orchestration layer; it does not own the write itself. |
| Speaker corrections (`chronicle/**/talents/speaker_corrections.json`) | `core/crates/solstone-core-speaker-id/src/corrections.rs` (`append_correction`, same split as speaker labels above) |
| Stream identity (`chronicle/**/<seg>/stream.json` marker + `streams/<name>.json` state) | `core/crates/solstone-core-segment` (`advance_unbound_stream` / `advance_bound_stream`); observer prune repairs a survivor's predecessor pointers locally |
| Observer ingest manifest (`chronicle/**/<seg>/ingest.json`) | `core/crates/solstone-core-ingest-resolve` (`write_ingest_manifest`) |
| Link CA and client authorization state (`link/ca/cert.pem`, `link/ca/private.pem`, `link/ca-staging/**`, `link/state.json`, `link/authorized_clients.json`) | `core/crates/solstone-core-sol-link/src/establish.rs` (CA, staging, state) and `.../src/ledger.rs` (`authorized_clients.json`). `solstone-core-convey-shell` only reads via `read_authorized_clients`. |
| Link account token file (`link/tokens/account.json`) | **No current writer, by design.** `core/crates/solstone-core-spl/src/link_state_files.rs`'s own module doc states it is read-only access to the local SPL link identity/service-token files and "never creates, updates, or retains their contents." |
| Link TOTP state (`link/totp.json`) | **No current writer at all.** No crate in `core/crates` references this path; the only remaining mention anywhere is a doc citing the deleted Python auth module. Do not assume TOTP state is maintained today. |
| Native pairing nonces (`link/nonces.json`) | `core/crates/solstone-core-sol-link/src/pairing/nonces.rs` |
| MCP endpoint owner identity, durable PoP key, bearer-token verifier ledger, and OAuth ledger (`mcp-endpoint/**`, including `tokens.json` and `oauth.json`) | `core/crates/solstone-core-mcp-endpoint/` (`lib.rs::bootstrap_mcp_endpoint_owner_identity`, `unix.rs`, `tokens.rs`, `oauth/store.rs`) |
| MCP agent interaction audit records (`chronicle/**/mcp.agent/**`) | `core/crates/solstone-core-mcp-audit/` (`write_interaction_record`) |
| Chronicle day content (`chronicle/YYYYMMDD/**`) | The capturing module (observer, importer) per its declared outputs |
| Index (SQLite, `indexer/*`) | Schema, connection, and writes: `core/crates/solstone-core-indexer-store/`. What to persist (discovery, edges, entity search, metadata): computed by `core/crates/solstone-core-indexer/`, which calls into indexer-store. Query surface: `core/crates/solstone-core-indexer-query/` is read-only. |
| Observer registry and sync history (`apps/observer/observers/*.json`, `apps/observer/observers/*/hist/*.jsonl`) | **No current writer — a real, known gap.** Confirmed by explicit tests in `solstone-core-ingest` and `solstone-core-doctor` that assert this directory does not exist after ingest/cleanup. Do not attribute this to whichever crate happens to be nearby; nothing currently writes a registry or history record here. |
| Import ingest/resolve staging (`imports/**`, excluding native body state and `imports/oura.json`) | `core/crates/solstone-core-import/src/{publish.rs,staging.rs}`. `solstone-core-import-web` (web-triggered flow) and the CLI/local-sync path are orchestration callers; `solstone-core-import-sources` (format parsers) and `solstone-core-import-host` (ffmpeg adapters) feed data in without writing `imports/` paths directly. |
| Body import state (`imports/health-dedupe.sqlite`, `imports/oura.json`, Oura token fields under `config/journal.json`, `imports/_approvals/**`, native `imports/body-*` bundles with manifests, envelopes, normalized shards, ledgers, and approved raw assets) | `core/crates/solstone-core-body-{ingest,source,store,rebuild}/` via `solstone-core body <verb>` |
| Operational-log/cache pruning and root task-log compaction — deletion/compaction only, across the dated allowlist | `core/crates/solstone-core-retention/src/logs.rs` (`CLASSES`: `chronicle_health_logs`, `talent_run_logs`, `talent_day_index`, `cogitate_history_cache`, `tokens`, `local_inference`, `awareness_logs`, `config_actions`, `facet_logs`, `pruning_runs`; `COMPACTABLE`: `root_task_log`, `retention_log`), consumed by `core/crates/solstone-core-retention-cli/`. **This table has no row for `apps/observer/observers/*/hist/`** — consistent with that path having no writer at all right now (nothing to prune), but a fact worth knowing before anyone restores a writer there: it will not be pruned until this class table is updated too. |

If you're about to write to a domain from a module not in this table, stop and route through the owner. If your change means a row above is now wrong, fix the row in the same commit — don't leave the next reader to rediscover it the way this rebuild had to.

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

```rust
let entity = match load_entity(eid)? {
    Some(existing) => existing,
    None => create_entity(eid, /* ... */)?,
};
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

Importers write source material to `imports/` and the raw-content areas of `chronicle/`. They may not create or modify entities, facets, observations, or other cross-cutting state. If an importer needs to create an entity for deduplication, it calls a domain-owned `seed_entity()`-equivalent in `core/crates/solstone-core-entity/` that surfaces the write explicitly.

### L8 — Hooks have declared outputs

Talent hook stages declare every write they can perform, and the runtime enforces this **at compile time**, not by convention. `core/crates/solstone-core-talent-runtime/src/contract.rs` defines a closed `StageId` enum and a `CommitFn` type whose own doc comment states the design directly: "Closed, static hook-stage contract. The runtime is not a plugin host." Every possible write is a variant of the `WriteIntent` enum in `core/crates/solstone-core-talent-runtime/src/writers.rs` (`DayAccumulator`, `Story`, `DailySchedule`, `Participation`, `Schedule`, `FacetNewsletter`, `EntityDetection`, `EntitiesReview`, and the rest); a write that isn't modeled as a `WriteIntent` variant cannot be committed, because there is no code path that would accept it. This is a stronger version of the old Python contract (frontmatter declared paths, a runtime hook validated them at runtime): the write surface is closed and enumerated in source, checked by the compiler, rather than declared in prose and checked after the fact.

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
- **Fail loudly, not silently.** Raise specific errors with clear messages; log through `log`, not ad hoc printing — see `docs/LOGGING.md` for the output-vs-diagnostic test, level definitions, and why a binary needs its own logger installed before a `log::` call does anything. Validate inputs at module boundaries. A silent swallow in production costs days of forensics — an error at the boundary is free.
- **Trust internal code.** Don't add defensive validation for things internal callers can't violate. Validate at system boundaries (user input, external APIs, imported files) — not between modules you control.

Generic software principles (DRY, KISS, YAGNI, single responsibility, small focused commits) apply; see `docs/coding-standards.md` for the full list.

## 9. File headers, naming, dependencies

- **SPDX header** as above — mandatory on source code files.
- **Naming:** modules / functions / variables `snake_case`; types `PascalCase`; constants `UPPER_SNAKE_CASE`. Full table in `docs/coding-standards.md`.
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
| `docs/CONVEY-FRONTEND.md` | Binding client-side conventions for any Convey workspace, shell chrome, or shared client helper — static shell + per-app workspace architecture, the `/api/shell` contract |
| `docs/OBSERVE.md` | Capture-side work: new modalities, transcription, sensing |
| `docs/SOLCLI.md` | Adding a new `solstone <cmd>` or `solstone call <app> <verb>` |
| `docs/PORTING.md` | Rust workspace rules: edition, iOS canary, native-dep proof |
| `docs/conversion/README.md` | The architectural map (plates, strands, cables) underlying the Rust workspace — read for "why is it shaped this way," not "how do I port X" (there's nothing left to port) |
| `docs/PROMPT_TEMPLATES.md` | Modifying talent prompt format or frontmatter |
| `docs/PROVIDERS.md` | The provider architecture: one active-brain resolver, four dispatch lanes (three cloud vendors plus local, where local also covers arbitrary OpenAI-compatible endpoints and confidential processing), and honest no-fallback failure semantics |
| `docs/testing.md` | Test structure, fixtures, debugging test isolation |
| `docs/environment.md` | Journal path resolution, managed-wrapper behavior, service install details, and `SOLSTONE_JOURNAL` rules |
| `docs/CHANNEL_ADAPTERS.md` | Release channel adapter config, scrub-gate expectations, and operator-safe placeholders |
| `docs/release-evidence-contract.md` | **Required before changing the retained release ledger schema registry** — why such a change breaks already-cut candidates, what `schema_version` does and does not tolerate here, and the frozen-fixture rule |
| `docs/journal-format-contract-maintenance.md` | Changing a committed journal at-rest format (observer ingest envelopes, `stream.json`, `audio.jsonl`, `screen.jsonl`) — schema floor vs producer-local requirements, and which relaxations are safe |
| `docs/JOURNAL_FILESYSTEM_CONTRACT.md` | The shared vocabulary for a journal root, its identity, entry kinds, and refusals — not a generic VFS |
| `docs/coding-standards.md` | Full naming conventions, ruff config, dep-management details — reference for everything not promoted into this file |
| `docs/project-structure.md` | Canonical directory layout; resolving "where does this file go" debates |
| `docs/LOGGING.md` | Choosing between `println!`/`eprintln!` and `log::`, picking a level, or adding a new diagnostic — the output-vs-diagnostic test, level definitions, and why a binary needs its own logger installed before a `log::` call site does anything |
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
