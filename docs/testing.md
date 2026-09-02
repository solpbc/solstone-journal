# Testing

## Test Structure

⚠ **There is no Python product test suite.** The repository-local device simulator has a small
dependency-free `unittest` suite under
`tools/journal_device_sim/tests/`; run it with `make check-journal-device-sim`.

- **Framework**: Cargo, for the native Rust workspace
- **Unit tests**: live beside their crate under `core/crates/<crate>/src/`; they exercise deterministic same-crate logic or cheap source/static contracts without crossing a product filesystem, process, service, network, platform, or native-runtime boundary
- **Broader same-crate tests**: may also live under `src/`, but the classified modules use each package's non-default `full-tests` feature and execute through package-specific classified targets; those targets are default `ci-full` entries and also preserve the legacy full-workspace preflight
- **Integration tests**: Cargo integration targets under `core/crates/<crate>/tests/`, grouped into named legs by `core/ci/suites.toml` and validated by `make check-rust-ci-topology`

| Packages | Routine selection | Broader same-crate selection | Integration selection |
|---|---|---|---|
| `solstone-core-speakers-analyze`, `solstone-core-speakers-onnx`, `solstone-core-vad-analyze` | `--no-default-features --lib`; runtime-free unit modules only | `full-tests` activates the normal `runtime` feature; `make check-rust-onnx-test` runs serially with the pinned ONNX Runtime | `solstone-core-vad-analyze::vad_oracles`; separate full-registry target |

These packages keep `runtime` in their default feature set, so ordinary production builds and supported-target checks continue to compile the shipped ONNX code. The routine no-default-feature route is narrower by design: it checks deterministic parsing, validation, provider planning, path construction, windowing, timestamp reduction, and response shaping without native linking or runtime setup. Product-filesystem, model, provider, inference, process, platform, and integration evidence stays on the full routes above.

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
- Per the [Makefile](../Makefile), `make ci` runs the routine code-focused lane: formatting, the CI-topology contract, library/binary Clippy checks, and serialized library/binary unit tests. The formerly duration-excluded `solstone-core-sol-link`, `solstone-core-convey-body`, `solstone-core-facets`, and `solstone-core-describe` packages are now split by behavior: deterministic in-memory and static-contract modules run routinely, while their filesystem, SQLite, HTTP/TLS workflow, corpus/oracle, and native/media modules require `full-tests` and run through package-specific default classified full-test legs in `make ci-full`. The existing `clippy-full` entry invokes package-specific feature-enabled Clippy targets so those broader modules retain `-D warnings` evidence. For `solstone-core-speakers-analyze`, `solstone-core-speakers-onnx`, and `solstone-core-vad-analyze`, it statically checks the default production closure and runs the runtime-free library closure. Their broader same-crate tests use `full-tests` with the normal runtime feature and remain in the staged full gate.
- The topology validator has no baseline or allowlist. It rejects every
  process-launch, network-constructor, or native-runtime call it detects in
  scanned unit-test code. On Linux, `make ci` requires Bubblewrap and runs with
  the network, PID, IPC, and UTS namespaces unshared, the checkout read-only
  except for the Cargo target directory, temporary storage rooted under
  `/var/tmp`, and Cargo offline. On macOS, the same Rust checks run locked and
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
- Run one crate's default-feature same-crate tests with `cargo test --manifest-path core/Cargo.toml -p <crate> --lib --bins`. For `solstone-core-sol-link`, `solstone-core-convey-body`, `solstone-core-facets`, and `solstone-core-describe`, that selects only routine same-crate evidence; run the matching `make check-rust-classified-full-tests-<suffix>` target for broader `full-tests` same-crate evidence and `make check-rust-classified-full-clippy-<suffix>` for its feature-enabled lint evidence. The sol-link targets also enable `test-hooks`. Omit `--lib --bins` only when you intend Cargo's eligible integration-target and doctest selection. A crate command does **not** run a dependency's tests; use `--workspace` when you need the default-feature sweep.
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
