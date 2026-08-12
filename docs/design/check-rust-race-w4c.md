# W4c design — check-rust-race

## Decision summary

Add a manually-invoked Rust timing gate, check-rust-race, not a new leg of
make ci. It runs five whole, serialized supervisor integration suites in
parallel beneath bounded CPU contention and reports each run as GREEN,
INCONCLUSIVE, or FAILED. ci-under-poison retains its current Cargo traversal,
so its closing informational echo names this gate alongside check-differentials
rather than invoking it.

The registered set is exactly the three tests already using W4b's load-aware
WaitOutcome contract:

- supervisor_app_stack
- supervisor_shutdown
- supervisor_tick

This is intentionally not a blanket list of all files that sleep.

## 1. Synthetic load magnitude as a number, and which budget it pressures

The proposed Makefile variables are:

- RUST_RACE_RUNS ?= 5
- RUST_RACE_LOAD_JOBS ?= 12

RUST_RACE_LOAD_JOBS ?= 12 is the synthetic load-generator count on the
established 16-core development host. Twelve busy-spin workers run while the
five concurrent Cargo test processes also compete for those cores. This
deliberately overcommits the scheduler while retaining a modest scheduling
margin; the variable is an explicit operator override for different hosts.

The untouched W4b mechanism has the relevant threshold in
core/crates/solstone-core/tests/support/await_outcome.rs lines 7–10 and
207–209: slept/requested >= 11/10, or 1.10x, is dilated. For a positive wait
whose check is still Pending when its iteration budget expires, that dilation
produces WaitOutcome::Inconclusive rather than WaitOutcome::Failed.

The concrete positive polling budgets which this load is intended to pressure
are:

- App stack, core/crates/solstone-core/tests/supervisor_app_stack.rs:169–187
  and :270–285: 5 ms × 1,600 for marker/path waits; :119–130 and :450–463:
  5 ms × 2,000 for termination and child reaping; and :481–494: 5 ms × 200
  for restart.
- Shutdown, core/crates/solstone-core/tests/supervisor_shutdown.rs:110–127:
  5 ms × 500 for boot; :197–210: 10 ms × 300 for task readiness; and
  :224–251: 5 ms × 6,000 for teardown.
- Tick, core/crates/solstone-core/tests/supervisor_tick.rs:174–193:
  5 ms × 1,600 for boot; :285–346, :566–583, and :845–867: 10 ms × 800 for
  path/log/runtime/schedule/restart waits; and :647–660: 10 ms × 3,000 for
  AC11 readiness.

For these three W4b-converted files specifically, a load-only delayed positive
poll that is still pending at exhaustion cannot be pushed below the 1.10x
dilation threshold in a way that misclassifies it as FAILED: if load delayed
the sleeps enough to exhaust the budget, slept/requested is at least 1.10x and
W4b reports INCONCLUSIVE. Thus the worst load-only outcome is INCONCLUSIVE,
not a false ordering FAILED. A true PollState::HardFail or a real assertion
remains FAILED.

This reasoning does **not** extend to any test outside the registered set.
Those tests retain raw hard-timeout polling or otherwise do not use the W4b
truth table, so synthetic scheduling delay can become a normal named test
failure and violate AC3.

There are non-poll deadline paths, such as fixed Tokio receive timeouts.
Twelve workers are therefore a calibration choice, not a mathematical guarantee
for every external-resource failure. Immediately before AC1 validation,
re-check CPU count, current load, and other worktree processes; prep's 18.42
load average and orphaned fixtures are not a quiet-host baseline. If the host
differs materially, override the worker count rather than changing W4b's
threshold or test budgets.

## 2. K and what runs in parallel with what

RUST_RACE_RUNS ?= 5 is the repeat count. Each one of those K runs is one
**complete** invocation of the three registered tests together:
supervisor_app_stack, supervisor_shutdown, and supervisor_tick.

Each complete invocation retains -- --test-threads=1, matching check-rust-test's
existing serialization discipline. This is a new, separate Cargo invocation; it
does not remove or alter --test-threads=1 in check-rust-test itself.

The K runs execute concurrently **with each other** through backgrounded
subshells plus a wait barrier. Each run has its own captured output and status
file. The RUST_RACE_LOAD_JOBS busy-spin workers run concurrently with all K
runs for the same duration.

**K parallel processes, each internally serial; the parallelism is across runs,
not across tests within a run.**

The registered suites create journals under unique epoch-nanosecond temp roots
and use journal-local Callosum sockets:

- core/crates/solstone-core/tests/supervisor_app_stack.rs:42–46
- core/crates/solstone-core/tests/supervisor_shutdown.rs:26–30
- core/crates/solstone-core/tests/supervisor_tick.rs:29–33

supervisor_tick's 4312 value is recorded data, not a shared bound TCP listener.
No fixed test temp path or listening port was found in the registered set. The
dynamic roots are adequate for K=5, although parallel validation must still
check for leaked child processes.

## 3. Registered targets and scope boundary

Near ONNX_HOST_TEST_PACKAGES, add:

- RUST_RACE_TEST_TARGETS := --test supervisor_app_stack --test supervisor_shutdown --test supervisor_tick
- RUST_RACE_RUNS ?= 5
- RUST_RACE_LOAD_JOBS ?= 12

Add an adjacent scope comment: this list is only the supervisor integration
tests that include support/await_outcome.rs; they are the only current tests
whose timeout result becomes an explicitly classified inconclusive outcome
under dilated polling.

Seven evaluated files remain explicitly out of scope:

- Supervisor-domain but not W4b-converted: supervisor_boot,
  supervisor_providers, and restart_convey_supervisor_seam.
- Concurrency-sensitive but outside supervisor scope: cogitate_session and
  generate_session.
- Not race detectors: convey_restart_no_python_spawn and convey_process.

The assignment text calls these “the other 6,” but the established inventory is
seven files (3 + 2 + 2). Implementation comments and documentation must use the
actual count and list.

Registering the three additional supervisor-domain files now would violate AC3:
they use raw thread::sleep polling and hard assertions, so load could create a
real named libtest failure that the outer classifier must correctly call FAILED.
The two session tests are genuine races but not supervisor-domain tests; the two
Convey tests are not timing race detectors.

## 4. Exact naming in ci-under-poison, and the naming guard

Extend ci-under-poison's existing second closing @echo line, which currently
names check-differentials, or add an adjacent closing @echo line, to also name
the exact target check-rust-race and direct readers to run make check-rust-race
for concurrency-sensitive supervisor changes.

This is purely an informational echo, **not** a $(MAKE) check-rust-race
invocation. The Cargo-subcommand traversal chain in ci-under-poison remains
untouched.

Add a ci_gate_purity.rs naming guard following
every_differential_test_is_named_in_its_own_gate or the simpler
target_body(...).contains(...) pattern in
make_ci_builds_and_exercises_every_host_packaged_binary. The new test asserts
that target_body(makefile, "ci-under-poison") contains the literal string
check-rust-race. This guard reds if that naming is ever removed.

**Standalone confirmation:** the check-rust-race target is never invoked via
$(MAKE) check-rust-race anywhere in ci-under-poison's recipe body, and it is
not part of any $(MAKE) ci traversal; it is a manually-invoked target only,
following check-differentials' deliberately-excluded-but-named precedent.

Do not add it to the ci-under-poison make chain. That would change the pinned
Cargo subcommand vector asserted by make_ci_never_executes_forbidden_interpreters
and make normal Rust CI execute stress load.

## 5. The drift guard — the two lists and how removal reds it

Add a focused ci_gate_purity.rs guard, for example
every_w4b_supervisor_test_is_named_in_rust_race_gate, modeled on
every_host_excluded_crate_is_tested_by_a_ci_target:

1. The Makefile-side list is parsed from RUST_RACE_TEST_TARGETS := into a
   BTreeSet of test names following --test.
2. The independently-derived list scans
   core/crates/solstone-core/tests/supervisor_*.rs for files whose source
   contains the literal inclusion #[path = "support/await_outcome.rs"].
   Support and unit-test files are excluded. Each expected target name is
   derived from the file stem.
3. The guard asserts exact BTreeSet equality between those two lists. The
   current set is supervisor_app_stack, supervisor_shutdown, and supervisor_tick.
4. The guard also asserts target_body(makefile, "check-rust-race") references
   $(RUST_RACE_TEST_TARGETS), rather than a hand-copied list.
5. The separate naming guard in section 4 asserts the ci-under-poison
   informational echo contains check-rust-race.

If a target is removed from RUST_RACE_TEST_TARGETS while its supervisor_*.rs
file still exists and still includes support/await_outcome.rs, the two sets
diverge and the guard's assert_eq! fails. This gives AC5 a direct,
source-derived failure mode.

AC5 validation must capture the actual printed **TEST RESULT LINE** from
Cargo/libtest output: the named failing test and its assertion panic message.
Cargo exit code 101 is insufficient proof because it also represents ordinary
build failures; it does not establish that this particular list-membership
guard was what failed.

Cargo auto-discovers normal tests/*.rs integration targets. The three registered
files have no explicit [[test]] stanza in core/crates/solstone-core/Cargo.toml;
the existing explicit stanzas are for differential-gated tests. The filesystem
scan therefore follows the actual integration-target naming convention rather
than duplicating a Cargo list.

## 6. Classification and the W4B_INCONCLUSIVE marker

### Stable marker

The existing W4b support layer is intentionally unchanged. Update only the
WaitOutcome::Inconclusive arm of panic_for_wait in each registered consumer
(supervisor_app_stack.rs, supervisor_shutdown.rs, and supervisor_tick.rs) to
prefix its panic text with the stable exact marker W4B_INCONCLUSIVE.

The current panic text exposes only free-form WaitMetrics::describe() output.
Parsing a printed floating-point dilation would duplicate the private 11/10
threshold and could incorrectly treat a real HardFail after delayed polls as a
load artifact. The marker changes test diagnostics only, not product behavior
or the trusted W4b support file.

### Classifier location and shape

Add solstone-core-race-classifier, a small Rust binary in
core/crates/solstone-core/tests/bin/, declared as an explicit [[bin]] with
test = false in core/crates/solstone-core/Cargo.toml. Existing helper binary
declarations at Cargo.toml lines 54–62 are the local precedent; no
harness = false test-target precedent exists in the workspace.

A normal binary is preferable: it takes a capture path and recorded Cargo exit
status as ordinary arguments, can be built before load begins, and cannot be
accidentally executed with no arguments by the normal workspace test gate. It
includes the unmodified support module using the existing consumer pattern,
adapted for its directory location, #[path = "../support/await_outcome.rs"].
It can therefore call cargo_test_abort_discriminator directly rather than
reimplementing Cargo footer parsing in shell.

Put the pure output-routing function in a new test-support module, with a small
auto-discovered Rust unit/integration test using canned Cargo output. It must
cover five routes: success; marker-tagged named libtest failure; ordinary named
libtest failure; the committed SIGKILL fixture; and nonzero output with no
test-binary evidence. The helper binary only adapts arguments, reads the
capture, and prints the verdict.

### Exact routing decision tree

The classifier receives the recorded exit status plus complete captured combined
Cargo output.

1. Cargo exit status zero is GREEN.
2. For nonzero exit, parse genuine libtest failures using a failed test summary
   (test result: FAILED plus its failures: list) and the matching per-test
   failure section or panic text. A mere string FAILED or an expected panic is
   not enough.
3. If any named failed test lacks W4B_INCONCLUSIVE, return hard FAILED and name
   every ordinary failing test. This includes assertion, panic, and ordering
   violations. If every named failure is marker-tagged, return INCONCLUSIVE,
   naming the affected tests.
4. Only when there is no named libtest failure, invoke
   cargo_test_abort_discriminator:
   - RanWithoutParseableOutcome { target } returns INCONCLUSIVE, including the
     exact Cargo rerun target. This covers the committed SIGKILL shape.
   - NoTestBinaryEvidence returns distinctly labeled hard FAILED as a Cargo
     build/runner break, never as an ordering failure or load artifact.
5. A malformed classifier invocation or capture-read failure is hard FAILED
   with its own classifier/infra label.

If one capture has both a marker-tagged inconclusive wait and an ordinary failed
test, FAILED wins. This honors AC8: a real named test failure is FAILED, while
an explicitly W4b-classified exhaustion is not falsely promoted to an ordering
violation.

## 7. Exit code vs. printed verdict — why the message is the deliverable

GNU Make flattens every nonzero recipe exit to code 2 at the make invocation
boundary (confirmed on GNU Make 4.3). Consequently, the target's internal
distinction between a hard FAILED run and an INCONCLUSIVE-only run cannot be
recovered from Make's exit code by a caller: make check-rust-race; echo $? will
show 2 for both.

The recipe's **printed output** is therefore the actual and sole source of
truth for distinguishing GREEN, INCONCLUSIVE, and FAILED. It must print one
line per run naming that run's verdict, then one final unambiguous aggregate
line before exiting. Humans and scripts consuming the target must read that
printed text; $? only distinguishes “everything was clean” (zero) from “not
everything was clean” (two).

The aggregate line must have three distinguishable shapes:

- GREEN: all K runs are green.
- INCONCLUSIVE-only: zero hard failures and one or more inconclusive runs;
  name the count of inconclusive runs.
- FAILED: one or more hard failures; name exactly which runs and tests
  hard-failed.

FAILED always wins in the aggregate. A batch containing both FAILED and
INCONCLUSIVE runs must not be reported as merely INCONCLUSIVE.

## 8. K-way shell orchestration

Add check-rust-race after check-rust-test, with build as a prerequisite. Before
load begins, build the classifier binary through Cargo under the target's scoped
bindgen export. The recipe then:

1. Validates RUST_RACE_RUNS and RUST_RACE_LOAD_JOBS are positive integers.
2. Creates one mktemp -d directory under the configured temporary root, holding
   run-N.log and run-N.status for every run.
3. Starts the configured busy-spin workers and records their PIDs.
4. Starts RUST_RACE_RUNS background subshells. Each runs one complete Cargo
   command for package solstone-core using --locked, --no-fail-fast,
   $(RUST_RACE_TEST_TARGETS), and -- --test-threads=1. Its combined stdout and
   stderr are captured to that run's log; its numeric result is written to that
   run's status file. A subshell records status instead of propagating it, so
   one failure cannot short-circuit the other K-1 runs.
5. Waits for all run subshells, then stops and reaps all load workers before
   classification so classification itself is not stressed.
6. Invokes the already-built classifier once per capture, prints a stable
   per-run line, counts GREEN/INCONCLUSIVE/FAILED, and emits the aggregate line
   described in section 7.
7. Uses EXIT/INT/TERM cleanup to kill and wait for both load workers and any
   running test subshells, then removes the private work directory. No capture
   or synthetic worker may survive interruption.

The target internally returns success only when all K are green. For an
inconclusive-only batch it returns a nonzero internal result, and for a hard
failure it returns a nonzero internal result; Make exposes either as 2, so the
printed final aggregate line is authoritative as described in section 7.

## 9. Makefile wiring and environment constraints

Add check-rust-race to the root .PHONY line and the target-scoped
BINDGEN_EXTRA_CLANG_ARGS export line at Makefile line 80. This is load-bearing:
the new target invokes Cargo directly and otherwise encounters the known Fedora
ffmpeg-sys-next limits.h failure.

All Cargo work for this wave, including the target and later validation, must
run via a Make target carrying BINDGEN_EXTRA_CLANG_ARGS or under an identical
explicitly supplied environment. Bare Cargo fails on this host before tests in
ffmpeg-sys-next bindgen. The Rust-conversion freeze permits new Rust Make gates.

## 10. AC2 deterministic-routing injection

For AC2, temporarily edit only
core/crates/solstone-core/tests/supervisor_app_stack.rs:332, in
launch_order_violation_accepts_order_and_rejects_inversion. Replace the
assertion that the known valid launch order has no violation with an assertion
that it has a violation.

This pure helper assertion fails every run independent of supervisor startup
timing, emits a conventional named libtest failure, and must make the aggregate
hard FAILED under the classifier. Revert it immediately after collecting the
routing evidence. It changes no product code and is separate from the
W4B_INCONCLUSIVE diagnostic edits.

## 11. Gate-hole-1: a new test escaping registration

The registered-list variable plus source-derived drift guard blocks silent
removal of any existing W4b-converted supervisor test and forces an explicit
decision when a future supervisor_*.rs test adopts the W4b module. It does not
catch a new raw-thread::sleep supervisor race test that never adopts W4b: that
file appears on neither side of the equality and can still escape
check-rust-race. This is the remaining gate-hole-1 risk.

Mitigate it in this wave with one concise convention in docs/testing.md, not
with a broad sleep-grep policy or rendered-document assertion: new
concurrency-sensitive supervisor integration tests must use
tests/support/await_outcome.rs, emit the W4B inconclusive marker at their
boundary, and be represented by RUST_RACE_TEST_TARGETS. docs/testing.md is the
appropriate contributor-facing testing guidance; CLAUDE.md is a symlink to the
broad developer guide and is less focused. This is proportionate to the known
gap and avoids speculative generalization to unrelated session or Convey tests.

## 12. Residual risk: will this target actually get run?

Naming check-rust-race in ci-under-poison's closing echo provides visibility,
not enforcement: it is prose a human can read and ignore. The drift guard
provides internal consistency—the registered list cannot silently drift from
its source-derived set—but it does not compel anyone to invoke
make check-rust-race or observe its printed verdict. A developer can touch a
registered supervisor test, never run this target, and keep the drift guard
green because the guard checks list membership rather than target execution or
success.

This residual risk is **not** closed by naming plus the drift guard alone.
Closing it further, for example with a periodic or scheduled nightly invocation
or a required manual review step when one of the three registered files changes,
is explicitly out of scope for this wave: the assignment forbids adding the
target to ci-under-poison or any make ci traversal. Track that enforcement
choice as a named follow-up rather than pretending the current design compels
execution.

## 13. Files and implementation order

1. Makefile: variables and scope comment, .PHONY, scoped bindgen-export list,
   check-rust-race recipe, and non-CI informational echo.
2. core/crates/solstone-core/Cargo.toml: classifier binary declaration.
3. New classifier binary and pure support/routing tests under
   core/crates/solstone-core/tests/; include the existing SIGKILL fixture as
   classifier input.
4. The three registered supervisor test files: add only the stable
   W4B_INCONCLUSIVE diagnostic to their existing inconclusive panic arm.
5. core/crates/solstone-core/tests/ci_gate_purity.rs: source/list equality
   drift guard plus target/echo wiring assertions; leave the pinned CI Cargo
   vector and existing serialization guard unchanged.
6. docs/testing.md: the narrow future-test convention.

## 14. Unrelated state

The prior solstone-core-convey-shell clippy authorization_gate red appeared
resolved during prep and is not credited to W4c. The
journal_native_process_contract state remained unconfirmed because the bare
Cargo reproduction hit the same bindgen environment failure; re-check it
through the Make-wrapped environment during implementation, without weakening
its assertions.
