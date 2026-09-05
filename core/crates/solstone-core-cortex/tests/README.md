# Cortex test-topology split — implementation report

## Driver shape

`solstone_core_cortex::test_hooks` (`src/test_hooks.rs`) is
`#[cfg(any(test, feature = "test-hooks"))]` + `#[doc(hidden)]`. Public
`lib.rs` product exports are unchanged.

Exported items a relocated test actually calls:

- `spawn_one` — sibling-selection, cwd (via controller), stdin-failure/reap
- `stop_group_with_grace` — process-group, graceful stop, forced cleanup, immediate stop
- `CortexState`, `CortexStore`, `Work` — construct and drive those cases
- `RunningUse` — shared `LaunchAuthority` from `CortexState::running` for `RunningUsesGuard` / immediate-stop
- `new_state` — constructs `CortexState` without exposing `Outbound` (its `new` cannot be `pub`)

Not exported (no relocating test calls them): `spawn_worker`, `ResolvedTalent`,
`write_valid_test_journal`, `RenewalHandle::production`, `BrainAdapter`.

### `stop_group` dead-code check

Production callers remain: the `spawn_one` launch terminate closure
(`process.rs`), which the timeout thread, `cancel_worker`, and
`run_until_with` immediate shutdown all reach via
`authority.lock().terminate(...)`. `terminate_and_reap` was deleted. The
10 s wrapper is **kept**:

`stop_group(pgid)` → `stop_group_with_grace(pgid, Duration::from_secs(10))`.

SIGTERM, 50 ms poll, SIGKILL, and the production 10 s constant are byte-identical
to baseline.

### Command capture

`build_talent_worker_command` (process.rs) holds construction from
`sibling_native_in_dir` through `process_group(0)`. `spawn_one` spawns the
returned `Command`. Unit tests record one `Command` in a `Vec` and inspect it
with `get_program` / `get_args` / `get_current_dir` / `get_envs`. No exec.

### Journal construction

Integration builds journals with `CortexStore::new` + local `package_roots`.
`write_valid_test_journal` stays `#[cfg(test)]` in `renewal.rs`.

## Absorption

Both identities below were merged into
`native_sibling_is_selected_and_lands_finish`.

| Original | Disposition | Assertions that survived |
|---|---|---|
| `native_sibling_is_reached_only_by_deliberate_spawn` | **absorbed into `native_sibling_is_selected_and_lands_finish`** (live half). Construction half is `deliberate_spawn_emits_exactly_one_native_sibling_command`. | Live: marker `x`, finish JSONL. Construction: program is `{executable_dir}/solstone-core`, argv `__talent-worker`, `SOL_FACET=override`. |
| `native_sibling_avoids_path_poison_and_lands_output` | **absorbed into `native_sibling_is_selected_and_lands_finish`** | Poison marker absent; finish JSONL lands. PATH poison still injected via request `env`. |

## All 34 owned identities

| Identity | Original observable | Final target | Disposition | Scanner |
|---|---|---|---|---|
| `captured_process_group_survives_direct_child_reap` | descendant outlives reaped direct child in same pgid | `captured_process_group_survives_direct_child_reap` | **moved** | 3 rows deleted (after-snapshot confirmed) |
| `cogitate_terminal_usage_writes_record` | token record for cogitate | `src/process.rs` same test | **retained** (already compliant) | scheduling retained |
| `generate_terminal_usage_does_not_write_record` | no token file for generate | `src/process.rs` same test | **retained** (already compliant) | scheduling retained |
| `native_sibling_avoids_path_poison_and_lands_output` | poison unused; finish lands | `native_sibling_is_selected_and_lands_finish` | **absorbed** | 2 rows deleted |
| `native_sibling_is_reached_only_by_deliberate_spawn` | native sibling + argv/env + finish | unit `deliberate_spawn_emits_exactly_one_native_sibling_command` + integration `native_sibling_is_selected_and_lands_finish` | **split** then live **absorbed** | 2 rows deleted |
| `non_json_stdout_is_log_only_info_and_error_defaults_terminal` | handle_stdout | `src/process.rs` same test | **retained** (already compliant) | scheduling retained |
| `renewal_cycle_never_reaches_injected_interpreters_but_deliberate_spawn_does` | renewal does not touch interpreters; spawn does | same **name** in `src/process.rs` (renewal + one captured Command, no exec) + live spawn absorbed into `native_sibling_is_selected_and_lands_finish` | **split** | clock deleted; scheduling retained |
| `spawn_and_read_child_cwd` | helper for three cwd tests | integration `run_cwd_case` | **moved** (helper deleted from src) | 2 rows deleted |
| `stdin_write_failure_terminates_and_reaps_spawned_child` | spawn_one Err + child dead | same name in integration | **moved** | scheduling deleted |
| `stdout_augmentation_fills_only_absent_values` | handle_stdout fill | `src/process.rs` same test | **retained** (already compliant) | scheduling retained |
| `stop_group_waits_for_a_graceful_exit_within_ten_seconds` | TERM wait then exit | replaced by `stop_group_records_term_and_does_not_kill_a_responsive_child` + `stop_group_records_term_then_kills_an_ignoring_child` | **moved** (criterion 10 receipts replace wall-clock) | 3 rows deleted |
| `brain_state_read_failure_does_not_stop_renewal_worker` | injected Wait | `src/renewal.rs` same test | **retained** (already compliant) | scheduling retained |
| `fake` | FakeBrain helper | `src/renewal.rs` same helper | **retained** | scheduling retained |
| `fingerprint_read_failure_does_not_stop_renewal_worker` | injected Wait | `src/renewal.rs` same test | **retained** (already compliant) | scheduling retained |
| `handle` | RenewalHandle helper | `src/renewal.rs` same helper | **retained** | scheduling retained |
| `mismatch` | dispatch helper | `src/renewal.rs` same helper | **retained** | scheduling retained |
| `outbound_send_failure_does_not_stop_renewal_worker` | injected Wait | `src/renewal.rs` same test | **retained** (already compliant) | scheduling retained |
| `run_two_worker_iterations` | worker helper | `src/renewal.rs` same helper | **retained** | scheduling retained |
| `shipped_stderr_sink_renders_and_writes_the_exact_renewal_line` | exact stderr line | `src/renewal.rs` same test | **retained** (already compliant) | scheduling retained |
| `startup_refresh_predicate_fires_four_cases_and_suppresses_three_cases` | predicate cases | `src/renewal.rs` same test | **retained** (already compliant) | scheduling retained |
| `startup_refresh_send_failure_is_diagnosed` | SendError | `src/renewal.rs` same test | **retained** (already compliant) | scheduling retained |
| `worker_caps_a_multi_hour_planned_wait_at_sixty_seconds` | injected Wait records 60s | `src/renewal.rs` same test | **retained** (already compliant) | scheduling retained |
| `dispatch_filters_only_at_service_boundary` | dispatch ignore | `src/service.rs` same test | **retained** (already compliant) | scheduling retained |
| `renewal_handle` | production handle helper | `src/service.rs` same helper | **retained** | scheduling retained |
| `running_state` | state + real `/bin/sleep` `LaunchAuthority` | helper and its drain/immediate-stop-without-signaling tests in `tests/cortex_child_supervisor.rs` (`test-hooks`) | **moved** (real spawn cannot live under `--lib`) | process/scheduling now in integration |
| `service_starts_one_renewal_worker_once_even_when_start_requested_twice` | one worker | `src/service.rs` same test | **retained** (already compliant) | scheduling retained |
| `startup_refresh_is_emitted_before_the_single_renewal_worker_starts` | lifecycle order | `src/service.rs` same test | **retained** (already compliant) | scheduling retained |
| `compare_and_take_finalization_allows_only_one_terminal_owner` | CAS finalize | `src/state.rs` same test | **retained** (already compliant) | scheduling retained |
| `failed_spawn_send_terminalizes_claim_and_leaves_drain_idle` | dropped spawn rx | `src/state.rs` same test | **retained** (already compliant) | scheduling retained |
| `immediate_stop_terminalizes_queued_claim_without_starting_it` | queued abort | `src/state.rs` same test | **retained** (already compliant) | scheduling retained |
| `nameless_request_is_silent_except_for_stderr_diagnostic` | no name | `src/state.rs` same test | **retained** (already compliant) | scheduling retained |
| `resolved_talent_state_is_available_until_finalization` | resolved map | `src/state.rs` same test | **retained** (already compliant) | scheduling retained |
| `status_reports_queue_depth_without_a_running_use` | status event | `src/state.rs` same test | **retained** (already compliant) | scheduling retained |
| `temporary_link_names_are_thread_unique` | unique tmp names | `src/storage.rs` same test | **retained** (already compliant) | scheduling retained |

## Named helpers and unflagged callers

| Symbol | Landing |
|---|---|
| `spawn_and_read_child_cwd` | deleted from src; logic in integration `run_cwd_case` |
| `running_state` | `tests/cortex_child_supervisor.rs` (`test-hooks`); constructs a real `LaunchAuthority` |
| `fake`, `handle`, `mismatch`, `run_two_worker_iterations` | retained in `src/renewal.rs` |
| `renewal_handle` | retained in `src/service.rs` |
| `cogitate_with_declared_journal_cwd` | `declared_cogitate_runs_in_journal_root` |
| `generate_talent_does_not_set_child_cwd` | `generate_inherits_controller_fixture_directory` |
| `cogitate_without_declared_cwd_does_not_set_child_cwd` | `undeclared_cogitate_inherits_controller_fixture_directory` |
| `drain_keeps_running_use_alive_until_its_own_exit_then_becomes_idle` | split: `drain_becomes_idle_after_finish` (integration) + `drain_keeps_running_use_alive_until_its_own_exit` (integration) |
| `immediate_stop_terminalizes_queue_and_signals_running_group` | split: `immediate_stop_returns_running_uses_without_signaling` (integration) + `immediate_stop_signals_the_running_group` (integration) |

## Boundary reconciliation

- Before (owned cortex rows): **43**
- After: **28**
- Added owned rows: **0**
- Deleted owned rows (15), each confirmed absent in
  `/var/tmp/tbeily47-cortex-topology/after-snapshot.txt`:

1. `…captured_process_group_survives_direct_child_reap::clock`
2. `…captured_process_group_survives_direct_child_reap::process`
3. `…captured_process_group_survives_direct_child_reap::scheduling`
4. `…native_sibling_avoids_path_poison_and_lands_output::clock`
5. `…native_sibling_avoids_path_poison_and_lands_output::scheduling`
6. `…native_sibling_is_reached_only_by_deliberate_spawn::clock`
7. `…native_sibling_is_reached_only_by_deliberate_spawn::scheduling`
8. `…renewal_cycle_never_reaches_injected_interpreters_but_deliberate_spawn_does::clock`
9. `…spawn_and_read_child_cwd::clock`
10. `…spawn_and_read_child_cwd::scheduling`
11. `…stdin_write_failure_terminates_and_reaps_spawned_child::scheduling`
12. `…stop_group_waits_for_a_graceful_exit_within_ten_seconds::clock`
13. `…stop_group_waits_for_a_graceful_exit_within_ten_seconds::process`
14. `…stop_group_waits_for_a_graceful_exit_within_ten_seconds::scheduling`
15. `…running_state::process`

Non-owned rows were not edited. The original validation run recorded 625
findings before rebase. On the final landed parent, `solstone-ci validate`
moves from 192 to 193 Cargo integration targets and from 311 to 296 findings;
package scopes (126) and named legs (13) are unchanged.

## `/bin/sh`

Eliminated as a fixture child. Process-group uses a Rust worker that spawns an
inheriting-pgid grandchild. Ignore-TERM uses `tokio::signal` (safe; workspace
`unsafe_code = forbid` blocks `sigaction` / `SigIgn` in this crate). A
non-exiting handler is required so the child can write the TERM receipt before
SIGKILL (criterion 10).

## Validation

Run directly on the settled tree. Wave 2 boundary: focused deterministic
library tests, static checks, topology/registry validation, exact-target
compile, and discovery only.

| # | Command | Exit | Evidence |
|---|---------|------|----------|
| 1 | `cargo fmt --all -- --check` | 0 | clean (re-run after the two lint fixes below, also 0) |
| 2 | `cargo clippy -p solstone-core-cortex --lib --bins -D warnings` | 0 | default features; no `unreachable_pub` from the visibility widening |
| 3 | `cargo build -p solstone-core-cortex` | 0 | see "default-build absence" below |
| 4 | `cargo test -p solstone-core-cortex --lib -- --test-threads=1` | 0 | 69 passed; 0 failed; 0 ignored |
| 5 | `solstone-ci validate` | 0 | Original run: 156 Cargo integration targets, 126 package scopes, 13 named legs, 625 routine-boundary findings. Final landed rerun: 193, 126, 13, 296; its parent is 192, 126, 13, 311. |
| 6 | `cargo clippy -p solstone-core-cortex --all-targets --features test-hooks -D warnings` | 0 | after two minimal fixes (below) |
| 7 | `cargo test -p solstone-core-cortex --test cortex_child_supervisor --features test-hooks --no-run` | 0 | links `cortex_child_supervisor-94b7b40f7affb01f` |
| 8 | `cargo test ... --features test-hooks -- --list` | 0 | **10 tests, 0 benchmarks**; no body executed |

Command 8 discovered exactly the criterion-9 set:
`captured_process_group_survives_direct_child_reap`,
`declared_cogitate_runs_in_journal_root`,
`drain_keeps_running_use_alive_until_its_own_exit`,
`generate_inherits_controller_fixture_directory`,
`immediate_stop_signals_the_running_group`,
`native_sibling_is_selected_and_lands_finish`,
`stdin_write_failure_terminates_and_reaps_spawned_child`,
`stop_group_records_term_and_does_not_kill_a_responsive_child`,
`stop_group_records_term_then_kills_an_ignoring_child`,
`undeclared_cogitate_inherits_controller_fixture_directory`.

After the process-authority migration, `drain_becomes_idle_after_finish` and
`immediate_stop_returns_running_uses_without_signaling` also live in this
file: a `LaunchAuthority` requires a real spawn, which the routine `--lib`
harness must not author.

### Lint fixes applied during validation

1. `tests/cortex_child_supervisor.rs` `poll_file` — `clippy::collapsible_if`;
   collapsed the nested `if` into a let-chain. No behavior change.
2. `tests/bin/worker.rs` `process_group` — `clippy::zombie_processes`. Scoped
   `#[allow]` with justification: the grandchild is deliberately left unwaited
   so the direct child is reaped while the descendant survives in the inherited
   process group. That is the observable the case asserts; the integration
   target's guard tears the whole group down afterwards.

### `state.rs` / `storage.rs`

Visibility widening only (`pub(crate)` → `pub` on `CortexStore`, `Work`,
`RunningUse`, `CortexState`, and the fields the driver needs), so
`test_hooks` can re-export them. Both `mod state;` and `mod storage;` remain
private in `lib.rs`, and `lib.rs`'s `pub use` line is unchanged, so the default
public surface does not drift. No test body in either file was altered:
`git diff core/crates/solstone-core-cortex/src/storage.rs` contains zero
references to `temporary_link_names_are_thread_unique`.

### Default-build absence of the driver

The default build unit's dependency file
(`core/target/debug/deps/solstone_core_cortex-55bceb65d7a42847.d`) lists **0**
references to `test_hooks.rs`; the `--features test-hooks` build units list 3–4.
The module is not compiled without the feature.

### Not executed

No `make` target of any kind, no workspace-wide `cargo test`, and no
`cortex_child_supervisor` test body were executed. Real-boundary and canonical
execution is VPE's post-landing work.
