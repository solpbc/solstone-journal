# Local Permit Yield On Sol Tool

Historical design note, August 2026: this records the permit-yield design for
the deleted OpenHands cogitate runtime. Native one-shot cogitate replaced that
runtime; the OpenHands-specific scope, decisions, file plan, and tests below
are retained as design history, not current implementation guidance.

## Scope

This note covers local cogitate runs that already hold a governed local
inference permit and then invoke the OpenHands `sol` tool. It changes only the
permit lifecycle around that `sol` tool call.

It does not change cloud providers, confidential local endpoints with no
governed `parallel_slots`, raw-read tools, bundled-local condenser calls,
provider/model resolution, cogitate policy, command policy, on-disk admission
artifacts, or telemetry schema.

Ground truth from the installed OpenHands SDK:

- `conversation.pause()` is not safe from a tool worker thread in the non-ACP
  `arun` path because `arun` holds `self._state` across
  `await agent.astep(...)` and `pause()` reacquires the same state lock.
- `conversation.interrupt()` is safe from a worker thread while `arun` is
  active: it sets a thread-safe cancellation token and schedules task
  cancellation with `loop.call_soon_threadsafe`.
- `arun` catches that `CancelledError`, emits synthetic orphaned-action
  observations, sets status to `PAUSED`, and returns normally. The next LLM
  request is not issued.
- `ParallelToolExecutor._run_safe()` converts every `Exception` from a tool
  executor into an `AgentErrorEvent`. A normal exception from `SolExecutor`
  cannot terminate the run by itself. Escaping a `BaseException` bypasses SDK
  cleanup and is rejected.
- Multiple concurrent `sol` calls are not reachable today: non-native local
  tool parsing yields one tool call, agent tool concurrency defaults to `1`,
  and undeclared `sol` resources resolve to a tool-wide mutex.
- `_run_command()` runs a real child process and inherits environment, so the
  nested `sol` process contends on the same
  `health/local-inference-admission/` flock files.

## Decisions

### D1. Lease Seam

Decision: introduce one local admission lease object owned by
`local.run_cogitate()` and pass it into the OpenHands provider as a keyword-only
argument.

Public shape:

- `local.run_cogitate()` creates a lease only when it actually acquired a
  governed local permit: bundled local, or non-confidential BYO local endpoint
  with `parallel_slots`.
- `openhands.run_cogitate(config, on_event=None, *, slot_lease=None)` accepts the
  lease.
- `_build_sol_tools(..., slot_lease=None)` passes it into `SolExecutor`.
- Cloud callers pass nothing. Provider registry routing remains unchanged:
  cloud providers call `solstone.think.providers.openhands`, while `local`
  calls `solstone.think.providers.local`.
- Confidential local endpoints with `parallel_slots is None` create no lease and
  keep current behavior.

Ownership stays with `local.run_cogitate()`. The current `permit.release()` in
its `finally` becomes `slot_lease.close()` when a lease exists. OpenHands and
the executor may yield and reacquire the lease, but they do not own final
release.

Implementation detail: after creating the lease, clear the old local `permit`
variable or route all telemetry/final release reads through the lease. Do not
leave both `permit.release()` and `lease.close()` active for the same permit.

### D2. Lease Location And Shape

Decision: implement the lease in `solstone/think/providers/local_admission.py`,
next to `LocalPermit`, because it is a small lifecycle wrapper around the
existing admission primitive and needs the same cancellation semantics.

Lease state:

- `capacity: int`
- `deadline: float`, an absolute `time.monotonic()` deadline captured by
  `local.run_cogitate()` as `started + timeout`
- current `LocalPermit | None`
- `threading.Lock`
- `threading.Event` used to cancel pending reacquire
- closed flag
- initial telemetry values copied from the first permit

Methods:

- `yield_slot()`: synchronous, worker-thread safe. It removes the current permit
  under the lock and releases it outside the lock. If the lease is closed, it is
  a no-op after releasing any held permit. If the lease is already yielded, raise
  loudly because this means the SDK concurrency contract changed.
- `reacquire()`: synchronous. It computes remaining time from the absolute
  deadline and calls `acquire_local_slot(capacity, remaining,
  cancel_event=...)`. On success it stores the new permit under the lock. If the
  lease was closed or cancelled before storage, it immediately releases the
  newly acquired permit and raises the cancellation exception. Deadline expiry
  raises `LocalAdmissionTimeout`.
- `cancel_pending_reacquire()`: sets the cancel event without releasing a
  currently held permit. This is what `SolExecutor.interrupt()` uses.
- `close()`: sets closed and the cancel event, releases any currently held
  permit, and makes any in-flight reacquire drop its ticket and stop. If a
  reacquire wins a slot in the race with `close()`, the post-acquire closed
  check releases that permit immediately.

The lease should be single-flight. `SolExecutor` should also hold a private
`threading.Lock` around `yield -> command -> reacquire` so a future SDK bump
cannot run two `sol` actions from the same parent permit at the same time.

### D3. Failed Reacquire Termination

Decision: failed reacquire is a terminal local admission failure, but the tool
executor must stop the SDK run with `interrupt()` rather than by raising the
exception directly.

Flow in `SolExecutor.__call__`:

1. Resolve and validate the command as today.
2. If there is no lease, run the command exactly as today.
3. With the executor's lease lock held:
   - call `slot_lease.yield_slot()` immediately before `_run_command()`;
   - call `_run_command()`;
   - call `slot_lease.reacquire()` in a `finally`, before returning any
     observation or re-raising any command exception.
4. If `reacquire()` raises `LocalAdmissionTimeout`:
   - store that exact exception on the executor behind a lock;
   - call `conversation.interrupt()` before returning, so the task cancellation
     is queued before the worker future completes;
   - return an error observation for history consistency.
5. If `reacquire()` raises `LocalAdmissionCancelled`, swallow it and return the
   observation already produced by the command, or an internal error observation
   if the command did not produce one. `LocalAdmissionCancelled` is not terminal.

The invariant is: once the parent permit is yielded, control cannot leave
`SolExecutor.__call__` unless the parent permit is held again or a terminal
`LocalAdmissionTimeout` marker has been stored and `conversation.interrupt()`
has been called. Policy-denied and read-budget-exhausted returns happen before
the yield and therefore never enter this path.

Flow in `openhands.run_cogitate()`:

1. Keep the `SolExecutor` reference returned by `_build_sol_tools()`.
2. Bind the constructed `Conversation` back onto the executor as a fallback.
   The installed SDK passes `conversation` into tool calls, but this pin avoids a
   silent behavioral change if the SDK call signature moves.
3. After `arun` returns and before wall-clock, cost, turn, stuck, paused, or
   no-output classification, check `sol_executor.take_terminal_error()`.
4. If present, call `conversation.close()` and re-raise the exact stored
   `LocalAdmissionTimeout`.

Exact type preservation:

- Bundled: `local.run_cogitate()` already treats `LocalAdmissionTimeout` as a
  timeout and, because `local_endpoint_reason_copy("local_queue_timeout")`
  returns `None`, re-raises the original exception.
- Governed BYO: `classify_byo_cogitate_error(exc)` returns `None` for
  `LocalAdmissionTimeout`; `getattr(exc, "reason_code")` supplies
  `local_queue_timeout`; `local_endpoint_reason_copy("local_queue_timeout")`
  returns `None`; the original exception is re-raised.
- Tests should pin both paths so a future endpoint-copy mapping cannot wrap this
  exception into `LocalProviderError`.

The executor needs the live `Conversation`. The installed SDK supplies it via
`tool(action_event.action, conversation)` inside `_execute_action_event()`.
Implementation should still bind the conversation onto the executor after
construction and use `conversation or self._conversation` in `__call__`.

### D4. Budget Arithmetic

Decision: reacquire uses the same absolute budget as the parent local cogitate
run.

Deadline:

- `local.run_cogitate()` already captures `started = time.monotonic()` and
  `timeout = config["timeout_seconds"] or 600`.
- The lease deadline is `started + timeout`.
- Every reacquire computes `remaining = deadline - time.monotonic()` at the
  moment it starts waiting. If `remaining <= 0`, it raises
  `LocalAdmissionTimeout` immediately.

Composition:

- `_SHELL_TIMEOUT_SECONDS` remains 30 seconds. The child command can consume up
  to that amount while the parent permit is yielded.
- `openhands.run_cogitate()` still derives an in-process wall-clock deadline
  inside the same budget: `timeout_seconds - 30s`, or half the timeout for very
  small budgets.
- If the shell command plus queue wait leaves no budget, the parent terminates
  as `LocalAdmissionTimeout`.
- If reacquire blocks past the OpenHands wall-clock deadline, `run_task.cancel()`
  cancels `arun`; `_arun_safe()` calls `SolExecutor.interrupt()`; the lease
  cancel event wakes or bounds the blocking reacquire; `local.run_cogitate()`'s
  `finally` closes the lease. Any late-acquired slot is released by the
  post-acquire closed check.

The parent is allowed to make another model request only after
`slot_lease.reacquire()` has succeeded and stored a current permit.

### D5. Admission Cancellation

Decision: extend only the synchronous admission primitive with optional
cancellation, because the new blocking reacquire runs in a worker thread and
uses sync admission.

Public shape:

- `acquire_local_slot(capacity, timeout_s, *, exclusive=False,
  cancel_event: threading.Event | None = None)`

Semantics:

- If `cancel_event` is set before ticket creation, raise
  `LocalAdmissionCancelled`.
- If it is set while queued, raise `LocalAdmissionCancelled`.
- If a permit is acquired and the event is set before returning, release the
  permit immediately and raise `LocalAdmissionCancelled`.
- Tickets are dropped in the existing `finally` block for timeout, cancellation,
  and unexpected errors.
- `LocalAdmissionCancelled` is not a provider readiness reason and should not
  be mapped to owner-facing copy. It is a plain internal `Exception`, not a
  `LocalAdmissionTimeout` subclass, has no `reason_code`, is not exported from
  `local_admission.__all__`, and is never stored as a terminal marker.

The wait loop can keep the existing 25 ms poll cadence, but should use
`cancel_event.wait(sleep_s)` when a cancel event is present so close wakes the
worker promptly. Async admission does not need this parameter for this design.

`SolExecutor.interrupt()` should override the base no-op and call
`slot_lease.cancel_pending_reacquire()` when a lease exists. It does not need to
kill the child process in this design; `_run_command()` already has a 30 second
subprocess timeout, and any later reacquire will see the cancelled lease and
exit without taking or leaking a parent permit.

`slot_lease.cancel_pending_reacquire()` only sets cancellation state; it does
not release a currently held permit. `slot_lease.close()` sets closed state,
cancels any pending reacquire, and releases a currently held permit. The
separation matters because the SDK can interrupt a worker while
`local.run_cogitate()` still owns cleanup.

### D6. Telemetry

Decision: keep telemetry schema unchanged and report the original top-level
parent admission values.

- `queue_wait_ms` becomes `slot_lease.initial_queue_wait_ms`.
  Justification: this preserves the field's existing meaning as the wait for
  the cogitate run's top-level admission; internal yield/reacquire waits remain
  reflected in wall time and terminal reason.
- `admission_slot` becomes `slot_lease.initial_slot_index`.
  Justification: one cogitate row represents one parent run, so the stable
  original admission slot is less misleading than a last slot after a temporary
  yield.

Do not add fields for yield count, reacquire queue time, or final slot. If a
future observability pass needs that detail, it should be a separate schema
change.

### D7. Unchanged Paths

These stay put:

- Raw-read tools in `read_tools.py` keep the permit held for their whole tool
  execution. They do not receive the lease.
- The bundled-local condenser keeps using the parent-held permit.
- Policy classification and read budgets are unchanged.
- `parallel_slots` resolution is unchanged.
- Local endpoint error copy is unchanged.
- No new on-disk artifacts are introduced, so no layer-ownership row is needed.

## Docs Correction

Update `docs/PROVIDERS.md` in the "Local admission and bundled inference
telemetry" section. Replace the current statement:

- `Cogitate holds one permit for its run because the OpenHands SDK owns its internal multi-turn HTTP calls; this is conservative and avoids an uncontrolled second path to the same server.`

with:

- `Cogitate holds one parent permit across model turns, but temporarily yields that permit while the OpenHands sol tool runs a nested sol child process. The parent reacquires through the same FIFO admission pool before any further model request; failure to reacquire is a terminal local_queue_timeout.`

Keep the surrounding claims about governed lanes, confidential bypass, and
bundled-only telemetry.

## File-By-File Diff Plan

### `solstone/think/providers/local_admission.py`

- Add `LocalAdmissionCancelled`.
- Add optional `cancel_event` support to `acquire_local_slot()`.
- Add the lease class beside `LocalPermit`.
- Keep existing `LocalPermit` and exclusive-mode behavior intact.

### `solstone/think/providers/local.py`

- In `run_cogitate()`, after initial governed acquire, wrap the permit in the
  lease with `capacity.parallel_slots` or `endpoint.parallel_slots` and
  `deadline=started + timeout`.
- Pass `slot_lease=lease` to `openhands.run_cogitate()`.
- Replace final `permit.release()` with `lease.close()` for lease-backed runs.
- Route bundled cogitate telemetry through lease initial telemetry properties.
- Preserve exact `LocalAdmissionTimeout` behavior for bundled and governed BYO.

### `solstone/think/providers/openhands.py`

- Add keyword-only `slot_lease=None` to `run_cogitate()`.
- Thread the lease through `_build_sol_tools()` and `SolExecutor`.
- Keep and bind the `SolExecutor` reference after creating the `Conversation`.
- In `SolExecutor.__call__`, use the lease around `_run_command()` and terminal
  marker plus `conversation.interrupt()` on reacquire timeout.
- Add `SolExecutor.interrupt()` to cancel pending reacquire.
- Add helper methods on `SolExecutor` for storing and retrieving the terminal
  exception.
- Check the terminal marker immediately after `arun` returns and before existing
  wall-clock/cost/turn/stuck classification.

### `docs/PROVIDERS.md`

- Apply the wording correction above.

### Tests

- Update local-provider and admission tests for the new lease.
- Expand real SDK shape pins in `tests/test_openhands_sdk_shape.py`.
- Keep fake OpenHands tests only for provider plumbing and event translation;
  do not rely on them for SDK lock, interrupt, or worker-thread behavior.

## Implementation Sequence

1. Add cancel-aware sync admission and the lease in `local_admission.py`.
2. Refactor `local.run_cogitate()` to create, pass, close, and report through
   the lease without changing behavior when no permit is acquired.
3. Thread the keyword-only lease through OpenHands tool construction.
4. Update `SolExecutor` for yield/reacquire, terminal marker, bound
   conversation fallback, and interrupt cancellation.
5. Add the marker check in `openhands.run_cogitate()` before existing terminal
   classifications.
6. Update `docs/PROVIDERS.md`.
7. Add tests in the order below, starting with lower-level admission tests, then
   local provider tests, then real SDK shape pins.

## Test Plan

### `tests/test_local.py`

- Replace `test_run_cogitate_byo_acquires_permit_and_records_no_telemetry`.
  The fake `openhands.run_cogitate()` should accept `slot_lease`, call
  `slot_lease.yield_slot()`, prove a nested `acquire_local_slot(1, 0.1)`
  succeeds, release the nested permit, call `slot_lease.reacquire()`, and return
  `"ok"`. On the old tree this fails because the parent never yields and the
  nested acquire times out.
- Add bundled parity for the same yield/reacquire behavior, with telemetry still
  written once and using the lease's initial queue wait and slot.
- Add a capacity-1 "parent holds again before resume" test. After
  `slot_lease.reacquire()` and before the fake OpenHands run returns, a
  competing `acquire_local_slot(1, 0.03)` must time out. This proves the parent
  has reacquired before any possible next model turn.
- Add sol command failure coverage by monkeypatching `_run_command()` to return
  `is_error=True`; assert the lease is reacquired, no lock leaks, and the result
  stays an error observation rather than an ungoverned resume.
- Add sol command timeout coverage by monkeypatching `_run_command()` to return
  the same shape it returns on `subprocess.TimeoutExpired`; assert reacquire and
  no leak.
- Add failed reacquire coverage: yield the lease, queue or hold capacity so
  `reacquire()` exceeds the remaining budget, and assert
  `local.run_cogitate()` raises the exact `LocalAdmissionTimeout` with
  `reason_code == "local_queue_timeout"`.
- Add BYO exact-type coverage for the failed reacquire path. Existing initial
  queue timeout coverage proves the pre-OpenHands path; this pins the
  post-`sol` reacquire path.

### `tests/test_local_admission.py`

- Add `test_sync_queue_cancel_event_drops_ticket`: hold capacity, start
  `acquire_local_slot(..., cancel_event=event)` in a thread, wait until its
  ticket exists, set the event, assert the thread exits with
  `LocalAdmissionCancelled`, no wait tickets remain, and a new acquire succeeds.
- Add `test_cancel_event_after_acquire_releases_permit`: arrange the event to be
  set at the acquire-return boundary, then assert the acquired permit is released
  and capacity is fully restored.
- Add lease-specific close coverage: start `lease.reacquire()` while capacity is
  held elsewhere, wait for the wait ticket, call `lease.close()`, assert no stale
  ticket and full capacity restored.
- Add FIFO coverage for yielded parent: with parent yielded, an unrelated waiter
  that already has the oldest ticket must acquire before the parent reacquire.
  Synchronize on admission ticket files and thread events with bounded waits,
  not sleeps.
- Add a cross-process nested acquire test using `subprocess.run([sys.executable,
  "-c", ...])` with `SOLSTONE_JOURNAL` pointing at the tmp journal. The parent
  lease yields before the child starts; the child performs a real
  `acquire_local_slot()` against the same admission directory and exits `0`.
- Add capacity-2 two-parent coverage: create two leases holding capacity, yield
  both, run two nested cross-process acquires, then reacquire both parents.
  Synchronize on tickets/events with bounded waits and assert full capacity is
  restored.

### `tests/test_openhands_provider.py`

- Add fake-provider plumbing tests for:
  - `openhands.run_cogitate()` passes the lease into `_build_sol_tools()`;
  - `SolExecutor` binds the live conversation fallback;
  - a stored terminal timeout marker is checked before wall-clock, cost, turn,
    stuck, paused, or no-output classification.
- Keep these tests focused on solstone provider logic only. Do not use fakes to
  prove real SDK interrupt or `_run_safe` behavior.

### `tests/test_openhands_sdk_shape.py`

Use the installed SDK, not `tests/openhands_fakes.py`.

- Add `test_interrupt_from_tool_worker_stops_before_next_llm_completion`.
  Build a real `Conversation` with `openhands.sdk.testing.TestLLM`. First
  scripted response calls a custom test tool; the tool runs on the executor
  worker, calls `conversation.interrupt()`, and returns an observation. A second
  scripted assistant response is available. Assert `arun()` returns paused and
  `TestLLM` consumed exactly one completion.
- Add `test_run_safe_exception_becomes_agent_error_and_loop_continues`.
  Use a real tool executor that raises `RuntimeError` on the first call and a
  `TestLLM` script with a second response. Assert a second LLM completion is
  consumed and an `AgentErrorEvent` exists. This proves marker plus interrupt is
  necessary.
- Add `test_default_tool_concurrency_and_undeclared_tool_mutex_shape`.
  Assert `Agent(...).tool_concurrency_limit == 1`; assert a plain
  `ToolDefinition.declared_resources()` returns `declared=False`; assert
  `ParallelToolExecutor._resolve_lock_keys()` maps that to `["tool:<name>"]`.
  If private helper access is too brittle, assert through two same-tool actions
  with blocking executors and a max-workers executor that they serialize.

These are feasible with the installed SDK because `TestLLM` exists and supports
scripted tool-call messages. The private lock-key assertion is the only brittle
part; prefer a behavior test if construction overhead is acceptable.

### Raw-Read Tool Coverage

- Add a local cogitate test where a raw-read tool executes while a competing
  acquire attempts capacity 1. Because raw-read tools never receive the lease,
  the competing acquire must time out until the tool finishes. This can be a
  provider plumbing test with `build_read_tools()` monkeypatched to return a
  simple blocking read tool, or a real SDK test if setup remains small.

## Risks And Open Questions

- `SolExecutor.interrupt()` will not terminate an already-running child
  subprocess. The child is still bounded by `_SHELL_TIMEOUT_SECONDS` and owns its
  own admission lifecycle. This is simpler and satisfies the no-leak constraint,
  but it means a parent wall-clock timeout can leave the nested child running for
  up to 30 seconds.
- The terminal marker relies on `conversation.interrupt()` being queued before
  the worker future completes. The executor must call `interrupt()` before
  returning the error observation.
- The lease is intentionally single-flight. If a future SDK permits concurrent
  same-executor `sol` calls despite the current parser/defaults/mutex, tests
  should fail loudly rather than allowing an ungoverned resume.
- Reacquire wait time is not represented separately in telemetry. That is a
  deliberate schema-preserving choice.
- I verified the installed SDK exposes `TestLLM`, so the real SDK tests are
  feasible. I did not prototype those tests in this design pass.
