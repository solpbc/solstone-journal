# Testing

## Test Structure

⚠ **There is no Python product test suite.** The repository-local device simulator has a small
dependency-free `unittest` suite under
`tools/journal_device_sim/tests/`; run it with `make check-journal-device-sim`.

- **Framework**: Cargo, for the native Rust workspace
- **Unit tests**: live beside their crate under `core/crates/<crate>/src/` and run in `make ci`. They check code correctness or a contract (a wire format, a schema, a function's documented output) inside the test process.
- **Broader same-crate tests**: may also live under `src/`, behind the crate's non-default `full-tests` feature. The crate's package suite in `core/ci/suites.toml` enables that feature (`features = ["full-tests"]`, `default_full = true`), so `make ci-full` runs them; CI topology validation refuses a crate that declares the feature without such a suite. The exception is the three ONNX-linked packages in the table below, whose `full-tests` runs through `make check-rust-onnx-test`.
- **Integration tests**: Cargo integration targets under `core/crates/<crate>/tests/`, grouped into named legs by `core/ci/suites.toml` and validated by `make check-rust-ci-topology`

| Packages | Routine selection | Broader same-crate selection | Integration selection |
|---|---|---|---|
| `solstone-core-speakers-analyze`, `solstone-core-speakers-onnx`, `solstone-core-vad-analyze` | `--no-default-features --lib`; runtime-free unit modules only | `full-tests` activates the normal `runtime` feature; `make check-rust-onnx-test` runs serially with the pinned ONNX Runtime | `solstone-core-vad-analyze::vad_oracles`; separate full-registry target |

These packages keep `runtime` in their default feature set, so ordinary production builds and supported-target checks continue to compile the shipped ONNX code. The routine no-default-feature route is narrower by design: it checks deterministic parsing, validation, provider planning, path construction, windowing, timestamp reduction, and response shaping without native linking or runtime setup. Product-filesystem, model, provider, inference, process, platform, and integration evidence stays on the full routes above.

## What belongs in `make ci`

`make ci` runs on every change, often several at once on one shared host. Write new tests so it stays unit-only.

- **Routine:** logic, parsing, state transitions, error mapping, and contracts checked against committed fixtures. Temporary directories are fine: the Linux gate mounts `/tmp` and `/var/tmp` as memory-backed scratch, so journal writes and fsync are cheap there.
- **Behind `full-tests`:** a test that starts a process (a stub script, `git`, a host tool, a re-exec of the test binary), opens a socket (TCP, UDP or Unix, loopback included), reads host state (`/proc`, network interfaces, installed tools), or waits on the wall clock. Mark the test function or module `#[cfg(all(test, feature = "full-tests"))]`; helpers used only by those tests take the same attribute. The topology check refuses a direct `Command::new` or socket constructor in routine test code, but it cannot see one behind a helper, so apply the rule yourself.
- **In `make ci-full`:** a test that walks or parses the whole repository. Prefer a visibility boundary the compiler enforces (`pub(crate)`, a `test-hooks` re-export) to a scan.
- **Nowhere:** a new test that pins the text of the Makefile, CI configuration or another file's source, and a test that enforces copy, casing, punctuation, color or other design choices. Assert what the code produces for an input, not how its strings or styles read.

## Fixture Journal

A test points the journal at the checked-in fixtures by setting `SOLSTONE_JOURNAL`:

```
SOLSTONE_JOURNAL=tests/fixtures/journal
```

A Rust test sets `SOLSTONE_JOURNAL` itself. There is no autouse fixture.

The `tests/fixtures/journal/` directory contains immutable mock input with sample
facets, agents, transcripts, and indexed data. Tests may read it directly. Any
test that writes, scans, or rebuilds journal/index state must first copy the
needed input into a temporary directory such as `tempfile::TempDir`.

## Running Tests

- `make test` runs selected Rust library/binary unit harnesses and reports its
  source-derived omission boundary
- `make check-journal-device-sim` runs the dependency-free simulator unit and fake-bridge tests
- `make check-talent-fault-sim` checks the [finite talent fault runner](../tools/talent_fault_sim/README.md), whose native-worker scenarios run separately against disposable journals
- Per the [Makefile](../Makefile), `make ci` runs the routine code-focused lane: formatting, the CI-topology contract, library/binary Clippy checks, and serialized library/binary unit tests. Tests behind a crate's `full-tests` feature run through that crate's registry package suite in `make ci-full`, and the `clippy-full` entry lints each such crate with its suite's features so those modules keep `-D warnings` evidence. For `solstone-core-speakers-analyze`, `solstone-core-speakers-onnx`, and `solstone-core-vad-analyze`, it statically checks the default production closure and runs the runtime-free library closure. Their broader same-crate tests use `full-tests` with the normal runtime feature and remain in the staged full gate.
- The topology validator has no baseline or allowlist. It rejects every
  process-launch, network-constructor, or native-runtime call it detects in
  scanned unit-test code. On Linux, `make ci` requires Bubblewrap and runs with
  the network, PID, IPC, and UTS namespaces unshared, the checkout read-only
  except for the Cargo target directory, memory-backed `/tmp` and `/var/tmp`
  (capped by `CI_TMPFS_BYTES`), and Cargo offline. On macOS, the same Rust checks run locked and
  offline without the Linux containment layer.
- On a cold checkout or after cleaning `core/target`, run
  `make ci-full-prep-cargo` before `make ci` to materialize the build graph.
- Prepare the full gate explicitly with `make ci-full-prep`. Preparation owns
  the locked Cargo fetch, materializes the host library/binary check graph and
  routine library/binary test graph without executing tests, and verifies or
  repairs the pinned ONNX and PDFium runtime stages. During `make ci-full`, the
  runner sets `CARGO_NET_OFFLINE=true` for every selected entry, and Cargo
  invocations remain locked.
- `make ci-full` runs the default full-gate plan defined in
  `core/ci/suites.toml`. It keeps going after a failing registry entry, applies
  a timeout to every selected registry entry,
  and writes a revision-bound JSON receipt under `target/ci-receipts/`. With
  default Cargo target settings, this is outside `core/target`, so `make clean`
  leaves the evidence intact.
  Outcomes are `PASS`, `FAIL`, `BLOCKED`, `SKIP`, or `INCONCLUSIVE`; anything
  except `PASS` or a platform `SKIP` makes the command fail. Execution requires
  a clean Git worktree so the receipt is bound to the clean starting revision.
- `make ci-full-plan` prints the selected plan without executing it. The same
  selectors work with both plan and run commands:

  ```bash
  make ci-full-plan AREAS=stats,support
  make ci-full AREAS=stats
  make ci-full SETS=native
  make ci-full PACKAGES=solstone-core-stats-web
  make ci-full TARGETS=solstone-core-top::render_reference,fmt
  ```

  Values separated by commas are unioned within one selector. Supplying more
  than one selector intersects those dimensions. Unknown values and selections
  that match nothing are errors, never empty green runs. Use `RECEIPT=path` to
  choose the receipt location.
- `core/ci/suites.toml` is the source of truth for integration targets and named
  full-gate legs. Its contract rejects missing, duplicated, stale, or unknown
  Cargo targets. The default plan includes MSRV, all-target Clippy, doctest,
  dependency-policy, native runtime/helper, shipped-binary, and Apple-platform
  coverage. It includes every registered integration target except
  `solstone-core-speakers::discovery_semantics` and
  `solstone-core-describe::cli`; the latter runs once through the default
  `describe-stubs` leg, which also checks its stub census. Package-scope entries
  marked `default_full = true` run as well.
- `make check-rust-race` remains a selectable, repeated contention lane rather
  than part of the default full plan. Live-service validation remains an
  operator lane and is never inferred from a successful automated receipt.
- New concurrency-sensitive supervisor integration tests must use
  `core/crates/solstone-core/tests/support/await_outcome.rs`, emit the
  `SUPERVISOR_RACE_INCONCLUSIVE` marker when that helper returns an inconclusive outcome,
  and join `RUST_RACE_TEST_TARGETS` so `make check-rust-race` covers them.
- Run one crate's default-feature same-crate tests with `cargo test --manifest-path core/Cargo.toml -p <crate> --lib --bins`. That selects only routine same-crate evidence; for a crate's `full-tests` evidence run `make check-rust-registry-package CI_PACKAGE=<crate> CI_FEATURES=<features> CI_RUNTIME=<runtime>` with the values its package suite names (for the three ONNX-linked packages, `make check-rust-onnx-test`). Omit `--lib --bins` only when you intend Cargo's eligible integration-target and doctest selection. A crate command does **not** run a dependency's tests; use `--workspace` when you need the default-feature sweep.

## On-demand local thinking installation

The [local thinking installation harness](local-thinking-install.md)
runs the real portal against disposable fresh and prior-MLX journals, through to
an inference answer. Run it on an Apple Silicon Mac when changing this lifecycle;
it is separate from ordinary CI and release gates. The guide covers candidate
preparation, baseline comparisons, receipts, and cleanup.

## Worktree Development

Run the full stack (supervisor + callosum + sense + cortex + convey) against test fixture data:

```bash
make dev                    # Start stack (Ctrl+C to stop)
```

In a second terminal, hit endpoints:

```bash
export SOLSTONE_JOURNAL=tests/fixtures/journal
curl -s http://localhost:$(cat tests/fixtures/journal/health/convey.port)/
```

Notes:

- Agents won't execute without API keys. This is expected in worktrees.
- Output artifacts go in `scratch/` (git-ignored)
- Service logs: `tests/fixtures/journal/health/<service>.log`
- `make dev` writes runtime artifacts (stats cache, health logs, task logs) into
  the fixtures journal. They are covered by `tests/fixtures/journal/.gitignore`
  and should never be committed.
