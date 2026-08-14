# Native `journal maint` runner design

## Purpose and parity boundary

This design ports the Python-owned `journal maint` command, its static 27-item
one-time migration registry, state JSONL reader/writer, serial attempt runner,
and all body behavior to Rust.  It preserves the Python command contract and
on-disk formats described in `maint-native-prep-notes.md`; it deliberately
does **not** add validation, a supervisor-start gate, a dynamic registry, a
second executable, or a compatibility interpreter.

The static Rust registry replaces Python source discovery.  Its order is the
Python implementation's observed `(task name, app)` order, not the stale
Python docstring's `(app, task)` claim.  `blocks_supervisor_start` remains
metadata only: no runtime consumer is introduced.

## Crate boundary and module layout

Create `solstone-core-maint`, a library crate, rather than enlarging
`solstone-core-system-health`.  The latter remains the small, dependency-light
read-side owner used by doctor.  The new crate follows the sibling recurring
`solstone-core-maintenance` precedent: it owns parser, static registry, CLI
rendering, attempt orchestration, and injectable services.  Unlike that
recurring-routine crate, it also owns a private one-task worker protocol.

`solstone-core-maint` depends on system-health for the read model and on the
domain crates only for their public migration APIs.  It never opens or mutates
domain files itself.  Each body function is physically implemented at the
write-owning domain boundary listed below; `bodies/` contains only the 27
static dispatch adapters and retired zero-result bodies.  This is required by
the L1/L2 ownership rules, not a convenience abstraction.

```text
core/crates/solstone-core-maint/
  src/lib.rs                  # public run_cli, run_worker, injectable runner entry points
  src/parser.rs               # maint argv parser and exact usage/help strings
  src/registry.rs             # literal [MaintTask; 27], lookup and task metadata
  src/state.rs                # state/read presentation adapters over system-health
  src/attempt_log.rs          # append-and-flush JSONL writer and attempt replay reader
  src/runner.rs               # parent serial runner, timeout policy, RunnerPlatform trait
  src/worker.rs               # private worker request validation and one-body dispatch
  src/render.rs               # list/details/task formatting and outcome text
  src/bodies/mod.rs           # one adapter per live body; two retired bodies only here
  src/bodies/{activities,entities,...}.rs
  tests/...                   # unit/integration tests added with implementation

core/crates/solstone-core-system-health/src/maint.rs
  # static-registry-aware read model; no parser, process, or body dependency

core/crates/solstone-core-convey-config/       # NEW bounded config/convey.json owner
core/crates/solstone-core-talents/              # NEW bounded agents/talents layout owner
```

`solstone-core-convey-config` is intentional.  The native tree has two
limited pre-existing writers of `config/convey.json`: the private facet rename
side effect in `solstone-core-facets/src/store/write.rs:267` and the private
facet-merge helper in `solstone-core-journal-cli/src/local_ops.rs:1054`.
Neither is a reusable config authority.  The new crate will own the three
convey migrations and absorb those two narrow existing writers when the crate
is introduced, leaving no competing raw writer.  It is not a generic journal
configuration API.

`solstone-core-talents` is likewise necessary rather than placing agent/run
directory writes in journal-io.  Current `solstone-core-sol` owns AI skill
configuration, but no native crate owns journal-root `agents/`/`talents/`
layout or run logs.  Journal-io supplies containment/atomic primitives only;
it must not become the owner of those domain paths.

Wire the public command into `solstone-core` alongside `Maintenance` with
`run_storage_ops_verb("maint", ...)` and add the crate to the workspace and
core dependencies.  The journal CLI cutover is last: retain the `maint`
`PROCESS_SPECS` record for routing/coherence/census provenance and add a
matching `NATIVE_PROCESS_SPECS` record targeting `solstone-core ["maint"]`.
This is the established dual-table native process pattern.

## CLI and registry contract

The Rust parser is owned by `solstone-core-maint::parser`; it must reproduce:

* description `Run maintenance tasks for apps`, the optional `task` positional,
  `--list/-l`, `--force/-f`, and the shared `-v/--verbose`, `-d/--debug`
  behavior and help text from `setup_cli()`;
* the four-line usage/example text and `--` end-of-options behavior;
* `--list` precedence over both task and `--force`;
* unqualified matching only when unique.  A duplicate logs the same ambiguity
  warning then follows the existing CLI path to `Task not found: {task_name}`
  plus `Use 'journal maint --list' to see available tasks.` and exit 1;
* Pending, In Progress, Failed, Completed grouping and exact current task,
  detail, prior-attempt, no-task/no-pending, force-without-task, and completed
  rendering from the prep note.

The registry is literal data; no source scanning or AST parsing exists in the
native implementation:

```text
pub struct MaintTask {
    pub app: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub retry_on_next_start: bool,
    pub blocks_supervisor_start: bool,
    pub body: MaintBody,
}
pub type MaintBody = fn(&MaintBodyContext<'_>) -> MaintBodyResult;
pub struct MaintBodyContext<'a> { pub journal: &'a Path, pub dry_run: bool, pub verbose: bool }
pub struct MaintBodyResult { pub stdout: Vec<String>, pub exit_code: i32 }
```

`dry_run` is worker-internal body wiring, not a new public `journal maint`
option.  The existing public command has no dry-run flag.  Individual
migration APIs receive `false` from production dispatch and fixture tests can
invoke their public domain function with `true` directly.

## Read state reconciliation

Replace the public five-state enum with the four observable CLI states:

```text
pub enum MaintTaskStatus { Pending, InProgress, Success, Failed }

pub enum MaintStateIntegrity { Parsed, Unreadable }

pub struct MaintTaskState {
    pub app: String,
    pub task: String,
    pub description: String,
    pub retry_on_next_start: bool,
    pub blocks_supervisor_start: bool,
    pub status: MaintTaskStatus,
    pub exit_code: Option<i64>,
    pub ran_ts: Option<i64>,
    pub duration_ms: Option<i64>,
    pub line_count: usize,
    pub state_file: PathBuf,
    pub integrity: MaintStateIntegrity,
}
```

`Unreadable` is therefore an integrity observation, not a terminal task
status.  A missing file is `Pending/Parsed`; a nonempty file with no valid JSON
object rows, or a file that cannot be read, is `InProgress/Unreadable`.  Blank
and malformed rows are skipped.  Mixed valid/malformed input is `Parsed` and
is interpreted from the latest valid attempt.  This exactly keeps Python's
visible “present but unusable falls through to in progress” behavior while
retaining the diagnostic distinction that doctor has today.

`read_maint_task_states(journal, registry)` produces every static task (so a
never-run source task is visible), ordered by the registry.  Keep a
compatibility read-only convenience function for callers that need only
durable state-file discovery during the transition, but move doctor to the
registry-aware call.  `read_maint_task_state(journal, task)` derives the
state-file path.  `latest_attempt_events(text)` retains current split rules:
new `exec` starts a block; a different nonempty string attempt ID starts one;
the final block wins.

Doctor must change its match without changing its output semantics:

* `Failed` remains failure.
* `InProgress` with `ran_ts` older than 300,000 ms remains `started, no exit`.
* `integrity == Unreadable`, or `InProgress` without `ran_ts`, remains the
  existing unreadable-state warning text.

Thus no condition formerly diagnosed as `MaintTaskStatus::Unreadable` is
silently made healthy.

## Attempt JSONL and worker protocol

The direct writer is intentionally flush-only.  Do not use
`solstone-core-journal-io::append_jsonl`, because that helper fsyncs and is
stronger than Python parity.  The writer opens
`<journal>/maint/<app>/<task>.jsonl` with create+append, writes one serialized
row plus `\n`, and calls `flush()` after every row; it never calls `sync_all`.

```text
pub struct AttemptLogWriter { /* file, task, attempt_id */ }
pub enum MaintAttemptEvent { Exec(AttemptExec), Line(AttemptLine), Exit(AttemptExit) }
pub fn open_attempt_log(journal: &Path, task: &MaintTask, attempt_id: String)
    -> io::Result<AttemptLogWriter>;
pub fn append_attempt_event(writer: &mut AttemptLogWriter, event: &MaintAttemptEvent)
    -> io::Result<()>;
pub fn read_attempt_logs(path: &Path) -> io::Result<Vec<AttemptLog>>;
```

`AttemptExec`, `AttemptLine`, and `AttemptExit` serialize respectively to the
existing `exec`, `line`, and `exit` JSON objects: `attempt_id`, epoch-ms `ts`,
and the same `app/task/cmd`, string `line`, or `exit_code/duration_ms/error`
fields.  `duration_ms` is absent for spawn exceptions and stalled exit rows;
the output reader removes only the final newline.  Attempt IDs are UUID v4
lowercase-hyphenless hex strings.

The private child command is:

```text
<current solstone-core executable> __maint-worker --one-task --task <app:name>
```

The parent sets `SOLSTONE_JOURNAL` explicitly to its resolved absolute journal
root and pipes stdout and stderr (merged by the parent); it supplies no stdin.
`__maint-worker` is recognized before public command parsing and is absent
from public usage.  `--one-task` is mandatory, making accidental invocation
or a future public-name collision fail closed.  The worker accepts only an
exact qualified static-registry name, runs exactly that body once with
`dry_run=false`, prints every body line to stdout, and exits with the body's
returned code.  It cannot list, select pending work, recurse to a parent, or
write attempt JSONL.

The parent records the worker argv in its `exec` row, starts a combined-output
reader, prints each captured line as `  {line}`, and appends corresponding
`line` rows.  Timeout behavior is fixed: warning after each quiet **30.0 s**,
hard cap at **120.0 s**, `terminate`, wait **5 s**, `kill`, wait **5 s**, then
synthetic `-SIGKILL` if still unkillable.  A hard cap logs/stores
`error="stalled"`; a spawn exception stores exit `-1` and its error with no
duration.  Parent scheduling remains serial: run pending plus failed/retry
tasks, do not rerun in-progress or ordinary failures, and continue after an
error.

Keep timeout policy deterministic without sleeps or real processes:

```text
pub trait RunnerPlatform {
    fn now_monotonic(&self) -> Duration;
    fn now_epoch_ms(&self) -> i64;
    fn spawn_worker(&self, request: &WorkerRequest) -> Result<Box<dyn WorkerChild>, RunnerError>;
}
pub trait WorkerChild {
    fn recv_line_until(&mut self, deadline: Duration) -> Result<Option<String>, RunnerError>;
    fn try_wait(&mut self) -> Result<Option<i32>, RunnerError>;
    fn terminate(&mut self) -> Result<(), RunnerError>;
    fn wait_until(&mut self, deadline: Duration) -> Result<Option<i32>, RunnerError>;
    fn kill(&mut self) -> Result<(), RunnerError>;
}
```

The real platform wraps `Command`, a stdout-reader thread, and `Instant`; a
fake platform returns scripted clock/output/process events.  `run_task_with`
accepts `&dyn RunnerPlatform`, so warning, hard-cap, termination, and JSONL
shape tests need neither wall-clock sleeps nor spawned binaries.

## Final per-body domain routing

`REUSE` APIs are cited at their current locations.  `NEW` signatures are the
public, buildable domain API to add; each report/error type named here is a
public type defined alongside its function.  No body adapter writes a path
directly.

| Task | Target domain crate and status | Exact API and destination |
|---|---|---|
| activities:000_migrate_activity_icon_to_emoji | facets — NEW | `pub fn migrate_custom_activity_icons_to_emoji(journal: &Path, dry_run: bool) -> Result<ActivityIconMigrationReport, FacetWriteError>` in `solstone-core-facets/src/store/activities.rs`. |
| entities:001_migrate_to_journal_entities | facets (which already depends on entity + entity-matching) — NEW + REUSE | `pub fn migrate_legacy_facet_entities(journal: &Path, fuzzy_threshold: u8, dry_run: bool) -> Result<LegacyFacetEntityMigrationReport, FacetEntityMigrationError>` in `solstone-core-facets/src/store/legacy_entity_migration.rs`; use `entity_matching::entity_slug` (`slug.rs:16`), `entity::create_journal_entity` (`store/create.rs:20`), and `save_facet_entity_link` (`store/write.rs:126`). The adapter passes literal `90`; matching is `>=`. |
| observer:000_migrate_remote_to_observer | observer — NEW | `pub fn migrate_remote_observer_storage(journal: &Path) -> Result<RemoteObserverMigrationReport, RemoteObserverMigrationError>` in `solstone-core-observer/src/store/remote_migration.rs`; it uses the observer's store writers plus journal-config-write's existing transaction. |
| search:003_migrate_index_stream | indexer-store — NEW + REUSE | `pub fn migrate_legacy_stream_index(journal: &Path) -> Result<IndexStreamMigrationReport, StoreError>` in `solstone-core-indexer-store/src/migrations/index_stream.rs`; it owns PRAGMA inspection and calls `reset_index` (`db.rs:130`) then `scan_journal(journal, true)` (`scan.rs:118`). Reset failure is reported as Python does but does not turn a recognized legacy schema into an unhandled body failure. |
| search:004_migrate_topic_to_agent | facets + journal-stats-cli — NEW | `pub fn migrate_event_topic_keys(journal: &Path, dry_run: bool) -> Result<EventTopicMigrationReport, EventTopicMigrationError>` in `solstone-core-facets/src/store/event_topic_migration.rs`; `pub fn migrate_stats_topic_keys(journal: &Path, dry_run: bool) -> Result<StatsTopicMigrationReport, StatsMigrationError>` in `solstone-core-journal-stats-cli/src/migrations/topic_keys.rs`. The latter is the correct `stats.json` owner: current root write is `run.rs:127`; it also deliberately covers discovered stats files. |
| settings:001_backfill_streams | segment — NEW | `pub fn backfill_stream_records(journal: &Path, host: Option<&str>, verbose: bool) -> Result<StreamBackfillReport, StreamRepairError>` in `solstone-core-segment/src/stream_repair.rs`. Preserve all nine Python signal rules and precedence verbatim. |
| settings:002_restructure_stream_dirs | segment — NEW | `pub fn restructure_segments_by_stream(journal: &Path, dry_run: bool) -> Result<SegmentRestructureReport, SegmentRelocationError>` in `solstone-core-segment/src/relocate.rs`; it owns marker preflight, moves, verification, and the no-preflight-collision behavior. |
| settings:003_seed_default_app_navigation | convey-config — NEW | `pub fn seed_default_app_navigation(journal: &Path) -> Result<ConveyConfigMigrationReport, ConveyConfigError>` in `solstone-core-convey-config/src/navigation.rs`. It locks/mutates only `config/convey.json`, preserves no-op bytes, and does not create the file after a resolver/persist failure. |
| settings:004_backfill_import_manifests | import — NEW + REUSE | `pub fn backfill_retained_import_manifests(journal: &Path) -> Result<ImportManifestBackfillReport, ImportError>` in `solstone-core-import/src/dedupe.rs`, using `write_manifest(&ManifestWriteRequest)` (`dedupe.rs:100`). |
| settings:005_pin_curation_nav | convey-config — NEW | `pub fn pin_curation_navigation(journal: &Path) -> Result<ConveyConfigMigrationReport, ConveyConfigError>` in `solstone-core-convey-config/src/navigation.rs`. |
| settings:006_drop_services_nav | convey-config — NEW | `pub fn drop_services_navigation(journal: &Path) -> Result<ConveyConfigMigrationReport, ConveyConfigError>` in `solstone-core-convey-config/src/navigation.rs`. |
| settings:007_migrate_pdf_extractions | segment — NEW | `pub fn migrate_pdf_extractions(journal: &Path) -> Result<PdfExtractionMigrationReport, PdfExtractionMigrationError>` in `solstone-core-segment/src/document_migration.rs`; it verifies a nonempty replacement before deletion and retains header-only/unparseable inputs. |
| settings:008_migrate_pairing_home_address | journal-config-write + sol-link — NEW | `pub fn migrate_pairing_home_address(journal: &Path) -> Result<PairingAddressMigrationReport, JournalConfigWriteError>` in `solstone-core-journal-config-write/src/pairing_migration.rs`; it invokes `pub fn parse_legacy_pairing_home_address(raw: &str) -> Result<String, LegacyPairingAddressError>` added to `solstone-core-sol-link/src/pairing/addresses.rs`. |
| sol:000_migrate_agent_layout | segment — NEW | `pub fn migrate_agent_layout(journal: &Path, dry_run: bool) -> Result<AgentLayoutMigrationReport, SegmentRelocationError>` in `solstone-core-segment/src/relocate.rs`. |
| sol:001_migrate_agent_run_logs | talents — NEW | `pub fn migrate_agent_run_logs(journal: &Path, dry_run: bool) -> Result<TalentRunLogMigrationReport, TalentStorageError>` in `solstone-core-talents/src/run_logs.rs`. |
| sol:002_migrate_chronicle | segment + indexer-store — NEW | `pub fn migrate_root_days_to_chronicle(journal: &Path, dry_run: bool) -> Result<ChronicleMigrationReport, ChronicleMigrationError>` in `solstone-core-segment/src/chronicle_migration.rs`, followed by `pub fn remove_legacy_index_artifacts(journal: &Path) -> Result<IndexArtifactRemovalReport, StoreError>` in `solstone-core-indexer-store/src/migrations/index_stream.rs`. Copy-merge/delete and final root-day/index absence checks remain in the segment operation. |
| sol:004_rename_agents_to_talents | talents — NEW | `pub fn rename_agents_to_talents(journal: &Path, dry_run: bool) -> Result<AgentsToTalentsMigrationReport, TalentStorageError>` in `solstone-core-talents/src/layout.rs`; it plans all paths, aborts before mutation on any collision, and treats destination-only paths as migrated. |
| sol:005_migrate_dream_to_think_schedules | system — NEW | `pub fn migrate_dream_to_think_schedules(path: &Path, dry_run: bool) -> Result<ScheduleMigrationReport, ScheduleError>` in `solstone-core-system/src/schedule/config.rs`. |
| sol:006_rename_unified_triage_providers | maint — RETIRED | `fn retired_unified_triage_providers(_: &MaintBodyContext<'_>) -> MaintBodyResult` in `solstone-core-maint/src/bodies/sol.rs`; unconditional zero/skipped `retired`, no domain call. |
| sol:007_migrate_sol_service_schedules | system — NEW | `pub fn migrate_sol_service_schedules(path: &Path, dry_run: bool) -> Result<ScheduleMigrationReport, ScheduleError>` in `solstone-core-system/src/schedule/config.rs`. |
| sol:008_migrate_provider_check_schedule | system + system-health — NEW | `pub fn migrate_provider_check_schedule(path: &Path) -> Result<ProviderScheduleMigrationReport, ScheduleError>` in `solstone-core-system/src/schedule/config.rs`, followed only on successful schedule processing by `pub fn remove_legacy_provider_check_artifacts(journal: &Path) -> Result<ProviderArtifactCleanupReport, HealthArtifactError>` in `solstone-core-system-health/src/provider_artifacts.rs`. |
| sol:009_remove_granola_sync_schedule | system — NEW + REUSE | `pub fn remove_granola_sync_schedule(path: &Path) -> Result<GranolaScheduleRemovalReport, ScheduleError>` in `solstone-core-system/src/schedule/config.rs`; it performs the exact-key/value guard before internally using existing `remove_schedule_entry(path, "sync:granola")` (`config.rs:110`). |
| thinking:000_unify_provider_config | thinking + journal-config-write — NEW + REUSE | `pub fn unify_provider_config(config: &mut Map<String, Value>, environment: &ProviderEnvironment) -> ProviderConfigMigrationReport` in `solstone-core-thinking/src/providers.rs`; `pub fn apply_provider_config_migration(journal: &Path, environment: &ProviderEnvironment) -> Result<ProviderConfigMigrationReport, JournalConfigWriteError>` in `solstone-core-journal-config-write/src/provider_migration.rs`, using `mutate_journal_config` (`config.rs:72`). |
| thinking:001_migrate_provider_install_state | local — NEW + REUSE | `pub fn migrate_provider_install_state(journal: &Path, proof: &dyn LegacyInstallProofReader) -> Result<ProviderInstallStateMigrationReport, ProviderInstallStateMigrationError>` in `solstone-core-local/src/install/legacy_migration.rs`; it uses `write_status` (`install/status.rs:338`) after verified proof and owns non-destructive busy/proof/mismatch outcomes. This is the required legacy-truth bridge; it is absent today. |
| thinking:002_pin_google_model_aliases | thinking + journal-config-write — NEW + REUSE | `pub fn pin_google_model_aliases(config: &mut Map<String, Value>) -> GoogleModelAliasMigrationReport` in `solstone-core-thinking/src/providers.rs`; `pub fn apply_google_model_alias_migration(journal: &Path) -> Result<GoogleModelAliasMigrationReport, JournalConfigWriteError>` in `solstone-core-journal-config-write/src/provider_migration.rs`, using `mutate_journal_config`. |
| timeline:002_migrate_rollup_schedules | system — NEW + REUSE | `pub fn remove_legacy_rollup_schedules(path: &Path, dry_run: bool) -> Result<ScheduleMigrationReport, ScheduleError>` in `solstone-core-system/src/schedule/config.rs`; it exact-compares both legacy values before internally using `remove_schedule_entry`. |
| timeline:002_register_segment_summary_model | maint — RETIRED | `fn retired_segment_summary_model_registration(_: &MaintBodyContext<'_>) -> MaintBodyResult` in `solstone-core-maint/src/bodies/timeline.rs`; unconditional zero/skipped `retired`, no config/domain read. |

The schedule APIs are deliberately named migration operations rather than a
public arbitrary-map rewrite.  Existing `remove_schedule_entry` is sufficient
only after each operation's ownership guard; no generic rewrite/remove API is
introduced.

## Fixture and test strategy

Tests use real temporary journal trees and real JSON/JSONL/SQLite files; no
mock filesystem or fake persistence layer is used for body behavior.  Every
fixture is built by a named helper under the owning crate's test support and
mirrors the Python test inputs named in the prep notes.  The body-adapter tests
assert output/exit mapping separately from domain fixtures.

| Task(s) | Real fixture shape and required assertions |
|---|---|
| activities:000 | `facets/<facet>/activities/*.jsonl` containing custom and already-emoji icons; assert changed/scanned reports and dry run byte identity. |
| entities:001 | `facets/<facet>/entities.jsonl` with detached, exact, fuzzy-89/90, aliases/principal variants; assert `entities/<slug>/entity.json` and scoped relation files, longest-name/union behavior, and retained legacy JSONL. |
| observer:000 | `apps/remote/remotes/**`, `apps/observer/observers/**`, and `config/journal.json` variants for source-only, identical destination, differing destination, and observer-key already set. |
| search:003 | Build a legacy `indexer/journal.sqlite` with `rusqlite` in a test helper using the old `chunks` columns, then place an indexable journal file and assert reset+full scan.  Separate real current-schema and absent-DB fixtures assert no-op.  Do not check in an opaque `.sqlite` blob: programmatic schema construction is reviewable and directly states the legacy contract. |
| search:004 | `facets/*/events/*.jsonl` with topic/agent collisions plus root and day `stats.json` objects containing all four topic-key maps; assert no overwrite and dry-run byte identity. |
| settings:001 | Chronicle segment directories with `stream.json`, `audio.jsonl`, imported audio/raw/import manifests, host/tmux/fallback variants; table-drive the nine precedence signals and assert stream state seq/prev linkage. |
| settings:002 | Direct-child segment marker fixture, missing-marker refusal fixture, and existing target fixture; assert no mutation before refusal and post-move layout/count. |
| settings:003/005/006 | Real `config/convey.json` bytes for absent, malformed-list, populated, duplicate, and no-op forms; assert lock transaction, byte-identical no-op, and persist failure leaves no new/partial file. |
| settings:004 | `imports/<id>/import.json`, retained basename file, and optional `imported.json`; assert hash, source/days/files fields, existing manifest idempotence, and retained-original refusal. |
| settings:007 | Cross-stream document/image JSONL next to nonempty, header-only, and missing transcripts; orphan extraction text fixtures assert verified markdown creation before delete and unconvertible preservation. |
| settings:008 | `config/journal.json` valid bare URL, credentials/query/path/invalid values, existing `home_address`, and no-op variants; assert host_url removal policy. |
| sol:000 | Segment root markdown/known JSON/facet state plus daily flat agent names; include identical and differing destinations and dry-run bytes. |
| sol:001 | Root `agents/` flat JSONLs, symlink JSONL, named and unnamed runs, existing destination, and day index; assert symlink/remove/move/index semantics. |
| sol:002 | Root `YYYYMMDD` trees, existing `chronicle/YYYYMMDD` merge target, and sqlite/WAL/SHM files; assert merge/delete and final absence validation. |
| sol:004 | Root/day/segment `agents` and `health/agents.json`; assert all-or-nothing collision refusal, destination-only no-op, and dry-run plan. |
| sol:005/007/009 + timeline rollup | Real `config/schedules.json` maps preserving metadata/order: stale/unstale commands, exact/divergent named entries, malformed/missing/empty, dry run, no-op bytes/mtime, and write failure. |
| sol:006 + timeline register | Arbitrary malformed/missing journal layouts; assert returned zero/skipped summary and no bytes/mtime change for normal and dry-run direct tests. |
| sol:008 | Schedules with old provider cadence/enabled combinations, exact existing brain, malformed schedule, then `health/talents.json`, locks/log/tmp files; assert cleanup only after schedule success and cleanup failure is retryable. |
| thinking:000 | `config/journal.json`, environment map, and `.config/vertex-credentials.json` fixtures for active-provider precedence, malformed recovery, confidential cleanup, and exact legacy-field deletion. |
| thinking:001 | Real local install status/cache/config records plus fixture proof-reader bytes for ready, mismatch, unavailable proof, and busy cases; assert no destructive state change until proof validates and retry/supervisor metadata stays registry-driven. |
| thinking:002 | `config/journal.json` provider slots/pro/custom model variants; assert exact pins, secret-free report lines, byte-identical no-op, and transaction write failure. |

Runner/read-side fixtures additionally cover absent/blank/malformed/mixed JSONL,
attempt-ID split/replay, all status/duration/line-count fields, parser `--`
and precedence, ambiguous name failure, sequential selection, writer flush
rows, and fake-platform warning/cap/TERM/KILL paths.  Cutover integration
tests add `maint` to the native process usage-anchor probe, use it as the
poison-harness negative subject, and retain `backup` as the live Python
positive control.  The process census fixture/hash is reviewed but is expected
to remain unchanged because `PROCESS_SPECS` remains the same provenance row.

## Ordered implementation slices

1. Add the `solstone-core-maint` crate, static registry metadata, parser and
   rendering, static-registry-aware system-health read model, doctor integrity
   adaptation, and direct flush-only attempt writer.  Wire a CLI-only runner
   with no live body dispatch.
2. Add private `__maint-worker --one-task` dispatch and parent runner seams;
   implement fake clock/child tests for line capture, warnings, cap, TERM/KILL,
   JSONL, lookup, and pending/retry selection.
3. Establish missing bounded owners: convey-config (including relocation of
   existing limited config/convey writers) and talents.  Implement retired
   no-ops and simple JSON/JSONL/path bodies: activities, observer, settings
   003/004/005/006/008, sol 001/004, and search 004.
4. Implement segment/facet/entity/import bodies: entities 001, settings
   001/002/007, sol 000/002, preserving each migration's preflight and
   destructive/refusal policy.
5. Implement schedule, provider, and SQLite bodies: search 003; sol
   005/007/008/009; timeline rollup; thinking 000/001/002.  Add the missing
   local legacy-proof bridge and constrained system-health cleanup API.
6. Wire `solstone-core` public/private commands and only then add `maint` to
   `NATIVE_PROCESS_SPECS`.  Add the native contract usage anchor/probe,
   poison-harness cutover test, and evaluate the unchanged process-census
   fixture/hash.  Remove the Python process dispatch only when those contract
   tests prove the native route; source Python bodies are removed only in the
   separately authorized cleanup change.

## Risks and resolved questions

* The only material ownership gap was `config/convey.json`; existing writers
  are narrow private side effects, so a bounded owner crate plus their
  relocation is necessary to avoid an L2 violation.  This is a dependency for
  three settings bodies, not an optional polish task.
* `stats.json` ownership is now resolved to `solstone-core-journal-stats-cli`,
  not facets/indexer.  Search event JSONL and stats JSON are two domain calls
  behind one static task.
* `sol:001` and `sol:004` have no existing native journal talent-layout owner;
  `solstone-core-talents` is required.  Journal-io remains a primitive-only
  dependency.
* `thinking:001` has no current native legacy-truth bridge.  Its public proof
  trait must expose only verified/nonsecret evidence; no Python bridge or
  shell-out is permitted.
* The public `Unreadable` status is removed, but its doctor behavior is
  preserved through `MaintStateIntegrity::Unreadable` exactly as specified.
  This resolves the acceptance-criterion/Python-parity tension.
* No user decision is blocking implementation.  The implementation owner
  should, before slice 3, confirm the desired crate name (`convey-config` and
  `talents`) with maintainers if workspace naming policy requires a different
  spelling; their responsibilities and API boundaries are fixed by this
  design.
