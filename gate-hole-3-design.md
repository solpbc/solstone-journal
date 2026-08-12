# W4b (v2) design: bounded sleep waits and Callosum EOFs

## Gate decisions requiring Jer's approval

### AC13 baseline: original brief stands

AC13's inherited red is exactly the one named in the original brief:
`solstone-core-convey-shell`, test `authorization_gate`, with 11
`dead_code` errors. The symbols include `EchoQuery`, `EchoObservation`,
`EchoState`, `body_echo_router`, `body_echo_router_with_observations`,
`body_echo`, `body_echo_recorded`, `body_echo_inner`, unused
`Fixture` methods, `tree`, and `multiplexed_requests`.

This was confirmed against a detached scratch worktree at `origin/main`
`54dc2dea6` by a complete, untruncated
`make check-rust-clippy` capture: 458 lines; Make exit 2 (the nested Cargo
command exits 101); 11 error diagnostics, all in
`solstone-core-convey-shell`/`authorization_gate`; and zero
`cloned_ref_to_slice_refs` matches. The first baseline for this task ran in
this lode's stale, pre-fetch worktree and therefore measured a superseded tree.
Do not re-point AC13 to `solstone-core-doctor`. This wave does not fix the
inherited Convey-shell red.

### Non-negotiable polarity and precedence table

`await_outcome` classifies only a completed bounded **sleep poll**. It does
not turn a real test failure into an infrastructure outcome.

| Polarity | Check result / exhaustion state | Dilation | Verdict |
|---|---|---:|---|
| Positive (`assert present`) | `Held` before the budget ends | any | `Passed` |
| Positive | still `Pending` at exhaustion | `< T` | `Failed` |
| Positive | still `Pending` at exhaustion | `>= T` | `Inconclusive` |
| Negative (`assert absent`) | every check stayed `Held` through the full window | `< T` | `Passed` |
| Negative | every check stayed `Held` through the full window | `>= T` | `Inconclusive` |
| Negative | watched item appeared (`HardFail`) | any | `Failed` |
| Either | checker reports a genuine error / early child-death failure | any | `Failed` |

`HardFail` wins over a simultaneous or subsequent watchdog expiry. An
aggregate containing one real failure and one dilation expiry is `Failed`,
never `Inconclusive`.

For the positive-`Pending`/`< T` failure row, dilation is strictly the
sleep-only accumulation defined below, never total loop wall-clock. This is
material at `exited_convey_restarts_under_restart_policy`:
`fixture_process_running` forks `ps` every 5ms iteration. Counting that
body cost would read roughly 3-10x even on an idle machine and incorrectly
launder a genuine `Failed` into `Inconclusive`.

### CI isolation (stop-ship #3)

The cargo-output discriminator will be a pure function with its own test and
captured fixture only. It will not be added to the Makefile, wrapped around
`check-rust-test`, or invoked by any cargo/CI command. Therefore
`ci_gate_purity.rs` retains its pinned Cargo subcommand sequence unchanged.

## Scope and non-goals

This wave changes test support and the three supervisor integration test files
only. It adds no production supervisor or Callosum behavior. It does not
consolidate their existing `TempJournal` or `ChildGuard` types, does not
modify `ci_gate_purity.rs`, and does not fix the unrelated doctor clippy
failure.

The classifier applies only to bounded loops whose cadence is an explicit
sleep. Timeout-plus-`read_line` loops and fixed intentional delays are not
poll classifiers and remain direct.

## Shared support module

Add `core/crates/solstone-core/tests/support/await_outcome.rs`. It remains
private test support and is included from the three supervisor integration
tests and a new, same-crate focused test target using:

```rust
#[allow(dead_code)] // Individual integration-test crates use different support subsets.
#[path = "support/await_outcome.rs"]
mod await_outcome;
```

The allowance is on the module declaration, as for `support/stub_peer.rs`,
because every integration-test crate compiles the shared module independently.
The module will not be included by another crate. No new dependency or crate
is needed.

The public-to-the-test-crate surface is deliberately small:

```rust
pub(crate) enum WaitPolarity { Positive, Negative }

pub(crate) enum PollState {
    Pending,
    Held,
    HardFail(String),
}

pub(crate) struct WaitMetrics {
    pub(crate) requested: Duration,
    pub(crate) slept: Duration,
}

pub(crate) enum WaitOutcome {
    Passed(WaitMetrics),
    Failed { reason: String, metrics: WaitMetrics },
    Inconclusive(WaitMetrics),
}

pub(crate) fn await_outcome<Now, Check, Sleep>(
    polarity: WaitPolarity,
    interval: Duration,
    iterations: usize,
    now: Now,
    check: Check,
    sleep: Sleep,
) -> WaitOutcome
where
    Now: FnMut() -> Instant,
    Check: FnMut() -> PollState,
    Sleep: FnMut(Duration);

pub(crate) async fn await_outcome_async<Now, Check, Sleep, SleepFuture>(
    polarity: WaitPolarity,
    interval: Duration,
    iterations: usize,
    now: Now,
    check: Check,
    sleep: Sleep,
) -> WaitOutcome
where
    Now: FnMut() -> Instant,
    Check: FnMut() -> PollState,
    Sleep: FnMut(Duration) -> SleepFuture,
    SleepFuture: Future<Output = ()>;
```

`iterations` must be nonzero (the helper rejects zero as a caller bug). It is
an iteration budget, not a wall-clock timeout: this preserves each test's
explicit count of checks and sleep requests. The synchronous entry uses
`thread::sleep`; the async entry uses `tokio::time::sleep`. A small private
measurement/state routine is shared by the two wrappers so their classification
cannot diverge; it is not a general wait framework.

Each iteration first calls `check`. Positive checks use `Held` for the
sought condition and `Pending` for not-yet. Negative checks use `Held`
while the prohibited item remains absent and `HardFail` once it appears.
`Pending` in a negative wait is a caller bug and becomes `Failed`, preventing
an accidentally ambiguous negative assertion. `HardFail` immediately returns
`Failed`.

On every actual sleep, and only there, the helper performs:

1. `before = now()` immediately before the supplied sleep.
2. Sleeps exactly `interval` through the supplied sleeper.
3. `slept += now().saturating_duration_since(before)` and
   `requested += interval` immediately after it returns.

Condition evaluation, JSON/filesystem reads, `try_wait`, formatting, and
`fixture_process_running`'s `ps` subprocess are outside that interval. This
directly prevents v1's body-inclusive measurement bug.

## Threshold and classifier tests

Use one interval-independent ratio threshold: **T = 1.1000 (11/10)**. The
comparison is integer duration arithmetic
(`slept * 10 >= requested * 11`), not floating point. Ratio normalizes the
two intervals, and the measured idle maxima leave explicit headroom:

- 5ms: max 1.0640x, leaving 0.0360x to T.
- 10ms: max 1.0326x, leaving 0.0674x to T.

Separate thresholds add policy without evidence: the noisier 5ms sample
already bounds the shared value. Metrics may render a ratio for an assertion
message, but the verdict uses the exact 11:10 comparison.

Add focused tests in a new `core/crates/solstone-core/tests/await_outcome.rs`
target. It includes the support module and owns all its tests.

- Truth-table tests cover all rows above, including `HardFail` precedence on
  the final iteration.
- Boundary tests use injected time, actual `requested`/`slept` metrics, and
  `epsilon = 0.0010`: 1.0990x (`T - epsilon`) is `Failed`; 1.1010x
  (`T + epsilon`) is `Inconclusive`. Exact 1.1000x is also
  `Inconclusive` because the contract is `>= T`.
- A sleep-only regression test advances a fake clock substantially inside each
  condition check and exactly one interval inside each sleeper. It asserts
  `slept == requested` and a 1.0000x result. This fails if loop-body work is
  included and proves metrics come from injected timestamps rather than a stub.
- A positive early-`Held` test confirms success ignores dilation; a negative
  full-window-`Held` test confirms high dilation becomes `Inconclusive`.

## Per-site conversion inventory

### `supervisor_app_stack.rs`

| Current site | Decision | Reason |
|---|---|---|
| `ChildGuard::terminate`, sleep 81 | Convert, sync positive | Bounded process-exit wait. `try_wait` errors are `HardFail`; normal exit is `Held`. See Drop handling below. |
| `wait_for_markers`, sleep 126 | Convert, sync positive | One aggregate presence predicate; retain 1,600 iterations at 5ms. |
| `assert_marker_absent`, sleep 194 | Convert, sync negative | Required negative-polarity case; appearing marker is immediate `HardFail`; a dilated clean window is inconclusive. |
| `wait_for_path`, sleep 203 | Convert, sync positive | One path-presence predicate; retain 1,600 iterations at 5ms. |
| collector timeout, 277 | Leave direct | Timeout around a task with no poll sleep; collector already has the correct EOF guard. |
| child-reap loop, sleep 374 | Convert, sync positive | One aggregate all-PIDs-gone predicate; retain 2,000 iterations at 5ms. |
| restart-policy loop, sleep 394 | Convert, sync positive | One process-running predicate; direct sleep timing excludes its per-iteration `ps` fork. Retain 200 iterations at 5ms. |

### `supervisor_shutdown.rs`

| Current site | Decision | Reason |
|---|---|---|
| `ChildGuard::drop`, sleep 51 | Convert, sync positive cleanup wait | Preserve the 1,000x5ms grace period; cleanup force-kills after any non-pass outcome. |
| `wait_for`, sleep 89 | Convert, sync positive | Presence predicate plus `try_wait` child-exit/error as `HardFail`; retain 500x5ms. |
| `request_and_started`, timeout/read 103 | Leave as reader wait | No sleep interval; add only EOF guard. |
| task-ready loop, sleep 151 | Convert, async positive | Single path predicate; retain 300x10ms. |
| lifecycle-clear loop, sleep 177 | Convert, async positive | Closure records three first-removal ticks as side effects, is `Held` only on child exit, and is `HardFail` on `try_wait` error. Existing order assertions remain. Retain 6,000x5ms. |
| fixed delay, 200 | Leave direct | It establishes the first sync-pass baseline before injecting a foreign heartbeat; not a predicate poll or assertion window. |

### `supervisor_tick.rs`

The prep table's approximate helper ranges have drifted: live helper ranges are
`wait_for_path` 237-245 (sleep 240), `wait_for_logged_message` 247-265
(sleep 260), and `wait_for_runtime_phase` 267-281 (sleep 276).

| Current site | Decision | Reason |
|---|---|---|
| `ChildGuard::drop`, sleep 97 | Convert, sync positive cleanup wait | Preserve 1,000x5ms and existing force-kill fallback. |
| `wait_for_socket`, sleep 153 | Convert, sync positive | Socket+ready aggregate predicate; early child exit/error is `HardFail`; retain 1,600x5ms. |
| `receive_until`, timeout/read 193 | Leave as reader wait | No poll sleep; add only EOF guard. |
| `receive_started_command`, timeout/read 220 | Leave as reader wait | No poll sleep; add only EOF guard. |
| `wait_for_path`, sleep 240 | Convert, async positive | Single path predicate; retain 800x10ms. |
| `wait_for_logged_message`, sleep 260 | Convert, async positive | Logged-message predicate; retain 800x10ms. |
| `wait_for_runtime_phase`, sleep 276 | Convert, async positive | Runtime-phase predicate; retain 800x10ms. |
| AC10 scheduler loop, sleep 326 | Convert, sync positive | Scheduler status predicate; retain 800x10ms. |
| AC13 status loop, timeout/read 347 | Leave as reader wait | No poll sleep; add only EOF guard. |
| AC11 task-ready loop, sleep 405 | Convert, async positive | Readiness predicate; retain 3,000x10ms. |
| AC11 status-history loop, timeout/read 414 | Leave as reader wait | No poll sleep; add only EOF guard. |
| fixed delay, 492 | Leave direct | Deliberate 300ms observation window before asserting a batch event did not submit work. |
| restart notification, timeout/read 599 | Leave as reader wait | No poll sleep; add only EOF guard. |
| replacement-PID loop, sleep 627 | Convert, async positive | PID-change predicate; retain its 8s timeout envelope and 10ms cadence. |
| fixed delay, 670 | Leave direct | Deliberate 300ms non-materialization window before asserting segment absence. |
| fixed delay, 756 | Leave direct | Deliberate 300ms window before asserting the wedge threshold was reset. |

All converted sites preserve their current iteration count and interval. The
helper's `Inconclusive` output is rendered with requested/slept values and
site-specific predicate context; ordinary low-dilation exhaustion remains a
normal assertion failure.

## AC12: explicit and Drop-path termination behavior

`supervisor_app_stack.rs` gets a local outcome-finalizer around
`ChildGuard::terminate`. `Passed` returns. `Failed` and `Inconclusive`
first receive best-effort child cleanup (`kill` then `wait` if SIGTERM grace
expired), then a local `panic_or_log_termination` wrapper:

- outside unwinding, panic with outcome and metrics, preserving the explicit
  test failure;
- if `std::thread::panicking()` is true, emit an `eprintln!` diagnostic and
  suppress the second panic, so `Drop::drop` cannot double-panic/abort while a
  prior assertion unwinds.

`Drop::drop` still invokes `terminate` and waits. A focused test uses
`catch_unwind` for ordinary raising, plus a small drop probe during an outer
panic to prove the original panic survives and cleanup does not issue a second
panic. Shutdown/tick guards retain their current best-effort cleanup semantics;
they only adopt the shared measured wait for their grace period.

## AC9: deterministic EOF coverage

The test connections are Tokio Unix-domain Callosum streams. For each of the
six sites, connect through the existing helper (or shutdown's direct
connection), immediately drop the returned `OwnedWriteHalf`, and await the
site's reader-facing wait under its existing outer timeout. The server sees
`ReadFrame::Eof`, removes the client, and the test reader gets `Ok(0)`
without load-dependent timing.

Add the byte-count assertion immediately after every `read_line`, matching
the app-stack template. The six messages must each contain the required phrase
but name their local event/frame:

| Site | Assertion message content |
|---|---|
| tick `receive_until` | `the connection closed before supervisor <event> event for <reference>` |
| tick `receive_started_command` | `the connection closed before supervisor started frame` |
| tick AC13 status | `the connection closed before supervisor status event` |
| tick AC11 history | `the connection closed before timeout-history status event` |
| tick restart notification | `the connection closed before supervisor restarting event` |
| shutdown `request_and_started` | `the connection closed before supervisor started event for <reference>` |

Where an inline loop prevents direct exercise, extract only that file-local
reader loop into a named private async helper, preserving its predicate and
timeout at the caller. Add deterministic EOF tests that directly call each
reader-facing helper after dropping the write half and assert its own message.
No organic under-load repro is accepted as evidence.

## AC8: cargo-output discriminator and fixture

Add the first core test fixture directory:
`core/crates/solstone-core/tests/fixtures/`. The committed fixture is
`cargo-test-sigkill-supervisor-tick.txt`, matching the existing crate-local
`tests/fixtures/<descriptive-name>` convention.

Implementation will first build the target, then run this scoped invocation
with output captured:

```text
cargo test --manifest-path core/Cargo.toml -p solstone-core --test supervisor_tick \
  ac11_capped_task_is_terminated_with_timeout_exit -- --exact --nocapture
```

While it runs, implementation identifies the descendant test binary whose
executable name begins `supervisor_tick-`, sends that process `SIGKILL`, and
saves Cargo's stdout/stderr verbatim in the fixture. Cargo's footer is expected
to contain:

```text
error: test failed, to rerun pass \`-p solstone-core --test supervisor_tick\`
```

Add a pure support function,
`cargo_test_abort_discriminator(output: &str) -> CargoTestEvidence`, where
`CargoTestEvidence::RanWithoutParseableOutcome { target }` is the
`Inconclusive` result and `CargoTestEvidence::NoTestBinaryEvidence` means
the required Cargo footer was absent. It extracts the complete rerun target
(`-p solstone-core --test supervisor_tick`), not an individual test name:
libtest's partial abort output cannot reliably identify that name.

The new focused test target reads this fixture relative to
`CARGO_MANIFEST_DIR`, asserts the inconclusive variant and exact target, and
separately tests text without the footer as `NoTestBinaryEvidence`. This is
the entire AC8 use: no Makefile target, wrapper, cargo invocation, CI hook, or
`ci_gate_purity` update is added.

## Implementation order and verification intent

1. Add private support and its focused test target, including truth table,
   injected-clock formula, threshold boundaries, and pure Cargo discriminator.
   Capture and commit the AC8 fixture during this step.
2. Include support in the three supervisor test files and convert only the
   approved sleep-poll sites, retaining every iteration and interval budget.
3. Add six EOF byte-count checks, minimal file-local reader helper extractions,
   and deterministic dropped-write-half tests.
4. Add the app-stack Drop finalizer and ordinary-panic/already-panicking-drop
   tests.
5. Run only the requested narrow checks after implementation. Interpret clippy
   against the inherited AC13 `solstone-core-convey-shell`
   `authorization_gate` dead-code red (11 errors), which remains out of scope;
   no new diagnostics beyond it are acceptable.

## Risks and approvals

- The AC13 exception correction is the required gate decision.
- This wave delivers the cargo-output discriminator **mechanism** and its own
  unit-test proof only. It does not deliver end-to-end classification of a
  `make ci` run, and it changes none of `make ci`'s pass/fail color. Wiring
  the discriminator into an actual CI path is explicitly out of scope and is
  handed off to W4c alongside `check-rust-race`.
- AC8 depends on the local Cargo version emitting the documented footer for a
  killed test binary. If it differs, preserve the real captured output and
  adjust only the pure parser/fixture expectation; do not add a CI wrapper.
- High dilation makes negative absence windows inconclusive rather than green.
  That is the required asymmetric rule and may expose scheduling pressure
  earlier tests silently accepted.
- A future wait with meaningful condition-check cost must use this helper or
  remain hand-rolled; it must never measure loop-body elapsed time as sleep
  dilation.
