# Python to Rust Porting Doctrine

This document is for engineers and coding agents porting solstone behavior from
Python into the Rust workspace under `core/`. It records the wave-0 rules before
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

## Native Dependency Release Proof

A Rust conversion that adds or bumps a dependency with C/C++ build steps or
native linkage is not complete after source checks alone. Before the conversion
wave closes, prove the supported release targets still build and pass artifact
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

| Evidence | Repository command | Class | Notes |
|----------|--------------------|-------|-------|
| Rust formatting | `make check-rust-fmt` | GNU-host check | Host source-format evidence only. |
| Rust MSRV | `make check-rust-msrv` | GNU-host check | Verifies the pinned MSRV rail without changing `rust-version`; excludes `solstone-core-speakers-analyze` and `solstone-core-speakers-onnx` from host coverage and invokes no Python. |
| Rust lint | `make check-rust-clippy` | GNU-host check | Runs clippy with `-D warnings`; excludes `solstone-core-speakers-analyze` and `solstone-core-speakers-onnx` from host coverage and invokes no Python. |
| Rust tests | `make check-rust-test` | GNU-host check | Runs workspace Rust tests on the GNU host, excluding `solstone-core-speakers-analyze` and `solstone-core-speakers-onnx`; invokes no Python. |
| Rust dependency policy | `make check-rust-deny` | GNU-host check | Locked, offline bans/licenses/sources policy over the supported cargo-deny graph. |
| SPL dependency pin | `make check-spl-dependency-pin` | GNU-host check | Verifies the Rust core workspace resolves `spl-core` and `spl-transport` only through the workspace-owned `spl-rust` tag pin, with member manifests inheriting it, lockfile binding intact, and local patch/source replacement routes rejected. |
| Rust advisories | `make audit` | GNU-host check | Verifies a signed advisory mirror packet, materializes its bundle locally, then performs a locked offline advisory check without refreshing or mutating the operator inputs. |
| iOS canary | `make check-rust-ios` | iOS cross-target canary | Cross-target drift evidence for eligible library crates; explicitly excludes `solstone-core-indexer-store` because the native SQLite store is not yet in the iOS gate, and `solstone-core-speakers-analyze` plus `solstone-core-speakers-onnx` because the analyzer transitively depends on ONNX Runtime host-only native linkage. |
| Core sdist compile inputs | `make check-core-sdist-compile-inputs` | Packaging-source check | Verifies shipping Rust compile-time inputs are discovered and covered by the normalized `solstone-core` sdist injection set. |
| Release candidate rail | `scripts/release.sh --candidate` / `scripts/release.sh --recover <version> <source-commit>` | Frozen | During the Rust-conversion freeze, the script refuses unconditionally before producing any evidence. |

### Rust ONNX Runtime Provisioning

`solstone-core-speakers-onnx` links dynamically to the ONNX Runtime C API from
the journal Python environment. It does not download or vendor ONNX Runtime and
does not read paths from inside the crate. The host Rust commands
`make check-rust-msrv`, `make check-rust-clippy`, `make check-rust-test`, and
`make build` exclude `solstone-core-speakers-analyze` and
`solstone-core-speakers-onnx` through `RUST_HOST_EXCLUDES`, so they require no
ONNX Runtime or Python provisioning. The frozen `ci`/`build`/`test` path does not
invoke `scripts/resolve_onnxruntime_capi.py`. That resolver remains in the
repository and has dedicated Python tests, but no current Makefile recipe invokes
it. `wheel-speakers-analyze-linux-x86_64`,
`wheel-speakers-analyze-linux-aarch64`, and `wheel-macos` build the analyzer for
distribution by staging the runtime and setting `ORT_LIB_PATH` directly.

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
release rail (`release`, `release-test`, `release-checks`, `publish-release`, and
`publish-release-test`) and `scripts/release.sh` itself are hard-frozen: every
mode, including `--candidate`, `--recover`, and `--dry-run-linux`, fails
immediately with a freeze diagnostic. The alternate Python test rails
`test-cov`, `test-integration`, `test-release`, `test-performance`, `test-app`,
`test-only`, `watch`, and `coverage` do the same. There is no bypass; this freeze
lifts only when the Makefile and release script are changed again.

`make audit` is unaffected and still runs its Python advisory validator.
`make install-checks` and its Python-and-Rust sub-targets also remain
runnable directly, but `ci` and `verify` no longer reach them. The Python product
and pytest suite are unchanged; they are simply no longer gated by `ci`.

**Do not add new Python tests.** Anything that needs a unit test is written in
Rust. The Python tree is reference material for the duration of the conversion
and is removed before the next release, so a new Python test is investment in
something being deleted — and because no `make` target runs pytest, it is also
investment nothing executes. A green `ci` says nothing about any Python
assertion, so a wave whose criteria include one can report full green having run
none of them.

For a component that lives behind a process boundary, the honest test is a Rust
test that **spawns the real executable** and observes its stdout, stderr and exit
code. That tests the boundary as a boundary, it lands in the language that
survives the conversion, and it puts the assertion inside the gate a wave
actually names. If such a test cannot locate the executable it must fail loudly
rather than skip — a skipped test is a criterion that did not run wearing a green
tick.

The one Python test that still earns its place is a **cross-language
differential** comparing a rewritten component against the reference
implementation it replaces. That cannot be written in one language, and deleting
the reference makes "does the rewrite behave like the original?" permanently
unanswerable.

Transparency is intentionally different: `TRANSPARENCY_ACTIVATED ?= 0` is
exported by the Makefile and is checked inactive by default. It soft-gates
`check-transparency-minisign`, `publish-transparency`, and
`resign-transparency-pointer` through Makefile `ifeq` branches, as well as the
direct `scripts/transparency_publish.py` CLI entrypoint. Set
`TRANSPARENCY_ACTIVATED=1` in the environment or invoke
`make TRANSPARENCY_ACTIVATED=1 <target>` to reach the real implementation. Unlike
the hard-frozen release rail, this transparency gate is reversible without a
code change.

### Signed Advisory Mirror Audit

`make audit` requires four operator-provided inputs: `AUDIT_ADVISORY_BUNDLE`
for the local advisory bundle, `AUDIT_ADVISORY_RECEIPT` for the freshness
receipt, `AUDIT_ADVISORY_PUBKEY` for the approved minisign public key, and
`AUDIT_ADVISORY_LOCATOR` for the private mirror locator. The signature selector
is derived from the receipt path as `<receipt>.minisig`; there is no separate
signature option.

The audit is local-only. Git verifies and clones only the local bundle file, and
the locator is used only as cargo-deny's advisory database identity in the
offline check. It is never used as a clone source, fetched, pulled, or probed.
Use a placeholder such as `PRIVATE_MIRROR_LOCATOR` in notes and logs; do not
record a real private host, path, credential, or URL-derived token.

The public trust pins are key ID `5FCC81CD3DE12315` and public-key SHA-256
`c9fb713fe57791afbdebddde7b334e950ce1efcc167d49daf4cc1cbd930bb122`. The
receipt must be canonical JSON, its adjacent minisign signature must carry the
trusted comment for the same advisory commit and UTC time, and the receipt UTC
is the only freshness authority.

On success, stdout is exactly one compact JSON object with these fields:
`product`, `advisory_cohort`, `synced_commit`, `receipt_utc`, `max_age`,
`checked_at`, `cargo_lock_sha256`, `cargo_deny_version`, and `verdict`.
The witness contains no paths, locators, credentials, or child process output.

The audit is non-destructive: packet inputs, the source tree, ambient Cargo
state, and release candidate/evidence directories are not modified. The
bundle-cloned advisory database and cargo-deny config are owned temporary
materialization and are removed before the success witness is emitted.
If any gate fails, reacquire the signed packet from the controlled mirror
process, place the adjacent signature next to the receipt, verify the public key
pin, and rerun `make audit`.

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

`tests/verify_speaker_discovery_clustering_differential.py` runs the
unknown-speaker discovery clustering differential, feeding an `.npz` embedding
matrix to sklearn and to the native analyzer. Its default `production-path` mode
drives the `solstone.apps.speakers.discovery` kernel invocation path with the
operator-supplied helper binary, while `direct-binary` keeps the lower-level
`discovery-cluster` request mode available. The report separates noise-boundary
flips from cluster-to-cluster structural moves.

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
otherwise. It is distinct from success, usage errors (64), empty-input codes, and
temporary failures (75). Signal death is normalized to temporary failure (75).
The supervisor intentionally keeps mapping non-zero scheduled-task exits to
`error`; command stderr carries the operator-facing detail.

### Indexer Native Write Routing

`journal indexer` routes command writes (`--reset`, `--rebuild-edges`,
`--rescan`, `--rescan-full`, and `--rescan-file`) to the sibling
`solstone-core indexer` binary. Query-only invocations remain in Python. Mixed
write+query invocations run native writes first; on native success they enter
the Python query path, and on native non-zero they return that code without
querying. The command no longer reads journal config for write routing, so stale
old routing keys in `config/journal.json` are inert.

Before launching the native helper, the wrapper checks that the current runtime
has a compatible `solstone-core` wheel: the normalized host tuple must be in
`probe.SOLSTONE_CORE_COVERED_PLATFORMS`, and the platform tags advertised by
`packaging.tags.sys_tags()` must intersect the tag set recorded in
`probe.SOLSTONE_CORE_PLATFORM_TAGS`. Linux x86_64 and Linux aarch64 require the
manylinux 2.17 / manylinux2014 glibc floor. macOS requires
`macosx_14_0_arm64`; older arm64 macOS hosts therefore report no compatible
wheel. A covered source checkout without `solstone-core` distribution metadata
also returns 78 for every write-bearing command until the developer runs
`make install`.

Backup-restore full rescans, direct `index_file()` callers, chat stream appends,
importers, day-accumulator writes, and index-mutating deletes bypass
`journal indexer` and continue to use the named Python in-process writers.

The wrapper normalizes `--rescan-file` to an absolute path with the same Python
journal-path resolver used by `index_file()` before passing it to native. This
keeps `chronicle/`-prefixed relative paths from being interpreted differently by
the Rust relative-path resolver.

Native indexer compound writes are atomic at the logical replacement-unit
boundary. A content file replacement deletes old chunks, inserts new chunks,
writes its `files` mtime, and co-commits its segment aggregate rebuild. An edge
file replacement deletes old edge rows and `edge_files` state, extracts and
inserts replacement rows, and writes the `edge_files` mtime as one unit.
Entity search deletes stale entity-search chunks, inserts replacement chunks,
and writes both watermarks as one unit. Reset is SQLite-native: it drops and
recreates index objects transactionally and does not unlink the database, WAL,
or SHM files.

Command writes now use the native path only. Journals containing edge source
files whose extraction fails preserve prior native `edge_files` rows and mtime
so the unchanged file retries on the next scan. The remaining Python in-process
bypass consumers keep their existing Python semantics because they do not enter
`journal indexer`.

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
- `docs/design/native-sol-client/10-resident-command-lane-design.md`
- `docs/design/native-sol-client/11-link-serve-design.md`

## Dual Paths And Shims

The repository no-shims rule still stands. During an active port, a temporary
old/new route is a deliberate, time-boxed, per-change exception. Each dual path
needs a named deletion schedule. Do not add compatibility aliases,
deprecated-parameter handling, or compatibility re-exports.

The native `sol` cutover has one sanctioned temporary delegation boundary: the
finite private compatibility inventory in `solstone/think/sol_compat_inventory.py`,
checked by `scripts/check_native_sol_compat.py`. The inventory is the only
authority for that command set; do not copy the list into docs or gates. The
removal criterion is zero Python delegation from supported-platform native
`sol`: every remaining compatibility path has either a native authority with a
production aggregate handler or an explicit direct native match-arm home for
top-level local behavior, then the compatibility inventory and module exec
bridge are deleted together.

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
