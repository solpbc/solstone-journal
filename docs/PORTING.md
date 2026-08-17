# Python to Rust Porting Doctrine

This document is for engineers and coding agents porting solstone behavior from
Python into the Rust workspace under `core/`. It records the workspace rules before
any behavior moves.

**The map is separate from the doctrine.** This document is *how* to port. The
boundaries themselves — every plate, every strand, which end owns each contract,
and what must be carried forward out of the Python — are defined in
[`conversion/`](conversion/README.md). Read that for what and where; read this
for how.

## Workspace Scope

The Rust workspace lives at `core/`. It contains a thin `solstone-core` bin,
the `solstone-core-cli` adapter library, and subsystem crates such as
`solstone-core-journal` as Python behavior is ported.

Rust crates use edition 2024, `rust-version = "1.95"`, and
`license = "AGPL-3.0-only"` inherited from `core/Cargo.toml`. Every `.rs` file
starts with the two-line `//` SPDX header used by `AGENTS.md`.

## Mobile Readiness

Rust subsystem logic should stay eligible for the iOS canary unless a host-only
adapter makes that impossible. The native markdown indexer keeps discovery,
metadata, segment parsing, stream-marker reads, and markdown chunking in
`solstone-core-indexer`, which remains covered by `check-rust-ios`.
`solstone-core-indexer-store` is excluded because its bundled-C SQLite build
cannot cross-compile from the Linux host. That exclusion is for the storage
adapter, not for the indexer logic. The eventual iOS path is to link the system
`libsqlite3` that iOS ships instead of bundling SQLite, then return the store
crate to the iOS gate.
`solstone-core-indexer-query` is likewise excluded now that its read-only
execution path has a non-dev bundled-C SQLite dependency; it can return when
the iOS path links the system `libsqlite3`.

`solstone-core-speakers` stays in the iOS canary because its DSP and discovery
clustering graph remains Rust-only; the `hdbscan`/`kdtree` clustering crates add
no C/C++ build steps or native linkage.
`solstone-core-speakers-analyze` and `solstone-core-speakers-onnx` are excluded:
the analyzer transitively depends on the ONNX Runtime host native-runtime
adapter, which is not mobile-ready subsystem logic.

`solstone-core-sol-link` is excluded permanently by product shape, not deferred
iOS debt. `sol link` is a desktop and linked-system surface; phones do not
consume it, and iOS/watchOS pairing lives in `spl-swift`, a separate package
with its own release rail. This matches the program's standing priority:
desktop-first is the product goal, mobile-runtime constraints are explicitly
not product requirements here, and the iOS canary is engineering insurance
rather than a product gate. The split is still useful to mobile consumers:
`spl-core` keeps pure pair-link parsing and CA logic iOS-eligible and
cross-checkable without a platform toolchain, while only `spl-transport` needs
the real host toolchain for `ring`'s C build.

`solstone-core-convey-http` is likewise excluded permanently by product shape,
not deferred iOS debt. It is the substrate for the journal-host `convey` web
service: the machine hosting the journal runs the server, while phones and
other devices consume it as HTTP clients over the network. Its currently inert
library happens to compile for iOS, but that is not a product requirement and
future TLS and loopback-binding work is host-specific; retaining it in the
iOS canary would confuse incidental portability with the supported deployment
shape.

`solstone-core-settings-web` is also excluded from the iOS canary. It is the
host-side Settings HTTP adapter: phones are clients, never hosts. Its router is
merged into Convey before the shell extensions and `session_gate` route layer,
and the exported router carries no fallback so it can compose with the other
native web lanes. Settings reads the pure category registry from
`solstone-core-describe-categories`; the describe crate re-exports that leaf so
the category API stays stable without carrying FFmpeg into a web settings read.

## Native Dependency Release Proof

A Rust conversion that adds or bumps a dependency with C/C++ build steps or
native linkage is not complete after source checks alone. Before the conversion
closes, prove the supported release targets still build and pass artifact
validation: Linux x86_64 musl, Linux aarch64 musl, and macOS arm64. Keep
required toolchain, target, and linker behavior in checked-in repository release
paths, not in a local shell profile. If a dependency cannot satisfy a supported
target, document the blocker and stop the conversion before merging it.

The ONNX Runtime speaker wrapper is a deliberate exception because it is outside
the `solstone-core` shipping closure. The `solstone-core` shipping bin depends
on no speaker ONNX crate, and `scripts/core_compile_inputs.py` walks only that
shipping closure. `solstone-core-speakers-analyze` is a helper binary outside
that core binary closure but reachable through the journal leaves, matching the
separate-native-binary shape described below. That unreachability is
load-bearing, not a convenience: reachability from
the shipping bin is an open architecture question, currently believed
unsatisfiable as-is. The Linux release lanes are
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` via zig, i.e. static
musl. The prebuilt ONNX Runtime wheels are glibc-only native libraries; the Linux
1.25.0 wheel requires GLIBC symbols up to `GLIBC_2.27` and links `libstdc++` and
`libgcc_s`. A static-musl binary cannot link or `dlopen` that glibc shared
library, so the three-target native dependency proof would fail rather than
merely remain pending.

`solstone-core-vulkan-probe` follows the same separately packaged helper shape
on the two Linux glibc lanes. It owns the host Vulkan `dlopen` boundary rather
than adding that dependency to the static-musl `solstone-core` binary or to the
ONNX-linked speaker helper.

The shipping shape for the speaker analyzer is settled: ship
`solstone-core-speakers-analyze` as a separate platform wheel on its own glibc
and macOS lanes, carrying the pinned CPU ONNX Runtime shared library that the
helper dynamically links. Do not design toward linking
`solstone-core-speakers-onnx` into the `solstone-core` bin.

The musl-to-glibc substitution is local to that helper wheel. The `solstone-core`
wheel stays on the existing static-musl Linux lanes and must remain byte-identical
to the core wheel that would be produced without the speaker analyzer helper.
The helper's Linux lanes use zig GNU targets and declare `manylinux_2_27`
because prep measured both the CPU ONNX Runtime library and zig-built helper
binary at `GLIBC_2.27`. Prep also measured a host GNU cargo-built helper binary
at `GLIBC_2.34`; that measured regression is why host GNU builds are forbidden
for the helper release lanes.

| Helper coverage | Status | Evidence |
|-----------------|--------|----------|
| `solstone/think/probe.py:SOLSTONE_CORE_SPEAKERS_ANALYZE_COVERED_PLATFORMS` | Platform coverage authority. | Build, content check, install into a bare venv, and real-inference smoke using the shipped `pyannote-segmentation-3.0.onnx` and `wespeaker-resnet34-256.onnx` assets for each covered helper platform. Linux helper lanes use zig GNU cross-link artifacts; macOS evidence is produced by the macOS build/proof hosts. Do not provision an emulator for the aarch64 Linux lane. |
| `solstone-core-pdf` (`sol-pdf/1`) | Separately packaged inspect/extract helper; it runtime-`dlopen`s its bundled PDFium shared library rather than build-time-linking an ONNX-style runtime. | `solstone-distribution acquire pdfium` pins and verifies PDFium `chromium/7920` (archive digest plus GitHub attestation) and carries its full notice bundle. |

| Evidence | Repository command | Class | Notes |
|----------|--------------------|-------|-------|
| Rust formatting | `make check-rust-fmt` | GNU-host check | Host source-format evidence only. |
| Rust MSRV | `make check-rust-msrv` | GNU-host check | Verifies the pinned MSRV rail without changing `rust-version`; excludes the three host-native helper packages from host coverage and invokes no Python. |
| Rust routine lint | `make check-rust-clippy` | GNU-host check | Runs library/binary Clippy with `-D warnings`; excludes the host-native helper packages and invokes no Python. |
| Rust full lint | `make check-rust-clippy-full` | GNU-host check | Compiles every ordinary target, including Cargo integration targets, with `-D warnings`; part of the default full registry plan. |
| Rust unit boundary | `make check-rust-unit` | GNU-host check | Runs workspace library and binary harnesses only (`--lib --bins`), serialized, locked, and offline. `RUST_ROUTINE_EXCLUDES` omits three host-native helper packages, covered by the default `onnx-host-tests` leg, and four slower library harnesses, covered by default package suites. The topology validator has no baseline or allowlist and rejects every process-launch, network-constructor, or native-runtime call it detects in scanned unit-test code. |
| Rust doctests | `make check-rust-doc` | GNU-host check | Runs documentation tests explicitly in the default full registry plan; excluded from routine `make ci`. |
| Rust tests | `make check-rust-test` | GNU-host check | Retained direct legacy command for the workspace Rust tests. The full-gate runner executes each selected integration target from `core/ci/suites.toml` separately so one failure cannot hide later entries. |
| Rust dependency policy | `make check-rust-deny` | GNU-host check | Locked, offline bans/licenses/sources policy over the supported cargo-deny graph. |
| SPL dependency pin | `make check-spl-dependency-pin` | GNU-host check | Verifies the Rust core workspace resolves `spl-core` and `spl-transport` only through the workspace-owned `spl-rust` tag pin, with member manifests inheriting it, lockfile binding intact, and local patch/source replacement routes rejected. |
| Rust advisories | `make audit` | Standalone check | Runs cargo-deny with its default advisory behavior. It is separate from the canonical full gate. |
| iOS canary | `make check-rust-ios` | iOS cross-target canary | Cross-target drift evidence for eligible library crates; explicitly excludes `solstone-core-indexer-store` and `solstone-core-indexer-query` because their bundled-C SQLite paths are not yet in the iOS gate, and `solstone-core-speakers-analyze` plus `solstone-core-speakers-onnx` because the analyzer transitively depends on ONNX Runtime host-only native linkage. |

### Rust ONNX Runtime Provisioning

`solstone-core-speakers-onnx` links dynamically to the ONNX Runtime C API from
the journal Python environment. It does not download or vendor ONNX Runtime and
does not read paths from inside the crate. The host Rust commands
`make check-rust-msrv`, `make check-rust-clippy`, `make check-rust-test`, and
`make build` exclude `solstone-core-vad-analyze`,
`solstone-core-speakers-analyze`, and `solstone-core-speakers-onnx` through
`RUST_HOST_EXCLUDES`, so they require no ONNX Runtime or Python provisioning. The
routine `ci`/`build`/`test` path does not invoke
`scripts/resolve_onnxruntime_capi.py`. That resolver remains in the
repository and has dedicated Python tests, but no current Makefile recipe invokes
it. Current full-gate preparation uses `make ci-full-prep-onnx`, which validates
and, when needed, repairs the staged runtime before the full runner reaches its
ONNX readiness and test entries.

The resolver stages symlinks, never copies, under
`core/target/onnxruntime-link/<platform>/lib/`. Linux stages
`libonnxruntime.so`, `libonnxruntime.so.1`, and the full-version shared object.
macOS stages `libonnxruntime.dylib` and the full-version dylib from the wheel.
It then executes Cargo with `ORT_LIB_PATH=<staged lib dir>`,
`ORT_PREFER_DYNAMIC_LINK=true`, and the matching runtime loader path
(`LD_LIBRARY_PATH` on Linux, `DYLD_LIBRARY_PATH` on macOS) prepended.

Direct `cargo test --manifest-path core/Cargo.toml --workspace` without the
resolver is unsupported for the ONNX crate because Cargo dependency build
scripts run before dependent crate build scripts. A crate `build.rs` cannot
retroactively provide `ORT_LIB_PATH` to `ort-sys`.

### Rust-Conversion Freeze

The development gate is Rust-only for the duration of the conversion. The
default `make` / `make all` target now aliases the native `make build` rail. The
former release Make targets and `scripts/release.sh` have been removed. The
remaining alternate Python test rails, `test-cov`, `test-integration`,
`test-performance`, `test-app`, `test-only`, `watch`, and `coverage`, fail
immediately with the conversion-freeze diagnostic.

The table above defines the two Rust validation paths. On Linux, routine
`make ci` requires Bubblewrap and runs with networking disabled, separate PID,
IPC, and UTS namespaces, a read-only checkout except for the configured Cargo
target directory, and private disk-backed temporary storage. On macOS, the
same locked, offline Rust checks run without the Linux containment layer.

On a cold checkout or after cleaning `core/target`, run
`make ci-full-prep-cargo` before `make ci`. Run `make ci-full-prep` before the
full gate. Full preparation fetches locked Cargo inputs, materializes the host
library/binary check graph and routine library/binary test graph without
executing tests, and verifies or repairs the pinned native runtimes.
Full validation expects those inputs to be prepared. It is registry-driven; the
runner sets Cargo offline mode for every selected entry, and Cargo invocations
remain locked. It runs selected entries independently,
continues after failures, applies a bounded timeout to every selected entry,
and writes a JSON receipt bound to the clean starting revision.
`make ci-full-plan` shows the default or selected plan without executing it;
`SETS`, `AREAS`, `PACKAGES`, and `TARGETS` provide comma-separated selectors.
The default includes:

- MSRV, all-target Clippy, Rust doctests, and dependency policy;
- every registered integration target except the one opt-in target;
- each package-scope entry marked `default_full = true`;
- native runtime and helper checks;
- shipped-binary builds and smokes; and
- Apple gates on applicable hosts.

The opt-in integration target is the host-contention
`solstone-core-speakers::discovery_semantics` target. `make ci` and
`make ci-full` use the [Makefile](../Makefile)'s
`run-rust-gate-under-poison` wrapper, which prepends failing shims for `python`,
`python3`, `pytest`, `ruff`, and `uv`; invoking one exits with code 97. `make
verify` remains an alias for `make ci`.

`make audit` is a standalone default cargo-deny check. Canonical full CI uses
the locked, offline `make check-rust-deny` bans/licenses/sources policy instead.
Per the [Makefile](../Makefile), `make install-checks` and its Python-and-Rust
sub-targets also remain runnable directly, but neither Rust gate reaches them.
Neither Rust gate invokes the Python product or pytest suite.

**Do not add new Python tests.** Anything that needs a unit test is written in
Rust. Neither Rust gate runs pytest, so a new Python assertion is outside the
green gate. Separate opt-in verification targets still can. A green `ci` says
nothing about any Python assertion, so a change whose criteria include one can
report full green having run none of them.

For a component that lives behind a process boundary, the honest test is a Rust
test that **spawns the real executable** and observes its stdout, stderr and exit
code. That tests the boundary as a boundary, it lands in the language that
survives the conversion, and it puts the assertion inside the gate a change
actually names. If such a test cannot locate the executable it must fail loudly
rather than skip — a skipped test is a criterion that did not run wearing a green
tick.

The retained `tests/verify_*` Python harnesses are manual tools; neither Rust
gate selects them.

The earlier transparency publishing targets and signed-advisory packet workflow
have been removed.

## Owner Timezone

The Python owner-timezone resolution is effectively `identity.timezone` from
`config/journal.json`, then UTC. The apparent host-local branches in
`get_owner_timezone()` are dead because CPython `astimezone()` returns a
fixed-offset `datetime.timezone` without a `.key`. Reproducing host-local time
in Rust would diverge from Python behavior.

## Layering

`solstone-core` is a process shell only: it reads `std::env::args()`, writes
stdout or stderr, and returns process exit codes.

`solstone-core-cli` is the CLI adapter. It takes an argv slice as input and
returns a typed outcome. It never reads `std::env`, never prints, and never
exits.

Subsystem libraries added in later waves take config and paths as parameters,
own no process-global state, and do not parse argv. The "no argv parsing in core
logic" rule binds these subsystem libraries.

## Error And Type Mapping

Python exceptions become `Result` errors at the Rust boundary. A port should
name the error cases it can emit; it should not collapse expected failures into
strings or panics.

Python `None` becomes `Option`. Truthiness becomes explicit predicates or
comparisons. A port must not rely on implicit emptiness checks when the Python
source distinguished empty, missing, and false values.

Python context managers and `__del__` cleanup become RAII ownership and `Drop`
where cleanup is unconditional. Fallible cleanup remains explicit because `Drop`
cannot return an error.

Monkeypatching, dynamic dispatch, decorators, middleware, and import-time side
effects become explicit seams. Before porting code with any of these concerns,
inventory the concern and add a conformance test that fails when the concern is
absent; absence is otherwise invisible in a diff.

## Data Boundaries

Python integers are arbitrary precision. Rust ports use `i64` for JSON-facing
integers unless a specific writer documents another width. Overflow is a
`Result` error at the boundary, never a silent wrap or debug-only assertion.
JSON integers outside `i64` are rejected at parse **when a port deserializes
into a typed field**. That rule does not describe the untyped path: measured
against this workspace's `serde_json` configuration, parsing into
`serde_json::Value` rejects nothing — `i64::MAX + 1` and `u64::MAX` round-trip
byte-identically as `u64`, and only a value beyond `u64::MAX` degrades to `f64`,
silently and still without an error. A port that reads arbitrary journal JSON
through `Value` therefore gets no overflow boundary for free and must impose one
where it matters.

Non-finite floats are a one-way incompatibility and are worth stating here
rather than only under hashing. Python's `json` both emits and accepts bare
`NaN`, `Infinity` and `-Infinity`; `serde_json` hard-rejects all three. So a
document the Python writer could have produced is unreadable by its Rust
replacement. For `config/journal.json` this was measured as unreachable — no
config writer coerces a float and nothing in the production tree can produce a
non-finite value — and the reader's response is a strict-load failure that
leaves the file untouched, which is the correct posture. **A port over any other
Python-written JSON owes the same reachability check rather than the
assumption.**

For the Python-compatible body-import JSON decoding seam, `solstone-core-body-source` is a hand-rolled parser that retains arbitrary-precision integers and accepts non-finite floats; it is not a general-purpose `serde_json::Value` replacement.

Python `str` maps to UTF-8 `String` or `&str`. Python `bytes` maps to
`Vec<u8>`. Filesystem paths map to `PathBuf` or `OsStr`; POSIX paths are not
guaranteed to be UTF-8, so ports must not use `.to_str().unwrap()`.

## Porting Instruments

`scripts/build_core_fixtures.py` generates Rust-facing fixtures under
`core/fixtures/`.

`core/fixtures/markdown_chunks.json` pins Python markdown chunking/token output
for the Rust markdown indexer port.

`core/fixtures/speaker_filterbank.json` pins the production speaker-filterbank
stage: both Python call sites must be bit-identical, feature rows are compared
with `FILTERBANK_VALUE_ABS_TOLERANCE`, and platform provenance is diagnostic
only so cross-architecture checks pass iff the fbank values agree within
tolerance.

`core/fixtures/speaker_stage_boundaries.json` pins speaker-pipeline branch
boundaries for interval selection, speaker-evidence gating, sentence assignment,
and k-selection. Silhouette scores are compared with
`CLUSTER_SCORE_ABS_TOLERANCE`; selected k values and cluster labels remain exact.
Native pyannote segmentation logic now lives in `solstone-core-speakers`, with
ONNX model execution isolated in `solstone-core-speakers-onnx`.
`speaker_stage_boundaries.json` remains a branch-boundary fixture; model-pass
numeric parity is carried by the `tests/verify_speaker_differential.py` bundles.

`tests/verify_indexer_differential.py` runs the indexer differential harness and
writes its report under the harness work directory unless `--report` is supplied.
`tests/verify_speaker_differential.py` runs the local speaker-pipeline
differential harness and writes/compares versioned `.npz` result bundles for
Python-to-port parity checks.

`tests/verify_speaker_verdict.py` consumes those recorded bundles without
rerunning speaker models, adding decision-flip replay for clustering,
owner-claim, and acoustic-tier outcomes plus DER scoring against
caller-supplied reference turns.

## JSON And Hashing

Canonical JSON is a per-writer contract, not a repository default. A Rust port
inherits the ordering and separators of the specific writer it replaces. Examples
with explicit sorted output today include `solstone/think/talent_provenance.py`,
`solstone/think/data_state.py`, `solstone/think/readiness.py`, and
`solstone/think/steward.py`.

`solstone/think/talent_provenance.py` computes identity hashes from the exact
string returned by `_canonical_json`. Byte drift changes the SHA-256 identity.
Two traps matter:

- Float exponent spelling: Python's `repr`-backed JSON formatting emits `1e+30`.
  Rust's standard JSON float formatting emits `1e30`. Same value, different
  bytes, different SHA-256.
- Non-finite values: Python emits bare `NaN` and `Infinity` tokens, which are
  not valid JSON. Rust's standard JSON serializers refuse to emit them, so a
  payload Python hashes today cannot round-trip through a conforming Rust writer.

Hashed canonical payloads therefore carry no floats and no non-finite values. If
a future port must hash a float, it owes a byte-exact Python `repr` emitter plus
a conformance test.

There is a pre-existing Python hazard: `_canonical_json` does not reject
non-finite values. A non-finite value can enter a hashed identity today. This
design documents that hazard but does not change Python behavior.

## Unsupported Inputs

Native ports reserve process exit code 69 for inputs the native command cannot
process. Wrappers should surface that code unless a command-specific design says
otherwise. It is distinct from success, the top-level usage fallthrough (64),
command-level parser errors (such as supervisor/check/install-models, 2), empty-input
codes, and temporary failures (75). Signal death is normalized to temporary failure (75).
The supervisor intentionally keeps mapping non-zero scheduled-task exits to
`error`; command stderr carries the operator-facing detail.

### Journal Config Native Verb

`solstone-core journal-config read [--journal PATH]` emits one JSON envelope
containing `present`, `sha256`, and `config`. `solstone-core journal-config
commit [--journal PATH] [--lock-timeout-ms N] --expect <fingerprint|absent>`
accepts a complete replacement JSON object only on stdin. `--expect` is
required; `absent` represents a missing config file and fingerprints use the
strict reader's `sha256:<lowercase hex>` form. Successful commits write no
stdout.

| Exit | Name | Applies to | Meaning |
|---:|---|---|---|
| 0 | success | read, commit | Read completed, or commit performed one matched atomic replacement. |
| 64 | EX_USAGE | read, commit | Bad argv (unknown/duplicate/missing-value flag, malformed `--lock-timeout-ms`, malformed `--expect`, omitted `--expect` on commit), OR (commit only) stdin that is not valid JSON, is not a JSON object, or exceeds 1 MiB. None of these become valid on retry. |
| 65 | EX_DATAERR | commit | CAS fingerprint conflict only — the caller's `--expect` did not match the config's actual current state (present-with-different-hash, present-when-absent-was-expected, or absent-when-a-hash-was-expected). The one code a client is expected to retry against. |
| 69 | EX_UNAVAILABLE | read, commit | Existing `config/journal.json` present but unreadable/corrupt (`ConfigLoadError::Corrupt`). |
| 73 | EX_CANTCREAT | commit | Atomic replacement of the config file failed (`AtomicWriteError`). |
| 74 | EX_IOERR | commit | Non-timeout lock I/O failure (`LockError::Io`), or a stdin/stdout stream I/O failure. |
| 75 | EX_TEMPFAIL | commit | Lock acquisition timed out (`LockError::Timeout`) — retry is appropriate. |

### Indexer Native Routing

`journal indexer` executes command writes (`--reset`, `--rebuild-edges`,
`--rescan`, `--rescan-full`, and `--rescan-file`) inside the Rust journal
binary against `solstone-core-indexer-store`; it does not locate or launch an
interpreter. Owner CLI queries also execute inside the Rust journal binary
against `solstone-core-indexer-query`. `search_journal`, `search_counts`,
`known_agents`, and `get_corpus_day_coverage` route through the aggregate Rust
binary when their still-Python feature plates call them. The command no longer
reads journal config for write routing, so stale old routing keys in
`config/journal.json` are inert.

Before a remaining Python feature plate launches the native helper, the bridge checks that the current runtime
has a compatible `solstone-core` wheel: the normalized host tuple must be in
`probe.SOLSTONE_CORE_COVERED_PLATFORMS`, and the platform tags advertised by
`packaging.tags.sys_tags()` must intersect the tag set recorded in
`probe.SOLSTONE_CORE_PLATFORM_TAGS`. Linux x86_64 and Linux aarch64 require the
manylinux 2.17 / manylinux2014 glibc floor. macOS requires
`macosx_14_0_arm64`; older arm64 macOS hosts therefore report no compatible
wheel. A covered source checkout without `solstone-core` distribution metadata
also returns 78 for every write-bearing command until the developer runs
`make install`. The [native bridge behavior tests](../tests/test_indexer_native.py)
cover the preflight ordering and exit-code contract.

Backup-restore full rescans, direct `index_file()` callers, chat stream appends,
importers, day-accumulator writes, index-mutating deletes, and entity-merge edge
maintenance use explicit `solstone-core indexer` mutation verbs. Their Python
plates may initiate the operation, but SQLite is opened for writing only in Rust.

The wrapper normalizes `--rescan-file` to an absolute path with the same Python
journal-path resolver used by `index_file()` before passing it to native. This
keeps `chronicle/`-prefixed relative paths from being interpreted differently by
the Rust relative-path resolver.

Native indexer compound writes are atomic at the logical replacement-unit
boundary. A content file replacement deletes old chunks, inserts new chunks,
and writes its `files` mtime. An edge
file replacement deletes old edge rows and `edge_files` state, extracts and
inserts replacement rows, and writes the `edge_files` mtime as one unit.
Entity search deletes stale entity-search chunks, inserts replacement chunks,
and writes both watermarks as one unit. Reset is SQLite-native: it drops and
recreates index objects transactionally and does not unlink the database, WAL,
or SHM files.

Production index mutations use the native path. If edge-source extraction fails,
the scanner preserves the prior `edge_files` row and mtime so the unchanged file
is eligible on the next scan; the [failure/retry test](../core/crates/solstone-core-indexer-store/src/scan.rs)
covers that behavior. Pre-`stream` and pre-`time_bucket` indexes are migrated
transactionally before the first native mutation; the [legacy-shape tests](../core/crates/solstone-core-indexer-store/src/db.rs)
assert that their existing rows remain queryable.

The detailed native atomicity design is in
`docs/design/indexer-native-atomicity.md`.

Native sol client design records:

- `docs/design/native-sol-client/00-prep-findings.md`
- `docs/design/native-sol-client/01-oracle-repro.md`
- `docs/design/native-sol-client/02-design.md`
- `docs/design/native-sol-client/03-batch-prep.md`
- `docs/design/native-sol-client/04-batch-design.md`
- `docs/design/native-sol-client/05-raw-body-parity.md`
- `docs/design/native-sol-client/06-cutover-design.md`
- `docs/design/native-sol-client/07-notify-contract-design.md`
- `docs/design/native-sol-client/08-link-join-design.md`
- `docs/design/native-sol-client/09-link-serve-prep.md`
- `docs/design/native-sol-client/09-link-serve-design.md` (hold decision; no implementation landed)
- `docs/design/native-sol-client/resident-command-design.md`
- `docs/design/native-sol-client/11-link-serve-design.md`

## Dual Paths And Shims

The repository no-shims rule still stands. During an active port, a temporary
old/new route is a deliberate, time-boxed, per-change exception. Each dual path
needs a named deletion schedule. Do not add compatibility aliases,
deprecated-parameter handling, or compatibility re-exports.

The native `sol` cutover no longer has a Python delegation boundary: every
supported command has a native authority with a production aggregate handler or
an explicit direct native match-arm home for top-level local behavior.

`solstone/think/journal_config.py` is a second sanctioned temporary boundary:
its `mutate_journal_config` subprocess CAS client wraps the native
`solstone-core journal-config read/commit` verbs documented above, with the
former in-process writer deleted rather than retained as a fallback. The 46
call sites in 19 Python modules and the in-process `read_journal_config()`
read half remain Python-side until those modules move to native Rust/native
`sol call` authorities. The removal criterion is zero Python callers of
`mutate_journal_config`: once every caller has a native authority or direct
native call home, delete this client and `journal_config.py`'s subprocess
plumbing together.

Native brain verbs ship as `solstone-core brain <verb>` subcommands of the
installed aggregate binary, not as a standalone writer binary.
`scripts/local_contract_corpus.py` and `scripts/brain_projection_corpus.py`
are retired from `expected_outputs()` because they import from `brain_state.py`,
which the native conversion reduces to a thin transport shim. The checked-in contract and
projection fixtures remain frozen native compatibility corpus; regenerating
them requires the recorded pre-cut source tree, not a fallback implementation
in the post-cut tree.

## Version Lockstep

`scripts/render_packaging.py` keeps Python leaf packages and Cargo metadata in
lockstep with the root `pyproject.toml` version. The current lockstep assumes
`X.Y.Z`. A Python pre-release such as `0.9.0rc1` is not a valid Cargo version;
before tagging one, add and test an explicit translation rule.

## Journal Resolution Decisions

The first behavior port is `get_journal_info()` / `get_journal()` from
`solstone/think/utils.py`, backed by `solstone/think/user_config.py`.

1. **MSRV is 1.95 for the locked native dependency set.** Rust 1.87 is enough
   for the safe home path, but the current bundled SQLite dependency line
   requires Rust 1.95. The journal resolver uses the hybrid shape: literal
   `HOME` when present, and `std::env::home_dir()` only when `HOME` is absent.
   This avoids a hand-rolled unsafe `getpwuid_r` implementation.
2. **No unsafe passwd FFI.** Keeping the old 1.85 floor would require libc
   backup code with buffer sizing and retry behavior for a home-directory
   lookup. That defect surface is not justified for this port.
3. **Home normalization follows `str(Path.home() / "journal")`, not just
   `os.path.expanduser("~")`.** The port reproduces the observed layers needed
   by `user_config.default_journal()`: present-but-empty `HOME` becomes `/`,
   trailing slashes are stripped with an or-root default, repeated separators
   and `.` components are collapsed lexically, exactly two leading slashes are
   preserved, `..` is not collapsed, and `.` joined with `journal` renders as
   `journal`. If the expanded home still starts with `~`, the port raises the
   same home-unavailable error as Python's pathlib guard. This is not a general
   pathlib port.
4. **Config stripping is Python stripping.** Rust `str::trim()` is not equivalent
   to Python `str.strip()` because Python also strips U+001C..U+001F. Journal
   config values use a small Python-compatible strip helper. Environment values
   are never stripped.
5. **TOML parsing uses `toml_edit` 0.22 parse-only.** The latest TOML crates
   track TOML 1.1 behavior such as accepting `\e`, which Python `tomllib`
   rejects. `toml_edit = 0.22.27` with only the `parse` feature matches the
   `tomllib` cases this port needs and keeps the lock cost smaller than the
   `toml` facade.
6. **Unit vector tests do not mutate process env.** The shared JSON vectors
   carry raw `HOME` / `SOLSTONE_JOURNAL` inputs, config bytes, checkout-root
   state, and observed Python outcomes. Rust unit tests replay those cases by
   passing values directly to library functions. Subprocess binary tests may use
   `Command::env` and `env_remove`.
7. **The binary wires no source-checkout root.** A native binary in a venv has no
   meaningful Python checkout root, so `solstone-core journal-path` deliberately
   resolves only CLI override, env, config, and default. The library still keeps
   the four Python resolver sources: `env`, `config`, `source`, and `default`.
8. **The binary label vocabulary is a superset.** `journal-path --journal PATH`
   is a binary-surface override with no Python equivalent in `get_journal_info()`.
   It short-circuits the library resolver and prints label `cli`; the library
   `Source` enum does not add a fifth variant.
9. **Non-UTF-8 env paths stay as paths.** The Rust API accepts `OsStr` /
   `PathBuf` and has Rust-only Unix tests for non-UTF-8 env paths. The shared
   JSON vector file is UTF-8 and does not encode arbitrary env bytes.
10. **Create errors are structural and shape-equivalent.** Directory creation
    errors carry source label, path, and `io::Error` fields. Their display shape
    mirrors Python's `could not create journal directory ({source}): {path}: ...`,
    but the OS-error text is not byte-equivalent to Python's `OSError`. Nothing
    consumes that message programmatically.
11. **No improvements to path meaning.** The port does no tilde expansion,
    canonicalization, resolving, absolutization, caching, new env vars, or
    config-gated dual path. `~/journal` from config remains a literal relative
    path.
