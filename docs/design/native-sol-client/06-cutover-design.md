# Native Sol Cutover Design

This design records the native-`sol` distribution cutover. It assumes the prep
preconditions passed and the cutover version is the next root package version.

## 1. Script Ownership Model

Superseded for 1.0.15: the root `solstone` wheel owns the public POSIX
`sol`/`solstone` launchers as raw script-file payloads, and the
`solstone-core` wheel owns only the sibling compiled `solstone-core` executable
they exec. The private `solstone-python-compat` console script was removed; the
native compatibility bridge now invokes `python -P -m
solstone.think.sol_compat_cli` from beside `solstone-core`.

Decision: the `solstone-core` wheel owns both public access scripts, `sol` and
`solstone`, as native `*.data/scripts/` members. The root `solstone` package no
longer owns either public script.

Root packaging changes:

- Remove the root `[project.scripts]` entries for `sol` and `solstone` from
  `pyproject.toml`.
- Add a private root console script named `solstone-python-compat`, backed by a
  new `solstone.think.sol_compat_cli` module. This is not a public entry point;
  it exists only for native `sol` to exec the finite compatibility set.
- Move the existing `solstone-core==<version>` marker pins out of the
  `[journal-host]` extra into root base dependencies, derived from
  `solstone.think.probe.SOLSTONE_CORE_COVERED_PLATFORMS`.
- Add one complement-marker tombstone dependency for unsupported platforms:
  distribution name `solstone-core-unsupported-platform`, source directory
  `scripts/solstone-core-unsupported-platform-tombstone/`. It follows
  `scripts/journal-host-tombstone/`: build fails unless an allow-build env var
  is set, and the error states that native `sol` requires a supported
  `solstone-core` wheel, lists the supported platform triples, and says that
  nominal install success without `sol` is impossible.

Maturin/Cargo decision: choose option (a). Convert `solstone-core-sol` into a
library crate with a reusable process-shell entry function, then add two thin
`[[bin]]` targets to the `solstone-core` package:

- `[[bin]] name = "sol"` in `core/crates/solstone-core/Cargo.toml`
- `[[bin]] name = "solstone"` in `core/crates/solstone-core/Cargo.toml`

This keeps `packages/solstone-core/pyproject.toml` pointed at
`../../core/crates/solstone-core/Cargo.toml`. Repointing maturin at the
workspace would make package selection and wheel script ownership less explicit,
and the current leaf package is already the release authority for the helper
binary.

`sol` and `solstone` are two distinct native bin artifacts, not one artifact
plus a wheel-time copy. Both bins call the same `solstone-core-sol` library
function. The library treats them identically; any future `argv[0]`-based text
must be explicit and parity-tested so acceptance criterion 6 remains true.

Consequences:

- `core/crates/solstone-core/Cargo.toml` adds bin targets and dependencies on
  `solstone-core-sol`, plus any transitive deps needed only by the thin bins.
- `core/crates/solstone-core-sol/Cargo.toml` gains the implicit library target
  from `src/lib.rs`. Its current process-shell logic moves from `src/main.rs`
  into `src/lib.rs`, and `src/main.rs` is deleted so no duplicate `sol` wrapper
  remains.
- `scripts/normalize_maturin_sdist.py` must stop pruning newly reachable
  workspace members from the core sdist lock graph.
- `scripts/check_wheel_contents.py::CORE_REQUIRED_SDIST_MEMBERS` must include
  the new core bin files, `solstone-core-sol`, `solstone-core-sol-client`,
  `solstone-core-sol-client-cli`, and every generated `#[path]` source required
  by `core/crates/solstone-core-sol-client/src/generated/inventory.rs`.
- The 19 app-local/native `command.rs` files currently included by
  `inventory.rs` must be present in the sdist build context. The adjacent
  `authority.toml` files should also be included so the sdist remains
  reconstructible and auditable, even though Cargo builds from generated Rust.

## 2. Native `sol import` Authority

Add a top-level native authority beside chat:

- `core/native-sol/think/native/import/authority.toml`
- `solstone/think/native/import/command.rs`

Authority shape:

- `surface = "sol-import"`
- `path = ["import"]`
- `kind = "top-level"`
- `entry_type = "top-level-import"`
- `operation_id = "import.top_level"`
- `handler = "import_top_level"`
- `backing_contract_operation_ids = ["import.save", "import.savePath", "import.start"]`
- params mirror `solstone/think/import_client.py::MODE_DISPOSITIONS`: positional
  media, hidden extra args, `--timestamp`, `--facet`, `--setting`, `--source`,
  `--force`, `--auto`, `--deterministic-only`, `--dry-run`, `--json`,
  `-v/--verbose`, `--backends`, `--sync`, `--save`, `--path`,
  `--list-importers`, and the `journal-source` relocation case.

The new `top-level-import` entry type must be accepted by:

- `scripts/build_native_sol_inventory.py`: `ENTRY_TYPES`, constants, partition
  reporting, generated inventory output, and `--check`.
- `scripts/check_native_sol_coverage.py`: top-level import is not part of the
  152 `sol-call` HTTP leaf count, but its vectors still need request-binding,
  success, duplicate, rejection, and boundary-failure coverage.
- `scripts/check_native_sol_architecture.py`: import command source is an
  authority source and must obey app-local native ownership rules.
- `scripts/check_native_sol_conformance.py`: top-level import must be joined to
  its backing OpenAPI operations and Flask routes.
- `core/crates/solstone-core-sol-client/src/generated/inventory.rs`: generated
  entry and handler binding.
- `core/crates/solstone-core-sol-client/src/aggregate.rs` and
  `core/crates/solstone-core-sol-client-cli/src/lib.rs`: longest-path dispatch
  and `Outcome` classification.
- `core/crates/solstone-core-sol/src/lib.rs`: `argv` routing for `import ...`.

Native import request sequence:

- If the media argument resolves through the file seam as an existing file,
  first request is multipart `POST /app/import/api/save` with form fields
  `client_item_id`, optional `facet`, optional `setting`, optional
  `source_hint`, optional `deterministic_only = "true"`, and file field
  `file = (filename, bytes, application/octet-stream)`.
- If the media argument is not an existing file, first request is JSON
  `POST /app/import/api/save-path` with `client_item_id`, optional metadata
  fields, optional `deterministic_only = "true"`, and `path`.
- If the save response is duplicate or `recommended_action = "do_not_start"`,
  stop after the first request and render duplicate output.
- Otherwise second request is JSON `POST /app/import/api/start` with `path`,
  `timestamp` from `--timestamp` or the save response, and `force`.

Frozen output strings:

- `sol import: journal-source management moved to `sol call import <verb>`.`
- `sol import: `--dry-run` requires the journal host. Run this on the journal host with `journal importer`.`
- `sol import: `--backends` requires the journal host. Run this on the journal host with `journal importer`.`
- `sol import: `--list-importers` requires the journal host. Run this on the journal host with `journal importer`.`
- `sol import: `--sync` requires the journal host. Run this on the journal host with `journal importer`.`
- `sol import: `--save` requires the journal host. Run this on the journal host with `journal importer`.`
- `sol import: `--path` requires the journal host. Run this on the journal host with `journal importer`.`
- `sol import: `--auto <guidance>` requires the journal host. Use `--timestamp` here or run `journal importer`.`
- `sol import: couldn't reach the journal. Start it with 'journal up' and retry.`
- `sol import: couldn't read journal response`
- `sol import: failed to stage import: {error}`
- `sol import: {detail}`
- `sol import: staged {staged_path} but processing was not queued: couldn't reach the journal`
- `sol import: staged {staged_path} but processing was not queued: couldn't read journal response`
- `sol import: staged {staged_path} but processing was not queued: {error}`
- `staged {path}`
- `timestamp {timestamp}`
- `queued processing task {task_id}`
- `queued processing`
- `sol import: duplicate import; skipping`
- `sol import: already imported on {imported_at}{entries}; skipping`
- `sol import: already staged as {import_id}; skipping`

`--json` output remains compact JSON with sorted keys. Duplicate `--json`
outputs the save response unchanged.

`client_item_id` is generated by a new native seam,
`ClientItemIdProvider`. The real provider emits uuid4 hex. Parity tests inject a
fixed provider so multipart/form payloads are deterministic.

## 3. Private Compatibility Entry

Private entry:

- Script name: `solstone-python-compat`
- Distribution: base `solstone` wheel
- Module: `solstone.think.sol_compat_cli`

Closed allowlist:

- top-level: `notify`, `doctor`, `check`, `contract`, `skills`, `link`
- journal group: exactly the 23 frozen `sol call journal ...` paths

Do not duplicate the 23 journal paths. Runtime dispatch may route only the
`sol call journal` subtree directly to `solstone.think.tools.call`. A new static
gate derives the actual Typer leaves from `solstone.think.tools.call.app` and
compares them to the frozen oracle remainder computed by
`scripts.build_native_sol_inventory.check_complete_partition()`. If the Typer
subtree grows or shrinks, the gate fails.

Native lookup follows the existing sibling pattern from
`solstone/think/core_handshake.py`: from the native executable path, replace the
filename with `solstone-python-compat`. Missing or non-executable sibling is an
install-coherence error:

- stderr: `sol: native compatibility helper is missing or not executable: {path}. Reinstall solstone and solstone-core.`
- exit: `78`

Recursion prevention uses two independent mechanisms:

- The compat entry dispatches directly to target modules and never invokes
  `sol`, `solstone`, or `solstone-python-compat`.
- Env sentinel `SOLSTONE_NATIVE_COMPAT_ACTIVE` has states. Native refuses to
  delegate if the variable is already present, then sets it to `armed` before
  exec. The compat entry accepts only `armed`, changes it to `active`, and
  refuses `active`, missing, or any other value with stderr
  `sol: compatibility dispatch recursion detected. Reinstall solstone and solstone-core.`
  and exit `70`.

Forwarding contract:

- Native uses exec replacement, not spawn-and-wait.
- `argv` is forwarded with leading internal marker
  `__solstone_native_argv0=<sol|solstone>`, followed by the original public
  arguments. The compat entry accepts and strips exactly that marker, then
  rebuilds downstream `sys.argv` as `sol <cmd> ...`, `solstone <cmd> ...`, or
  `sol call journal ...` / `solstone call journal ...` so target modules retain
  the public argv identity.
- stdin, stdout, stderr, cwd, and environment are inherited unchanged except for
  the sentinel.
- Exit status and Unix signal behavior are those of the compat process. Exec is
  chosen because it gives the best signal fidelity on the supported platforms.

Gate reconciliation:

- The exec lives in `core/crates/solstone-core-sol/src/lib.rs` or the retained
  process-shell wrapper, both outside
  `scripts/check_native_sol_no_python_spawn.py`'s current scan set.
- This is deliberate. The no-spawn gate protects authority sources plus the
  shared native client/client-cli crates. The process shell already owns real
  process seams such as build identity and is the correct boundary for the
  private compatibility exec.
- The no-spawn `ALLOWLIST` stays empty and no forbidden pattern is removed.
- Add a positive shell-boundary check instead: the only native compatibility
  exec reference may be in `solstone-core-sol`, and no authority source or
  shared client crate may mention the private helper name.

## 4. Gate Replacement Plan

Grammar oracle:

- Retire `scripts/build_native_sol_grammar_oracle.py` as a Python `call_app`
  generator.
- Replacement: `scripts/build_native_sol_authority_grammar.py`, derived only
  from `core/native-sol/**/native/**/authority.toml`, emits the native grammar
  projection.
- Non-vacuity: it must fail if zero authorities are discovered, if the generated
  `sol-call` projection is empty, or if it does not reconcile with the frozen
  `core/fixtures/native-sol/sol-call-grammar-v1.json` for all non-journal
  paths.

Python parity:

- Delete `tests/native_sol/run_python_parity.py` and
  `tests/native_sol/test_python_parity.py`.
- Replacement assurance is the committed native parity corpus plus Rust parity
  harness, coverage gate, conformance gate, and frozen pre-cutover Python
  manifest. After deletion, Python is historical evidence, not a live oracle.
- Non-vacuity: Rust parity must fail if no vectors load, if no vector maps to a
  native authority, or if any HTTP/top-level-import authority lacks success,
  request-binding, and failure coverage as applicable.

Conformance:

- Survives: Flask route discovery, OpenAPI operation discovery, method/path
  join, backing contract operation IDs, and reason-code equality.
- Dies: any dependency on Python adapter grammar or Python `call_app`.
- Native replacement asserts every HTTP authority and top-level import backing
  operation has a Flask route and OpenAPI operation; every native authority
  handler exists in generated inventory; and native import covers
  `import.save`, `import.savePath`, and `import.start`.
- Non-vacuity: fail if no authorities, no Flask routes, no OpenAPI operations,
  or no top-level import entry are found.

Python manifest:

- Pre-cutover manifest must cover all 21 deletion-owner files: 14 app
  `call.py` files, `ledger.py`, `profile.py`, `health.py`, `import_client.py`,
  `chat_cli.py`, and `sol_cli.py`.
- It must not list `convey_client.py`, which survives for link and SPL.
- Commit the corrected manifest before deletion.
- Post-deletion, the gate becomes both historical record and
  deletion-completeness assertion: the recorded blobs must match the pre-cutover
  commit, and the files expected to be deleted must be absent from the product
  tree except for `sol_cli.py`, where the check asserts the sol-surface dispatch
  blocks are gone and `journal_main()` still exists.

Access import-clean:

New case table:

- native public basics: `sol`, `sol --help`, `sol --version`, `sol --path`,
  `sol path`, `sol root`
- native top-level migrated: `sol chat --help`, `sol import --help`
- native aggregate: `sol call --help`
- compat top-level: `sol notify --help`, `sol doctor --help`,
  `sol check --help`, `sol contract --help`, `sol skills --help`,
  `sol link --help`
- compat journal subtree: `sol call journal --help`,
  `sol call journal search --help`
- journal host: `journal transcribe --help`
- routing errors: invoking the native binary with `think --help` must produce
  the unsupported-command error; `journal import --help` must still produce the journal-access
  rejection

`--real-install` becomes the authority. It installs the root package into a temp
venv, asserts `sol` and `solstone` are native scripts from `solstone-core`,
asserts `solstone-python-compat` exists, then runs the case table as
subprocesses. The simulated import-blocking child remains only for direct compat
and `journal_main()` Python cases.

## 5. Source-Checkout Behavior

After `uv sync`, `.venv/bin/sol` and `.venv/bin/solstone` are native
`solstone-core` scripts. The base root install in the same venv also provides
`.venv/bin/solstone-python-compat`.

The Makefile source-checkout uses continue to work:

- `Makefile:181` `sol skills build`: native `sol` routes `skills` to compat.
- `Makefile:182` `sol skills install --project journal --agent all`: same
  compat route.
- `Makefile:311` executable precondition: still checks `.venv/bin/sol`, now a
  native binary.
- `Makefile:739` `sol skills build --check`: native-to-compat route.

Ordering constraint: `make install` must run `uv sync` before invoking
`.venv/bin/sol`, because the native script and the private compat script are
installed by different distributions. A partial venv where native `sol` exists
without `solstone-python-compat` must fail with the explicit install-coherence
error from section 3.

## 6. Wrapper Impact

Do not bump `solstone/think/install_guard.py::WRAPPER_VERSION` if the wrapper
template remains byte-identical.

The wrapper still does `exec "$SOL_BIN" "$@"`, and `SOL_BIN` still points at
`.venv/bin/{sol,journal}`. Changing the target implementation from a Python
console script to a native binary does not change wrapper semantics. A version
bump would be gratuitous unless the rendered text changes.

The stale warning string remains unchanged:

`{binary}: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install`

The warning still fires because `REPO_ROOT="${SOL_BIN%/.venv/bin/{binary}}"`
depends only on the path shape, not the file contents.

## 7. Version And Docs

Version move to 1.0.14:

- Covered by `scripts/render_packaging.py`: root `pyproject.toml`, leaf
  pyprojects, root marker pins, Cargo workspace version, Cargo.lock workspace
  member versions, journal leaf pins.
- Must be added to render/check authority: root base `solstone-core` pins,
  complement tombstone pin, private compat script presence, and removal of
  public root `sol`/`solstone` scripts.
- Not covered and should stay historical: `transparency-head-log.jsonl` existing
  prior-version entry. The new transparency entry is appended only by the
  transparency publication flow.
- Test fixtures must derive expected version from one authority, normally root
  `[project].version`, rather than hardcoding `1.0.14` across release and
  packaging tests.

Docs:

- Add the finite compatibility inventory and zero-delegation removal criterion
  to `docs/PORTING.md` under Dual Paths And Shims.
- State the inventory once by referencing the compat inventory source/gate, not
  by duplicating the command list in prose.
- Removal criterion: native `sol` has zero Python delegation; the private compat
  script and all compatibility entries are removed; `check_native_sol_no_python_spawn`
  and the positive shell-boundary gate pass with no compat helper references.
- Add one PORTING link block following the existing
  `docs/design/indexer-native-atomicity.md` pattern, linking all native-sol
  design records:
  `00-prep-findings.md`, `01-oracle-repro.md`, `02-design.md`,
  `03-batch-prep.md`, `04-batch-design.md`, `05-raw-body-parity.md`, and this
  `06-cutover-design.md`.
- Add a static docs check that scans `docs/design/native-sol-client/*.md` and
  fails if any file is not linked from `docs/PORTING.md`. It must fail on an
  empty design directory or empty link set.

## 8. `timefhuman` Removal

Removal set:

- Delete `parse_time_range()` from `solstone/think/utils.py`.
- Remove `from timefhuman import timefhuman` from `solstone/think/utils.py`.
- Delete `tests/test_parse_time_range.py`.
- Remove `timefhuman` from root `pyproject.toml` base dependencies.
- Remove `timefhuman` from `scripts/check_extras_consistency.py::THIN_BASE`.
- Update `solstone/think/probe.py` comment explaining why probe inlines
  `is_source_checkout`; after this removal, importing `utils` no longer pulls
  `timefhuman`, but the inline helper can remain to avoid broader thin-base
  imports.
- Update `tests/test_preflight.py` blocked-family probes at the current
  `timefhuman` assertions to use a still-valid blocked package name from the
  host-only set, for example `frontmatter` or `flask`, depending on the specific
  assertion.

Acceptance criterion 19 needs a fixture test for the removal guard:

- Add a small static checker that scans for production/doc/export/generated
  callers of `parse_time_range` and `timefhuman` outside the known removal set.
- The fixture test creates a synthetic production caller and asserts the checker
  returns `RETAIN` with a blocker message naming the caller. That prevents the
  change from deleting the dependency if a new caller appears between design and
  implementation.

## File-Level Change List

### Commit 1: Script Ownership And Packaging

Add:

- `scripts/solstone-core-unsupported-platform-tombstone/README.md`
- `scripts/solstone-core-unsupported-platform-tombstone/setup.py`
- `core/crates/solstone-core-sol/src/lib.rs`
- `core/crates/solstone-core/src/bin/sol.rs`
- `core/crates/solstone-core/src/bin/solstone.rs`

Modify:

- `pyproject.toml`
- `uv.lock`
- `packages/solstone-core/pyproject.toml`
- `packages/solstone-journal/pyproject.toml`
- `packages/solstone-journal-cuda/pyproject.toml`
- `core/Cargo.toml`
- `core/Cargo.lock`
- `core/crates/solstone-core/Cargo.toml`
- `core/crates/solstone-core-sol/Cargo.toml`
- `core/crates/solstone-core-sol/src/main.rs`
- `scripts/render_packaging.py`
- `scripts/normalize_maturin_sdist.py`
- `scripts/check_wheel_contents.py`
- `scripts/check_rust_release_manifest.py`
- `tests/test_normalize_maturin_sdist.py`
- `tests/test_release_candidate_driver.py`
- `tests/test_release_install_smoke.py`
- `tests/test_release_native_records.py`
- `tests/integration/test_solstone_core_wheel_install.py`
- `tests/helpers/release_wheel_fixtures.py`

### Commit 2: Native Top-Level Import

Add:

- `core/native-sol/think/native/import/authority.toml`
- `solstone/think/native/import/command.rs`
- native import parity vectors under `core/fixtures/native-sol/parity/`

Modify:

- `scripts/build_native_sol_inventory.py`
- `scripts/check_native_sol_coverage.py`
- `scripts/check_native_sol_architecture.py`
- `scripts/check_native_sol_conformance.py`
- `scripts/check_native_sol_contract_routes.py`
- `core/crates/solstone-core-sol-client/src/aggregate.rs`
- `core/crates/solstone-core-sol-client/src/seam.rs`
- `core/crates/solstone-core-sol-client/src/generated/inventory.rs`
- `core/crates/solstone-core-sol-client-cli/src/lib.rs`
- `core/crates/solstone-core-sol-client-cli/src/bin/resolve_parity_leaves.rs`
- `core/crates/solstone-core-sol-client-cli/tests/parity.rs`
- `core/fixtures/native-sol/applicability.json`
- `tests/native_sol/test_parity_coverage.py`
- `tests/test_native_sol_inventory.py`
- `tests/test_native_sol_conformance.py`
- `tests/test_import_client.py` or its native successor

### Commit 3: Private Compatibility Entry

Add:

- `solstone/think/sol_compat_cli.py`
- `solstone/think/sol_compat_inventory.py`
- `scripts/check_native_sol_compat.py`
- `tests/test_sol_compat_cli.py`

Modify:

- `core/crates/solstone-core-sol/src/lib.rs`
- `core/crates/solstone-core-sol/src/main.rs`
- `scripts/check_native_sol_no_python_spawn.py`
- `scripts/check_access_imports_clean.py`
- `tests/test_access_imports_lazy.py`
- `tests/test_sol_cli_help.py`
- `tests/test_sol_service_hard_error.py`
- `tests/test_service.py`
- `tests/spl/test_service.py`
- `solstone/think/sol_cli.py`
- `solstone/think/call.py`
- `Makefile`

### Commit 4: Gate Replacement And Historical Manifest

Add:

- `scripts/build_native_sol_authority_grammar.py`
- `scripts/check_native_sol_deleted_python_manifest.py`
- `scripts/check_native_sol_docs.py`
- corrected pre-cutover manifest fixture if the digest gate is moved out of
  inline constants

Modify:

- `scripts/build_native_sol_grammar_oracle.py`
- `scripts/check_native_sol_grammar_oracle.py`
- `scripts/check_native_sol_python_manifest.py`
- `scripts/check_native_sol_conformance.py`
- `scripts/check_native_sol_coverage.py`
- `scripts/check_native_sol_architecture.py`
- `Makefile`
- `tests/native_sol/test_parity_coverage.py`
- `tests/test_native_sol_inventory.py`
- `tests/test_native_sol_conformance.py`

Delete:

- `tests/native_sol/run_python_parity.py`
- `tests/native_sol/test_python_parity.py`

### Commit 5: Delete Migrated Python Owners

Delete:

- `solstone/apps/activities/call.py`
- `solstone/apps/awareness/call.py`
- `solstone/apps/chat/call.py`
- `solstone/apps/entities/call.py`
- `solstone/apps/facets/call.py`
- `solstone/apps/import/call.py`
- `solstone/apps/network/call.py`
- `solstone/apps/settings/call.py`
- `solstone/apps/sol/call.py`
- `solstone/apps/speakers/call.py`
- `solstone/apps/support/call.py`
- `solstone/apps/thinking/call.py`
- `solstone/apps/transcripts/call.py`
- `solstone/think/tools/health.py`
- `solstone/think/tools/ledger.py`
- `solstone/think/tools/profile.py`
- `solstone/think/import_client.py`
- `solstone/think/chat_cli.py`

Modify:

- `solstone/think/call.py`
- `solstone/think/sol_cli.py`
- `tests/test_chat_cli.py`
- `tests/test_import_client.py`
- all app/tool call parity tests that import deleted modules, including
  `tests/test_activities_call_parity.py`, `tests/test_awareness_call_parity.py`,
  `tests/test_brain_health_cutover_parity.py`,
  `tests/test_entities_call_parity.py`, `tests/test_health_call_parity.py`,
  `tests/test_ledger_call_parity.py`, `tests/test_link_call_parity.py`,
  `tests/test_profile_call_parity.py`, `tests/test_settings_call_parity.py`,
  `tests/test_sol_call_parity.py`, `tests/test_speakers_call_parity.py`,
  `tests/test_thinking_call_parity.py`, and app-local `solstone/apps/*/tests/`
  call suites.

### Commit 6: Time Parser, Version, And Docs Cleanup

Add:

- `tests/test_removed_time_parser_guard.py`
- `scripts/check_removed_time_parser_ready.py`
- `docs/design/native-sol-client/06-cutover-design.md`

Modify:

- `solstone/think/utils.py`
- `pyproject.toml`
- `uv.lock`
- `scripts/check_extras_consistency.py`
- `solstone/think/probe.py`
- `tests/test_preflight.py`
- `docs/PORTING.md`
- `CHANGELOG.md`
- version-derived release/package tests listed in commit 1 where the prior
  version appears

Delete:

- `tests/test_parse_time_range.py`

## Risks And Open Questions

- The private compatibility entry is intentionally a temporary escape hatch.
  The removal criterion must stay tied to a single compatibility inventory gate
  or the command list will drift.
- Maturin multiple-bin behavior should be verified during implementation with
  wheel contents, because the plan relies on `bindings = "bin"` packaging every
  bin target of the selected `solstone-core` package.
- The unsupported-platform tombstone must be published before root base
  dependencies point at it, or unsupported installs will fail with a resolver
  missing-distribution error instead of the intended build-time message.
- Native import adds uuid generation and file upload behavior to the top-level
  native surface. The deterministic ID seam and multipart parity vectors are the
  main guard against subtle request drift.
- `sol_cli.py` cannot be deleted wholesale until `journal_main()` is split or
  retained; the deletion is only the Python `sol` access surface and root HTTP
  dispatch.
