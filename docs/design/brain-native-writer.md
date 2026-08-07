# Wave 2 native brain writer

## Summary and scope

Wave 2 moves the three write lanes for `health/brain.json` from
`solstone.think.providers.brain_state` into native Rust: refresh completion (and
abandonment), SPP prerequisite renewal, and runtime-failure recording.  The
public entry remains the installed `solstone-core` binary as `solstone-core
brain <verb>`; Python becomes a strict subprocess/wire transport client with no
in-process fallback.  The native state model must retain the current record
schema and projection behavior (`solstone/think/providers/brain_state.py:429-520`,
`core/fixtures/local_contract.json:2-10`).

This document is a design decision, not an implementation.  It deliberately
does not change product code, fixtures, or `docs/PORTING.md`.

## 1. Crate boundary

Put the writer in the existing `solstone-core-brain` crate.  Add a private
writer module beside its read/inspection modules, expose only the typed verbs
needed by `solstone-core`, and keep the CLI parsing/exit-code translation in
the `solstone-core` binary.

This is the smallest ownership boundary: brain inspection, the state-path and
fingerprint helpers, and `probe_file_lease_held` already live in that crate
(`core/crates/solstone-core-brain/src/inspect.rs:55-84`), and it already has the
approved `solstone-core-journal-io` dependency.  Unlike journal configuration,
there are not independent writer consumers which justify a separate
`*-write` crate.  The config crate's split is a CAS API used outside its reader
crate (`core/crates/solstone-core-journal-config-write/src/commit.rs:108-147`);
the brain writer is the one authority for the brain domain.

No new wrapper name is added to `core/deny.toml`: `solstone-core-brain` is
already in the `solstone-core-journal-io` wrappers list
(`core/deny.toml:30`).  Implementation must update that entry's explanatory
phrase from “read-only brain inspector” to describe the native brain authority;
the wrappers list itself is unchanged.

The CLI follows the existing aggregate-binary pattern: add `Command::Brain`, a
`BrainCommand` verb enum, and `parse_brain` to
`core/crates/solstone-core-cli/src/lib.rs` (the JournalConfig precedent is at
lines 9-39 and 115-142), then dispatch it to `run_brain` in
`core/crates/solstone-core/src/main.rs` (lines 55-90).  Do not extend the
inspect-only `solstone-brain` standalone binary.

## 2. Native state writer and lease

The writer has typed operations corresponding directly to the old ownership
surface:

1. begin/finish/abandon refresh, with the permit carried by the caller while
   its lease remains held;
2. begin/finish/abandon SPP prerequisite renewal; and
3. `record_runtime_failure` with its accepted/rejected result rather than an
   exception-only API.

Each mutation follows the established durable pattern: acquire the appropriate
guard, re-read under the state lock, validate the record/fingerprint/fences,
derive the complete replacement record, and call `journal_io::write_json` with
mode `0o600`.  `write_json` adds the final newline and delegates to durable
atomic replacement (`core/crates/solstone-core-journal-io/src/atomic.rs:155-170`);
it is the required publication primitive, not an ad-hoc temporary file.

### Lifetime lease primitive

Add the general primitive to `solstone-core-journal-io`, in a new `lease`
module exported by that crate, rather than make it brain-specific.  Python has
the same split: the existing lease belongs to `think.journal_io`, not to brain
state (`solstone/think/journal_io/lease.py:84-118`).  This also keeps the
domain writer from owning a reusable I/O policy.

The public shape is:

```text
pub struct FileLease { path: PathBuf, _guard: Flock<File> }
pub struct LeaseOptions { attempts: usize, retry_max: Duration, mode: u32 }
pub fn acquire_file_lease(path: impl AsRef<Path>, options: LeaseOptions)
    -> Result<Option<FileLease>, LeaseError>
```

`FileLease` is deliberately non-cloneable; retaining it retains the locked
file descriptor, and dropping it releases the advisory lease.  The default is
five attempts, a 250 ms monotonic deadline, and mode `0600`.  It creates the
parent, opens the *lease file itself* with `O_RDWR | O_CREAT`/read+write+create,
applies `fchmod(0600)`, then requests `LOCK_EX | LOCK_NB`.  Only
`EACCES`, `EAGAIN`, and `EWOULDBLOCK` mean contention: on a non-final attempt
before the deadline, sleep `min(deadline_remaining, uniform(10 ms, 250 ms))`;
otherwise return `Ok(None)`.  Open, chmod, and other flock errors are returned.
This is an exact behavioral port of the Python retry shape, including clamping
attempts to at least one and the total deadline (`lease.py:93-118`).

This must not reuse `hold_lock`.  `hold_lock` protects a short critical section
using a `*.lock` sidecar and its own timeout/poll contract
(`core/crates/solstone-core-journal-io/src/locking.rs:59-99`); the refresh
lease remains held across an external multi-second probe.  The write operation
therefore owns **both** guards in the same order as Python: lifetime refresh
lease first, then scoped `brain.json.lock` while writing.  No inherited-FD
adoption is part of this wave: the server process itself acquires and holds the
lease.  The future equivalent of Python's separate adoption proof
(`lease.py:189-217`) must not be guessed into this API.

## 3. `brain refresh --session` wire contract

Session mode is a one-request, two-phase NDJSON protocol.  Every record is one
UTF-8 JSON object followed by `\n`; input is bounded and strict (object only,
no unknown top-level fields).  It mirrors Generate's framing discipline:
`_main_v2_session` reads lines until EOF and `_is_session_terminal` accepts a
terminal schema only if its field set is exact
(`solstone/think/generate_wire.py:306-313`, `:359-401`).

The server first performs native `begin_refresh` synchronously, keeping the
returned `FileLease`.  Only then does it accept the request line.  The Python
host runs the existing probes after spawning the child and sends exactly one:

```json
{"schema":"solstone.brain.refresh.probe.v1","outcome":{"configuration":{},"lane_prerequisites":{},"generate":{},"cogitate":{}}}
```

**Implementation addendum (ready handshake):** Immediately after a native
begin succeeds with a real permit, and before it starts reading NDJSON, the
child writes and flushes exactly
`{"schema":"solstone.brain.refresh.ready.v1"}`. A spawning Python caller must
block-read this first stdout line: `ready` proves the child holds the lease and
is waiting for the later probe, while a result record proves it already took an
immediate-exit path. Without this record the caller would need a timeout
heuristic that either misclassifies a slow immediate exit or delays every
successful begin. The prerequisite-renewal child has the identical handshake
with `solstone.brain.prerequisite_renewal.ready.v1`.

`outcome` is the existing four-component `BrainProbeOutcome`, including the
normal component fields (`status`, `observed_at`, and optional `reason_code`,
`expires_at`, and `diagnostic`) rather than a new lossy projection
(`brain_state.py:429-513`; `brain_cli.py:606-634`).  The server validates it by
the native equivalent of `_validate_probe_outcome` before publication.

The host then sends the exact, fieldless terminal record and closes stdin:

```json
{"schema":"solstone.brain.refresh.terminal.v1"}
```

This order is intentional.  The server buffers the probe outcome until it sees
the terminal record; it does not commit a result on receipt of an unclosed
request.  A terminal record is clean close authority, exactly as Generate's
session terminal is.

**Correction (an earlier draft of this section contradicted itself on
terminal-before-probe — resolved here; treat this version as authoritative):**

- **Bare EOF** (stdin closes with no terminal record ever received): the
  caller has disappeared. Abandon internally (revision incremented, checking
  cleared, `chat_timeout`/`generate` per §4 below) and exit —
  attempt **no** stdout write. There is normally no listener on the other end
  of a bare-EOF pipe, so a best-effort write to a broken pipe is pointless; the
  durable state change (the abandon write to `health/brain.json`) is what
  matters, not the report of it.
- **Timeout** (`SESSION_INPUT_TIMEOUT` elapses with the caller still
  connected, i.e. stdin has not closed): the caller is alive but stuck. Abandon
  the same way, but the caller may still be listening, so emit the
  `kind:"abandoned"` result record on stdout (best-effort — an I/O error
  writing it is logged, not treated as a second failure) before exiting.
  Process exit is `0` (`ExitCode::SUCCESS`): the session behaved exactly as
  designed, correctly detecting and reporting a hung caller — this is not the
  caller's protocol misuse, so it is not signaled as one.
- **Protocol violation** (terminal-before-probe, duplicate probe, malformed
  JSON, a non-object line, unknown top-level fields, or any record received
  after the terminal that is not itself the terminal repeated verbatim):
  the caller sent something the wire contract does not allow. Abandon
  internally if a permit still exists (same write as above), **and still
  emit the `kind:"abandoned"` result record** — the caller sent a terminal or
  another well-formed-enough line, so it is plausibly still listening and
  benefits from a concrete diagnostic rather than a bare nonzero exit. What
  distinguishes this path from timeout is the **exit code**: use a dedicated
  `EXIT_PROTOCOL` code (`76`, sysexits' `EX_PROTOCOL` — add it alongside this
  binary's existing sysexits-derived constants at
  `core/crates/solstone-core/src/main.rs:33-39`) rather than `0`, so the host
  can tell "hung caller, handled cleanly" apart from "caller broke the wire
  contract" even though both leave a structured record on stdout.
- **EOF after the terminal**: normal closure, no special handling — this is
  simply the tail end of the clean-terminal path below.

On clean terminal handling (terminal received after exactly one valid probe
record, in order, nothing after it but EOF), the server consumes the buffered
outcome, finishes the refresh, and writes exactly one stdout record before
exit:

```json
{"schema":"solstone.brain.refresh.result.v1","kind":"projection","projection":{"aggregate_state":"ready","reason_code":null,"active_lane":"byo-cloud","active_provider":"anthropic","active_model":"…","fingerprint_sha256":"…","runtime_transition_in_progress":false}}
```

Derive `projection` by calling W1's `inspect::inspect_brain_state` fresh
against the journal after the write, rather than hand-deriving a projection
inline in the writer — this is the same call the ordinary read path already
uses and keeps projection logic in exactly one place.

For an abandonment, the result record is:

```json
{"schema":"solstone.brain.refresh.result.v1","kind":"abandoned","reason_code":"chat_timeout","component":"generate"}
```

### 3.2 Immediate-exit outcomes (no permit ever taken)

`begin_refresh` can resolve before any lease is taken or any NDJSON record is
read at all — the session process must handle these without entering the read
loop:

- **None-lane write already completed** (`begin_refresh` internally routed
  through the non-lease none-lane write and returned `Ok(None)`): a record
  *was* written. Emit the same `kind:"projection"` shape as a normal finish
  (call `inspect_brain_state` fresh, same as above) and exit `0`.
- **Ordinary lease contention, no `expected_contract`** (`Ok(None)`, nothing
  written): emit
  `{"schema":"solstone.brain.refresh.result.v1","kind":"not_started","status":"no_permit","reason":"lease_held"}`
  and exit `0` — this is a normal, expected outcome (someone else is already
  refreshing), not a failure.
- **`BeginRefreshError::ExpectedFingerprintStale`**: a genuine precondition
  failure the caller asked to be told about explicitly (via
  `--expect-fingerprint`/`--expect-absent`). No stdout record; print the error
  to stderr and exit `EXIT_DATAERR` (`65`), matching how `journal-config
  commit`'s `CommitConfigError::Conflict` is already mapped in this binary.
- **`BeginRefreshError::InvalidArgument`**: unreachable through the CLI as
  designed (§3.1's `BrainRefreshExpectArg` enum structurally prevents
  supplying both `--expect-fingerprint` and `--expect-absent`), but keep the
  match arm exhaustive — treat it the same as a usage error, `EXIT_USAGE`
  (`64`), rather than panicking or silently ignoring an unreachable-in-practice
  variant.

### 3.3 `prerequisite-renewal --session` — the same framing, narrower payload

Identical wire discipline (NDJSON, terminal-then-EOF vs bare-EOF vs timeout vs
protocol-violation with the exact same exit-code taxonomy above), driving
`begin_prerequisite_renewal` → `finish_prerequisite_renewal`/
`abandon_prerequisite_renewal` instead. Differences only in schema and the
immediate-exit taxonomy:

- Probe-in: `{"schema":"solstone.brain.prerequisite_renewal.probe.v1","lane_prerequisites":{...}}`
  — a single component object (matching `finish_prerequisite_renewal`'s
  `lane_prerequisites: Value` parameter), not a four-component `outcome`
  wrapper.
- Terminal: `{"schema":"solstone.brain.prerequisite_renewal.terminal.v1"}`.
- A successful begin first writes and flushes
  `{"schema":"solstone.brain.prerequisite_renewal.ready.v1"}` before waiting
  for the probe, as described in §3's ready-handshake addendum.
- Result: same `kind:"projection"`/`kind:"abandoned"` shapes, schema renamed
  to `solstone.brain.prerequisite_renewal.result.v1`.
- Immediate exit on `BeginPrerequisiteRenewal::Busy { reason }`: emit
  `{"schema":"solstone.brain.prerequisite_renewal.result.v1","kind":"not_started","status":"busy","reason":"<reason>"}`
  and exit `0` — this is the one place `busy` is a real status (AC18), unlike
  refresh's contention case above.
- Immediate exit on `BeginPrerequisiteRenewal::Unsafe { reason }`: emit the
  same shape with `"status":"unsafe"` and exit `0` — `Unsafe` covers a family
  of preconditions that failed to hold (non-SPP lane, fingerprint mismatch,
  unsafe evidence, etc.), all still normal, well-formed answers, not process
  failures.

### Hung-but-alive caller bound (AC15)

Use a native `SESSION_INPUT_TIMEOUT = 90 seconds`, measured from successful
`begin_refresh` and covering the entire input/terminal exchange; it does not
reset per line.  This is a hard-coded server protocol constant, not a new
`local_contract.json` value.  The 600-second `checking_ttl_seconds` is the
record's stale-marker recovery horizon (`core/fixtures/local_contract.json:10`),
not an appropriate time for a live child to retain a lease.  Ninety seconds
allows the existing 60-second cogitate diagnostic (`brain_cli.py:115`,
`:558-584`) plus dispatch overhead, while bounding a stuck caller to a short,
observable lease interval.  Timeout takes the same abandonment path as bare
EOF, releases the lease, and exits; it cannot leave a checking record until the
600-second stale recovery.

### 3.1 Corrected CLI verb surface (found during CLI-stage design, not covered above)

Section 2's writer functions include `abandon_refresh`/`abandon_prerequisite_renewal`,
which both take a `BrainRefreshPermit` by value — and `BrainRefreshPermit`
holds a live `FileLease` (an owned, un-clone-able, unserializable OS file
descriptor guard). A `BrainRefreshPermit` therefore cannot survive being
printed to stdout by one `solstone-core brain <verb>` process and read back by
a second, later one: the lease is released the instant the first process
exits, and there is no JSON encoding of a live fd. Standalone one-shot verbs
like `begin-refresh` / `finish-refresh` / `begin-renewal` / `finish-renewal`
are therefore unsound for the write lanes above and must not be built as
separate CLI invocations — only the writer function that both takes and
consumes a lease-holding permit within the *same process* is safe, which is
exactly what `--session` mode is for. This constraint was previously reasoned
through only for `refresh`, but it applies identically to prerequisite
renewal, which was not designed against it in the assignment's own `§2 In
scope` wording ("A session-child command, `solstone-core brain refresh
--session`...") and needed correcting here before implementation.

The corrected CLI verb surface has exactly three families:

- **`solstone-core brain refresh --session`** — as specified above: begin,
  hold the lease across the caller's probe, finish or abandon, all in one
  process. When `begin_refresh` returns `Ok(None)` (no-lease none-lane write
  already completed, or ordinary lease contention with no `expected_contract`)
  or a `BeginRefreshError` (stale-fingerprint/invalid-argument), the session
  process emits a single terminal result record reflecting that outcome (see
  below) and exits immediately without ever entering the NDJSON read loop —
  there is no lease to hold and nothing further to negotiate.
- **`solstone-core brain prerequisite-renewal --session`** — the identical
  NDJSON framing (probe-outcome-in, terminal, result-out, same 90-second
  `SESSION_INPUT_TIMEOUT`, same bare-EOF-abandons contract), but: (a) the
  probe-outcome-in record carries a single `lane_prerequisites` component,
  not the four-component `refresh` shape, matching
  `finish_prerequisite_renewal`'s narrower `lane_prerequisites: Value`
  parameter; (b) internally calls `begin_prerequisite_renewal` →
  `finish_prerequisite_renewal`/`abandon_prerequisite_renewal`; (c) on
  `BeginPrerequisiteRenewal::Busy { reason }` or `::Unsafe { reason }` (no
  permit, no lease taken), emits an immediate terminal result carrying that
  status and reason and exits without entering the read loop, mirroring
  refresh's own no-permit fast exit above.
- **`solstone-core brain record-runtime-failure`** — an ordinary one-shot
  verb: bounded JSON on stdin (`reason_code`, `component`,
  `expected_fingerprint_sha256`, `diagnostic`, optional
  `bundled_runtime_fingerprint_sha256`), one JSON `RuntimeFailureResult` on
  stdout. No lease is ever taken by `record_runtime_failure` (confirmed:
  it only holds the scoped `health/brain.json.lock`, never
  `acquire_file_lease`), so this is safe as a standalone, independently
  invocable process, exactly like `journal-config read`/`commit`.

There is no standalone `begin-refresh`, `finish-refresh`, `abandon-refresh`,
`begin-renewal`, `finish-renewal`, or `abandon-renewal` verb. AC2's "reaches
every one of them" is satisfied by these three verb families covering the
full operation space (refresh begin/finish/abandon/none-lane via the first,
renewal begin/finish/abandon via the second, runtime-failure via the third) —
not by a one-verb-per-Python-function mapping, which the lease-lifetime
constraint above rules out.

## 4. AC16 abandonment reason

**Superseded during implementation — `probe_internal_error` does not work and
is replaced below.** The original choice was disqualified by a real,
structural finding, not just the aggregate-mapping mismatch this section used
to flag (kept below for the record, but no longer the operative concern):

`probe_internal_error` maps to aggregate `"unknown"` → component status
`"unknown"`. A record.rs fix (landed separately, correcting a genuine porting
bug where the reducer discarded the real reason for any non-ok/not_attempted/
failed/blocked status) makes an `"unknown"`-status component's reason at least
survive reduction — but every `"unknown"`-status candidate still sits at
**priority 4**, the SAME priority tier as a missing/null lane-applicable
component's `"brain_record_invalid"` candidate. `reduce_evidence_with_runtime`
breaks priority-4 ties by `component_order` index
(`configuration`, `lane_prerequisites`, `generate`, `cogitate`), and
`configuration` sorts first. Abandoning a **freshly begun** checking record —
exactly AC16's scenario, and exactly what `refresh --session`'s bare-EOF/
timeout path does — leaves every OTHER lane-applicable component still null,
including `configuration`. `configuration`'s null-component candidate
(`(4, 0, "brain_record_invalid")`) always wins the tie-break over
`lane_prerequisites`'s `(4, 1, "probe_internal_error")` candidate, so the
composed record's real abandon reason is discarded by the reducer regardless
of the discard-vs-preserve fix — `probe_internal_error` can never be the
*winning* reason when abandoning a fresh checking record, only when every
higher-index null component happens to already carry real (non-null)
evidence, which is not this scenario.

**Corrected choice: a reason whose aggregate is `unhealthy` (→ component
status `failed`, priority 2) or `blocked` (→ priority 3) — both of which beat
every priority-4 null-component candidate outright, regardless of
`component_order` position.** Restricting further to reasons with an *empty*
`diagnostic_metadata_schemas` entry (so the abandon call's empty diagnostic
object needs no fabricated fields) and a defensible "the exchange never
completed" reading:

- **`refresh --session`**: `chat_timeout` (aggregate `unhealthy`, empty
  diagnostic schema, present in the `generate` component's reason vocabulary
  only — `writer::target_component("chat_timeout")` resolves to `generate`,
  the first `component_order` entry whose vocabulary contains it). Call
  `abandon_refresh(permit, "chat_timeout", Map::new(), now)`.
- **`prerequisite-renewal --session`**: `nvattest_unavailable` (aggregate
  `blocked`, empty diagnostic schema, present in the `lane_prerequisites`
  vocabulary — `finish_prerequisite_renewal`'s component is fixed to
  `lane_prerequisites` by construction, and `target_component` resolves this
  reason there since it's the first, and only, applicable component).
  `local_server_unhealthy` was considered and rejected: while its aggregate
  (`unhealthy`) fits, its diagnostic schema *requires* `phase` and
  `runtime_reason` from closed enums describing real local-inference runtime
  states, none of which genuinely apply to "the session's caller vanished" —
  fabricating one to satisfy validation would be actively misleading. Call
  `abandon_prerequisite_renewal(permit, "nvattest_unavailable", Map::new(), now, ...)`.

Neither choice is a perfect semantic match for "the caller disappeared" — the
evidence-recordable vocabulary was not designed with a session-caller-timeout
case in mind — but both are defensible ("the exchange timed out" /
"verification did not respond") and, unlike `probe_internal_error`, both are
STRUCTURALLY guaranteed to survive reduction as the record's actual reason
regardless of which other lane-applicable components happen to be null at
abandon time. This is the load-bearing property AC16 requires and
`probe_internal_error` could not provide.

## 5. Direct port checklists

### AC7 — all seven finish fences

The acceptance wording previously said five; the source has seven checks.  The
Rust finish and renewal paths must preserve all seven and have one focused test
per fence:

1. assert that the permit still owns the refresh lease;
2. reject when `now >= permit.expires_at`;
3. reject when no current record exists or its `checking` block is absent;
4. reject when `current.revision != permit.checking_revision`;
5. reject when `checking.run_id != permit.run_id`;
6. reject when `checking.checking_revision != permit.checking_revision`; and
7. reject when `checking.runtime_failure_marker_seen != permit.runtime_failure_marker_seen`.

These are a transcription of `_assert_finish_allowed`
(`solstone/think/providers/brain_state.py:2202-2220`), before the separate
post-fence fingerprint and SPP-lane checks in each finish operation.

### AC9 — runtime-failure precedence

Implement the following ordered chain, preserving reject-vs-continue behavior:

1. Normalize `now` to UTC; a failure returns rejected `state_unavailable` with
   error text.
2. Reject `reason_not_recordable` unless the reason is known, non-projection
   only, and maps to a runtime-failure aggregate.
3. Reject `component_reason_not_allowed` unless the component is exactly
   `lane_prerequisites`, `generate`, or `cogitate` and allows that reason.
4. Validate diagnostic values; reject `reason_not_recordable` on validation
   failure.
5. Validate expected fingerprint hex; reject `fingerprint_mismatch` on failure.
6. Take the state lock and read the current record.  Read `OSError` rejects
   `state_unavailable`; malformed, validation-invalid, or JSON-undecodable
   state instead continues with `current=None` and `current_readable=false`.
7. Load/validate the current write fingerprint.  Config/key/fingerprint-load
   failures or unavailable/non-OK fingerprint reject `fingerprint_not_available`;
   a different fingerprint rejects `fingerprint_mismatch`.
8. Publish revision `next(current)` when readable, otherwise revision one;
   preserve evidence only for a current record with the same fingerprint,
   overwrite the named component with the reason-derived status and diagnostic,
   install a new runtime-failure marker, clear checking, and atomically write.
   Any unexpected outer exception is rejected `state_unavailable`.

This is the source's precedence, including the non-obvious malformed-record
continue path (`brain_state.py:2516-2629`); it is not a generic “read failure is
fatal” rewrite.

## 6. AC13 writer-parity corpus rule

**Revised during implementation** (this section originally claimed a 40-record
subset; that claim did not survive contact with the actual write-lane code and
is corrected here to the verified count).

The fixture has 74 named records, but it has no reachability annotation.  Use
a semantic, explicit 28-record subset for writer-parity tests: a record is
reachable only when a terminal native write operation, seeded with a valid
prior record, can publish its complete state.  Do not use name prefixes as the
criterion.

Exclude 46 composition-only records:

- all 18 `checking_*` records, including the six with null non-configuration
  evidence: checking is the intermediate begin state, not a terminal output;
- 12 absent/foreign-fingerprint records, because a terminal writer always
  resolves and writes its current fingerprint;
- four `marker_superseded_*` records whose marker/revision combination is an
  adversarial projector state rather than an output of a terminal lane;
- four `marker_current_*` records (one per lane) — **originally claimed
  reachable via `record_runtime_failure`; this is wrong.** Every
  `marker_current` fixture record carries full four-component evidence with
  every component `status: "ok"`, alongside a `runtime_failure_marker` whose
  `revision` equals the record's own `revision`. `record_runtime_failure`
  cannot produce this shape: it always overwrites its target component with a
  non-`ok` status derived from the reason code (`_component_status_for_reason`
  never returns `"ok"`) in the same write that creates the marker. A record
  with all-`ok` evidence and a same-revision marker is a validator/projector
  fixture proving that `reduce_evidence_with_runtime` still weights a current
  marker above otherwise-ready evidence — it is not the output of any write
  lane; and
- eight `lane_none/*` and `config_missing_provider/*` records — **also
  originally claimed reachable via the none-lane begin path; also wrong.**
  `begin_refresh` (`brain_state.py:1935-1944` and, independently, again inside
  the lock at `1965-1977`) refuses to grant a checking permit whenever the
  resolved lane is `"none"`, under every combination of
  `expected_active_fingerprint_sha256`/`expect_active_fingerprint_absent`: the
  no-expectation path takes the non-refresh (`_begin_nonrefresh_record`)
  branch and returns no permit; the expectation-set path raises
  `BrainStateExpectedFingerprintStaleError` outright. There is therefore no
  sequence of calls that reaches `finish_refresh` while the resolved lane is
  `"none"`, so these eight `aggregate_state: "ready"`-shaped, real-evidence
  `none`-lane records (distinct from what `_begin_nonrefresh_record` itself
  writes, which is always `blocked`/`thinking_engine_not_chosen` with null
  fingerprint) are composition-only too.

The remaining 28 ordinary terminal records (seven cases — `ready`,
`ready_expiring_within_the_hour`, `evidence_expired`, `generate_failed`,
`cogitate_failed_generate_ok`, `prerequisites_blocked`,
`updated_at_in_the_future` — across the four real lanes `lane_bundled`,
`lane_byo_cloud`, `lane_byo_endpoint`, `lane_spp`) are all reached by
`finish_refresh` alone, seeded from a prior record via a real `begin_refresh`
call. No case in the reachable set is owned by `finish_prerequisite_renewal`
or `record_runtime_failure` specifically — both remain exercised by their own
targeted unit tests, just not by this fixture-driven parity matrix.

Implementation must put the chosen case names in a reviewable static test
selector (or fixture metadata added in the same commit), assert its cardinality
is 28, and retain separate read/projection tests for all 74.  It must not
regenerate the corpus from the post-cut Python shim.

## 7. Python cutover, corpus, and documentation

The hard cut replaces `brain_state.py` with only typed transport/process
helpers for the native verbs.  It deletes the state schema, projection,
fingerprint, lease, and file-writing fallback from that module and updates all
callers, including `brain_cli.py` and the talent runtime-failure path, to use
the native CLI boundary.  Python still owns probes; native owns begin, finish,
abandon, renewal, marker recording, and final projection.

Before removing those definitions, retire
`scripts/local_contract_corpus.py` and `scripts/brain_projection_corpus.py`
from `scripts/build_core_fixtures.py::expected_outputs()` (currently imported
at lines 139-140 and registered around 1545-1561).  They import 42 attributes
from the module this wave empties, so a post-cut tree cannot regenerate these
authoritative fixture bytes.  Keep the checked-in fixtures and provenance;
their regeneration requires the designated pre-cut tree.

### Draft paragraph for `docs/PORTING.md` (apply during implementation)

> Native brain verbs ship as `solstone-core brain <verb>` subcommands of the
> installed aggregate binary, not as a standalone writer binary.  Wave 2 also
> retires `scripts/local_contract_corpus.py` and
> `scripts/brain_projection_corpus.py` from `expected_outputs()`: together they
> import 42 attributes from `brain_state.py`, which this wave reduces to a thin
> transport shim.  The checked-in contract and projection fixtures remain the
> native compatibility corpus; regenerating them requires the recorded pre-cut
> source tree, not a fallback implementation in the post-cut tree.

This belongs beside the existing wave-0/dual-path doctrine and the
`journal_config.py` precedent (`docs/PORTING.md:470-492`).

## 8. Ordered implementation plan

1. Add the journal-I/O lifetime lease with unit tests for all retry, mode,
   contention, ownership, and drop-release behavior.
2. Add the brain writer's record types, strict validation/projection helpers,
   atomic mutation paths, seven fences, and AC9 precedence tests.  Preserve the
   existing read-only inspection API.
3. Add `Brain` command parsing and aggregate `run_brain` dispatch, including
   explicit exit classes for usage, busy/conflict, unavailable, and protocol
   failure.
4. Implement session mode and its process-lifetime timer; exercise clean
   terminal EOF, bare EOF, timeout, malformed input, duplicate input, broken
   stdout, and lease release.
5. Move Python CLI/talent callers to the wire subprocess client and remove all
   Python state-authority code in the same commit.  Retire the two corpus
   generators from `expected_outputs()` before their imports disappear.
6. Add corpus parity and cross-process tests, update the CLI/documentation
   inventory, then run only the task-mandated focused checks in the implementation
   stage.

## 9. Acceptance-criteria disposition

Each row below is the corresponding assignment criterion, retained verbatim
apart from whitespace and table-cell wrapping.  “Implementation attention”
means the design selects the behavior but the listed proof has to be added and
bound during the cut.

| AC | Assignment criterion | Design disposition |
| --- | --- | --- |
| 1 | Exactly three write lanes plus the reference's one non-lease write path: `begin` on a `none` lane writes `thinking_engine_not_chosen` WITHOUT taking the lease and returns no permit. | The native writer ports the reference's `lane == "none"` branch as a fourth, non-lease begin outcome: state lock + atomic write only, `thinking_engine_not_chosen`, no lifetime lease, and `None` permit. Add a direct no-lease/no-permit test. |
| 2 | `solstone-core brain <verb>` reaches every one of them and is wired into the packaged `solstone-core` binary. A test asserts the subcommand surface exists and an unknown verb is a usage error, not a panic. | **Corrected in section 3.1:** the verb surface is exactly `refresh --session`, `prerequisite-renewal --session`, and `record-runtime-failure` — not one verb per Python function. A `BrainRefreshPermit`'s live `FileLease` cannot cross a process boundary, so standalone begin/finish/abandon verbs are unsound for both lease-taking lanes; only a single-process session command can safely hold a lease across a caller-driven probe. `record-runtime-failure` never takes a lease and is a genuine one-shot verb. Add `Command::Brain` → `parse_brain` → `run_brain` in the aggregate binary (section 1) with exactly these three verbs, and assert unknown verb returns usage. |
| 3 | Every write acquires `health/brain.json.lock` (10s bounded non-blocking retry) before the atomic replace; the lock file is never unlinked. A two-real-process test issues a runtime-failure write and a refresh finish concurrently and asserts both serialize and exactly one lands, the loser refused not merged. | Every emitting path takes existing `hold_lock` on the record before `write_json`; do not unlink its sidecar. **Implementation attention:** add the named two-process race and byte/state assertions; retain the lock's existing 10-second contract. |
| 4 | Record is `0600`, `indent=2`, `sort_keys=true`, trailing newline. | Configure native `write_json` with all four options on every record publication. **Implementation attention:** assert bytes and mode on each representative write path, including none lane. |
| 5 | Caller-supplied evidence timestamps pass through byte-identical (`...Z` form preserved, no chrono normalization); `updated_at` is `datetime`-isoformat style (`...+00:00`). | Deserialize evidence timestamps as validated strings and serialize their original bytes; use the native `now` formatter only for newly-owned fields such as `updated_at`. **At risk:** chrono/serde defaults can normalize `Z`, so parity tests must pin both forms. |
| 6 | `revision` is monotonic; every accepted write increments it. | Port `_next_revision` semantics in every accepted finish, abandon, renewal, runtime-failure, and none-lane write. **Implementation attention:** test one increment per accepted path and no increment on every refusal. |
| 7 | A finish is fenced on all checks from `_assert_finish_allowed` (design correctly identified 7, not the assignment's stated 5). One test per fence, each asserting the finish is refused AND the on-disk record is byte-identical to before. | **Corrected/satisfied by design only with seven fences:** section 5 lists all seven. Add a fixture-backed test for each that verifies refusal, byte-identical record, and a still-defined permit where applicable. |
| 8 | Fingerprint key: 32 cryptographically-random bytes, `0600`, never regenerated when one exists. A two-process race test on a journal with no key asserts exactly one key survives and no record references an overwritten key. | Native key handling must use 32 OS-CSPRNG bytes, exclusive/create-safe publication at `0600`, and re-read rather than replace an existing key. **Implementation attention:** add the required two-process no-key race; this is independent of the refresh lease. |
| 9 | Runtime-failure marker rejection follows the exact precedence in `record_brain_runtime_failure` (design correctly expanded to 8 steps). One test per adjacent pair, input tripping both, asserting the earlier name wins. Vocabulary is `runtime_failure_rejected_reasons` in `local_contract.json` — no Rust copy of that closed set. | **Satisfied by design:** section 5 transcribes the eight ordered stages, including malformed-record continuation. Load the rejection vocabulary from `local_contract.json`; do not duplicate the closed set in Rust. **Implementation attention:** add adjacent-pair precedence cases. |
| 10 | A record carrying a non-null `checking` block composes with `refresh_permit_active = true` unconditionally (the validator hardcodes this; do not reduce against live lease state while composing a checking record). | Keep composition pure: a non-null `checking` forces `refresh_permit_active=true`; composition never probes the live lease. **Implementation attention:** add a checking-record composition test with no held lease. |
| 11 | Every record any write path emits validates under W1's validator, including the `none`-lane path; every refusal path leaves the on-disk record byte-identical to before and leaves the permit defined. State each assertion per path. | All native records pass the shared W1 validator before publication. Build a per-path matrix for refresh finish/abandon, renewal finish/abandon, runtime failure, and none begin: emitted record validates; each refusal preserves bytes and does not consume/undefine a permit except where the reference's finally-release contract requires release. **At risk:** implement must distinguish “permit value remains defined for assertion” from lease ownership after a reference finally block. |
| 12 | Composition parity — for all 74 fixture records, composing from that record's own evidence/fingerprint/revision/checking/marker equals the recorded one field-for-field except `updated_at` (pinned as `now` per-record). | Retain all 74 as native compose/projection parity input. This is deliberately separate from write-lane parity; pin `now` to each fixture's `updated_at` expectation. |
| 13 | Write-lane parity, over the reachable subset only (design corrected this to 28/46 — use that; an earlier implementation-time draft of this document said 40/34, which double-checking against the actual write-lane code disproved — see section 6). Do not report criterion 12 as satisfying this. | **Corrected, satisfied by implementation:** the explicit semantic 28 reachable / 46 unreachable selector from section 6, asserted at that cardinality, drives `finish_refresh` from a seeded prior state for all 28 and asserts field-for-field equality against the fixture. AC12's 74-record composition test does not satisfy this criterion on its own — this is a separate, write-path-driven test. |
| 14 | Lease held by a live process for the whole refresh, released by process death alone. Test kills the holder, asserts a second refresh acquires with no intervening repair. | The journal-I/O RAII `FileLease` is retained from begin through external probe/terminal resolution; kernel close on process death releases it. **Implementation attention:** add the kill-holder cross-process test with no stale-file repair. |
| 15 | A hung caller does not wedge refresh — the child bounds its own life independent of the generate contract's caller-death handling (design's 90s bound). Test holds a child open past its bound with the parent alive, asserts the lease is free afterward. | **Satisfied by design:** session server has a non-resetting 90-second bound independent of Generate, then abandons and exits. Add the live-parent/hung-child test; use a test-only controllable clock/timeout hook so it does not wait 90 real seconds. |
| 16 | A bare EOF is abandonment, not a finish: revision incremented, `checking` cleared, a named reason on a named component (design originally chose `probe_internal_error`/`lane_prerequisites`; superseded in section 4 by `chat_timeout`/`generate` for refresh and `nvattest_unavailable`/`lane_prerequisites` for renewal, after discovering the original choice can never win the priority-4 tie-break against a null `configuration` component on a freshly begun checking record). | **Corrected, satisfied:** bare EOF/timeout invokes native abandon with the corrected reason/component pair from section 4, which is structurally guaranteed (priority 2/3, not 4) to survive reduction as the record's actual reason regardless of which other components are null, incrementing revision and clearing checking. |
| 17 | A crash after `begin` leaves all-null evidence plus a fresh checking block — test the record the writer actually writes (not the fixture's `checking_expired_after_crash`); assert it validates and projects `brain_check_interrupted` through W1's read path once the lease frees. | Begin writes the actual fresh checking record with all-null evidence before the process dies. **Implementation attention:** crash a real holder after begin, assert that exact record validates, then inspect through W1 after kernel lease release and assert `brain_check_interrupted`. |
| 18 | Two concurrent refresh attempts: exactly one acquires, the other reports no permit and writes nothing (not `busy` — that status is prerequisite-renewal-only per the reference; keep that split or explicitly document a new status). | Preserve the reference split: `begin_refresh` returns **no permit** (`None`) on lease contention and writes nothing; it does **not** return `busy`. `begin_prerequisite_renewal` returns its existing **`busy`** result on contention. Add a two-attempt test for both operations and do not introduce a new status. |
| 19 | `inspect` still creates nothing — record, key, either lock, lease. Re-assert with the same full-tree snapshot approach W1 used. | Keep `solstone-core-brain` inspection read-only; neither inspect nor composition acquires/creates record, key, state lock, or refresh lease. **Implementation attention:** re-run W1's full-tree snapshot assertion against native inspect. |
| 20 | `brain_state.py` holds no record logic — fingerprint algorithm, validator, projector, evidence reduction, composer, all three write lanes gone. What remains: TypedDicts/exceptions, a vocabulary loader reading `local_contract.json` directly, and transport spawning `solstone-core brain`. | Hard-cut exactly to that thin Python transport surface; move all listed state logic to Rust and retain no fallback. **Implementation attention:** audit exports/importers so only permitted TypedDicts, exceptions, direct vocabulary loader, and process transport remain. |
| 21 | No fallback — no try-Rust-then-Python, no env var, no legacy path. | The transport treats missing binary, malformed response, and nonzero native exit as failure; it never computes/writes state in Python. Add static and behavioral no-fallback checks. |
| 22 | Public API unchanged in name and signature for all five importers (the 24 `brain_cli.py` symbols, 8 `brain_health.py` symbols, `talents.py`'s 2 import sites, `cortex.py`'s 2 import sites, `apps/thinking/routes.py`). If a signature genuinely cannot survive, name it and why. | Preserve all named Python function/type names and call signatures in the transport shim; implementation must enumerate the stated importer symbols before deletion. **At risk:** any API that exposes an in-process permit/lease cannot transparently cross the process boundary; if found, it must be named and approved rather than silently changed. |
| 23 | The single-writer assertion is added to `scripts/check_brain_health_cutover.py` as an AST walk (not a filename grep) enumerating every path that opens the record/key/either-lock/lease for writing across the Python tree, asserting the set is empty. `make check-brain-health-cutover` is added to this wave's gate list. | Add the AST-based Python writer inventory and the Make gate in the cut commit. **Implementation attention:** include record, fingerprint key, record lock, key lock if any, and refresh lease paths; report the empty set rather than filename matches. |
| 24 | `scripts/local_contract_corpus.py` and `scripts/brain_projection_corpus.py` are retired from `expected_outputs()` in the same commit as the cut. Fixtures stay frozen; generators are marked as requiring a pre-cut tree. | Satisfied by the section 7 cutover plan and drafted PORTING text: retire both registrations/imports in the same hard-cut commit, retain fixtures/provenance, and mark generation pre-cut-only. |
| 25 | Python test suite handling: no new Python test; tests whose subject is deleted are deleted (name them + count); tests asserting the public-API contract are kept and must pass. Run affected suites explicitly, each with its own exit code. | **Implementation attention:** do not add Python tests. Inventory deleted brain-state-subject tests by name/count before deletion, retain API-contract tests, and report each affected suite's separately captured exit code. |
| 26 | No file under `solstone/think/providers/` other than `brain_state.py` is modified. | Restrict Python provider edits to `brain_state.py`; enforce with final path review. **At risk:** if a sibling provider change seems necessary, stop for scope approval rather than broaden this wave. |
| 27 | `core/fixtures/local_contract.json` and `core/fixtures/brain_projection.json` are byte-unchanged — assert via hash comparison against the lode's base commit, not `git diff` against HEAD. | Treat both fixtures as frozen; implementation verification compares their hashes to the lode base commit. Do not use HEAD diff as the proof. |
| 28 | The `generate` contract, its fixture, and `solstone-core-generate` are untouched. | Session framing borrows Generate's protocol discipline only; it makes no source, fixture, crate, or contract change under Generate. Enforce with final path review. |
| 29 | `core/deny.toml:30`'s stale reason string is updated; if a new writer crate is introduced, it is also added to the `wrappers` list itself (design chose no new crate, so this is just the string update). | Satisfied by the existing-crate decision: update only the stale reason string at line 30; no new crate and no wrappers-list addition. |
| 30 | End-to-end verification through a real Python caller against a live journal is explicitly VPE-direct post-ship work — do not attempt it in the lode, do not report it as done. | Explicitly out of lode scope. Implementation may run fixtures/process tests only; it must neither attempt live-journal VPE-direct nor claim it complete. |

The material corrections remain AC7 (seven fences), AC13 (explicit 40/34
writer subset), and AC16 (the original `probe_internal_error`/`lane_prerequisites` choice could never win its own priority-4 tie-break on a fresh checking record; corrected in section 4 to `chat_timeout`/`generate` and `nvattest_unavailable`/`lane_prerequisites`).
AC18 additionally fixes the behavioral fork: refresh contention is no-permit
with no write, while prerequisite-renewal contention is `busy`.
