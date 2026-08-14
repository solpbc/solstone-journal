# Native `journal maint` prep notes

Research date: 2026-08-14.  This is a temporary engineering note, not a
product design document.  Product source was not changed.

## Scope census

`rg --files solstone/apps | rg '/maint/[0-9].*\\.py$'` finds 27 bodies:
activities 1, entities 1, observer 1, search 2, settings 8, sol 9, thinking
3, and timeline 2.  Discovery intentionally excludes `__init__.py` because
it starts with `_`.

## Python owner and command contract

### Runner (`solstone/think/maint.py`)

`MaintTask` is `{ app: str, name: str, script_path: Path, description: str =
"", retry_on_next_start: bool = False, blocks_supervisor_start: bool = False
}`; `qualified_name` is `"{app}:{name}"` ([maint.py:46](../../solstone/think/maint.py:46)).
`MaintTaskResult` is frozen and records task, success, exit_code, state_file
([maint.py:63](../../solstone/think/maint.py:63)).

`discover_tasks()` scans `Path(__file__).parent.parent / "apps"`, skips
non-directories and underscore app/script names, and constructs records from
`apps/*/maint/*.py`. It reads the file as raw text, finds the first triple
double or triple single quote anywhere after the license header, and takes its
first stripped line as the description; it does not import a task body. It
then AST-parses top-level simple `Assign` or `AnnAssign` constants only:
`MAINT_RETRY_ON_NEXT_START` and `MAINT_BLOCKS_SUPERVISOR_START`, and accepts
only literal boolean `ast.Constant`s. Any discovery exception is swallowed.
The actual sort is `key=(task.name, task.app)`, despite the module prose saying
“task name (app as tiebreaker)” and the function docstring saying `(app,name)`
([maint.py:73](../../solstone/think/maint.py:73), [maint.py:130](../../solstone/think/maint.py:130)).

State path is exactly `<journal>/maint/<app>/<task>.jsonl`
([maint.py:161](../../solstone/think/maint.py:161)). JSONL input ignores blank
and malformed lines; attempts are split by every `exec`, and also when a
non-exec event has a different string `attempt_id`; readers use only the last
block ([maint.py:249](../../solstone/think/maint.py:249)). A missing state file
is `pending`; a last `exit` of zero is `success`, any other/missing
`exit_code` is `failed` with default `-1`; an exec without final exit is
`in_progress`; a present but unusable file falls through to `in_progress`
([maint.py:206](../../solstone/think/maint.py:206)). State metadata derives
latest-attempt duration from an integer exit `duration_ms`, counts all line
events, and uses exit timestamp else first exec timestamp
([maint.py:166](../../solstone/think/maint.py:166)).

`run_task()` makes `<journal>/maint/<app>`, creates a random hexadecimal
`uuid.uuid4().hex` attempt ID, and launches exactly
`[sys.executable, "-m", "solstone.apps.{app}.maint.{name}"]`
([maint.py:322](../../solstone/think/maint.py:322)). It appends and flushes:

```json
{"event":"exec","attempt_id":"...","ts":<epoch-ms>,"app":"...","task":"...","cmd":[...]}
{"event":"line","attempt_id":"...","ts":<epoch-ms>,"line":"<stdout-minus-final-newline>"}
{"event":"exit","attempt_id":"...","ts":<epoch-ms>,"exit_code":<integer>,"duration_ms":<integer>}
```

On a stall, the exit row additionally has `"error":"stalled"`; an exception
trying to execute writes an exit row containing `exit_code: -1` and
`error: str(exception)` but no duration. It combines stderr into stdout,
uses text line buffering, has a daemon stdout-reader thread and a queue,
continues draining after process exit, and prints every captured output line as
`"  {line}"`. `poll_interval = min(0.5, stall_warn_interval_sec / 4)`.
The exact default literals are `stall_warn_interval_sec: float = 30.0` and
`stall_hard_cap_sec: float = 120.0`; each quiet 30-second interval logs
`Maint task stalled: %s (no output for %.1fs)`, then at 120 seconds logs a
hard-cap error and terminates ([maint.py:327](../../solstone/think/maint.py:327),
[maint.py:426](../../solstone/think/maint.py:426)). `_terminate_with_grace()`
calls `terminate()`, waits **5** seconds, then `kill()`, waits another **5**
seconds, logs `Maint task unkillable: %s`, and returns `-signal.SIGKILL` if
that too times out ([maint.py:308](../../solstone/think/maint.py:308)). It
emits `convey/maint_start` including app/task/description before execution;
normal `convey/maint_complete` includes app/task/exit_code/duration_ms/success,
while stall completion has error `stalled` and no duration.

`run_pending_tasks()` discovers in that ordering, runs exactly `pending` tasks
plus `failed` tasks opted into retry; it does not rerun `in_progress` or
ordinary failures, and continues sequentially after an unsuccessful task
([maint.py:585](../../solstone/think/maint.py:585)).

`get_task_by_name()` first matches an exact qualified name if a colon is
present. An unqualified name returns only when exactly one task shares the
stem. Multiple matches **only log a warning** (`Ambiguous task name '…', found
in: …`) and return `None`; `maint_cli` then turns `None` into its ordinary
hard CLI “Task not found” failure. So ambiguity is a hard CLI failure in
effect, but `get_task_by_name` itself is not an error-returning API
([maint.py:660](../../solstone/think/maint.py:660),
[maint_cli.py:71](../../solstone/convey/maint_cli.py:71)).

`blocks_supervisor_start` has no production runtime consumer: its only uses
outside definition/discovery are the two assertions in
[tests/test_maint.py:381](../../tests/test_maint.py:381) and :388. This was
confirmed by repository grep.

### CLI (`solstone/convey/maint_cli.py`)

Module usage comment is exactly:

```text
journal maint                    # Run pending tasks
journal maint --list             # Show status of all tasks
journal maint <task>             # Show task details and log output
journal maint --force <task>     # Re-run a specific task
```

Parser description is `Run maintenance tasks for apps`; it has optional
positional `task` (help: `Task to show details for (or to re-run with
--force)`), `--list`/`-l` (help: `List all tasks with their status`), and
`--force`/`-f` (help: `Re-run a specific task (requires task name)`). Its
RawDescription epilog is:

```text
Examples:
    journal maint              Run all pending maintenance tasks
    journal maint --list       Show status of all tasks
    journal maint chat:fix_x   Show task details and log output
    journal maint -f fix_x     Re-run a specific task
```

`setup_cli()` adds `-v`/`--verbose` (`Enable verbose output`) and `-d`/
`--debug` (`Enable debug logging`), calls `parse_args()` (not
`parse_known_args()`), then `init_cli_runtime(args.verbose,args.debug)`
([utils.py:1030](../../solstone/think/utils.py:1030)). Consequently `--` ends
option parsing: `journal maint -- --list` uses positional task named `--list`
and reports it missing; it is not list mode. Argparse permits the flags before
or after the positional in normal fashion. `--list` is checked first, so it
wins over both a supplied task and `--force`; `--list --force task` lists.

List order and labels are exactly Pending, In Progress, Failed, Completed.
`print_task()` prints `  {qualified_name}{optional ' - description'}{optional
' (in progress)' or ' (exit N)'}`; a run metadata line is `    ran
YYYY-MM-DD HH:MM` plus optional ` (duration, N lines)`. Duration formatting:
`<1000 => Nms`; `<60000 => floor(N/1000)s`; else
`floor(N/60000)m floor(N%60000/1000)s` ([maint_cli.py:32](../../solstone/convey/maint_cli.py:32)).

Details print qualified name, optional description, exact status line
(`Status: pending`, `Status: in progress`, `Status: success (exit 0)`,
`Status: failed`, or `Status: failed (exit N)`), optional `Ran: …` and `Log:
…`, blank line, then current attempt output. Older attempts follow a blank
line and `Prior attempt {index}:`; it reverses attempts, so current is first
and the immediately preceding one is `Prior attempt 2:`. `_read_attempt_logs`
starts a record on `exec` (and tolerates pre-exec rows), collects only string
line fields and exit errors, sets duration only from integer `duration_ms`,
and ignores blank/malformed JSON ([maint_cli.py:69](../../solstone/convey/maint_cli.py:69)).

Exact absence messages are `Task not found: {task_name}` and `Use 'journal
maint --list' to see available tasks.` (stderr, exit 1); `--force` without a
task emits `--force requires a task name.` and `Usage: journal maint --force
<task>`. Other terminal text is `No maintenance tasks found.`, `No pending
maintenance tasks.`, and `Completed {succeeded}/{ran} task(s)`
([maint_cli.py:193](../../solstone/convey/maint_cli.py:193)).

## Migration bodies and pinned Python behavior

The following is an exhaustive body census. “Paths” means journal-relative.
Tests listed are the existing direct tests; several older migrations have no
dedicated direct test and only runner coverage.

| Task | Current behavior, paths, safeguards | Existing test source / pinned cases |
|---|---|---|
| `activities:000_migrate_activity_icon_to_emoji` | Calls `migrate_custom_activity_icons_to_emoji(dry_run)` and reports changed records/files/scanned files. Actual owner controls activity config paths. | No direct task test found. |
| `entities:001_migrate_to_journal_entities` | Reads `facets/*/entities.jsonl`, skips `detached`; exact case-insensitive then RapidFuzz token-sort matching at `FUZZY_THRESHOLD = 90` using `>=`; longest name wins, aliases union, principal OR; preserves legacy files. Writes `entities/<slug>/entity.json` and `facets/<facet>/entities/<slug>/entity.json` (relationship strips global fields). | [tests/test_maint_001_migrate_to_journal_entities.py](../../tests/test_maint_001_migrate_to_journal_entities.py): no `facets/` returns `([],0)` and fresh migration zero summary; normal shape skips detached. |
| `observer:000_migrate_remote_to_observer` | Moves `apps/remote/remotes/**` to `apps/observer/observers/**`; identical target deletes source, different target logs/prints warning and leaves source; prunes empty legacy dirs. In `config/journal.json`, moves `observe.remote` entries into unset `observe.observer` keys then deletes remote. | No direct task test found. |
| `search:003_migrate_index_stream` | Read-only sqlite PRAGMA of `indexer/journal.sqlite`; no DB/table/current expected column set is no-op. Missing expected `{content,path,day,facet,agent,stream,idx}` triggers native reset/full scan. | [test_maint_migrate_index.py](../../solstone/apps/search/tests/test_maint_migrate_index.py): old rebuild, current/no-db no-op, reset failure still returns true. |
| `search:004_migrate_topic_to_agent` | Rewrites `facets/*/events/*.jsonl`: topic becomes agent unless agent already exists, then removes topic. Rewrites every `stats.json`: `topic_data/counts/minutes/counts_by_day` keys to agent keys without overwriting existing agent value. Per-file exceptions report and continue; dry-run never writes. | No direct task test found. |
| `settings:001_backfill_streams` | Classifies non-empty segments, repairs/writes `chronicle/<day>/**/<segment>/stream.json`, rebuilds `streams/<name>.json`. Signal precedence listed below. Existing same stream only has linkage repaired; exactly same stream/seq/prev skips. | No direct task test found. |
| `settings:002_restructure_stream_dirs` | Requires every nonempty direct child segment marker, then moves `<day>/<segment>` to `<day>/<stream>/<segment>`; deletes empty segment dirs (dry-run available), refuses with exit 1 before any change if markers missing; verifies post-count and tells user to rebuild index. Does not collision-check target before `shutil.move`. | No direct task test found. |
| `settings:003_seed_default_app_navigation` | Sets Convey state journal root, lock-mutates `config/convey.json` through `seed_default_app_navigation`; no change when keys exist; exit 1 on resolver/persist failures. | [test_seed_default_app_navigation.py](../../solstone/apps/settings/tests/test_seed_default_app_navigation.py): resolved journal not cwd, no rewrite, failure does not create config. |
| `settings:004_backfill_import_manifests` | For each `imports/<ts>`, skips an existing `manifest.json`; otherwise requires `import.json` `file_path` basename retained within import dir, hashes it and writes manifest using `imported.json` source type/files/days where present. | [test_maint_004_backfill_import_manifests.py](../../solstone/apps/settings/tests/test_maint_004_backfill_import_manifests.py): basename locator, retained-original refusal, byte-hash fields, idempotence. |
| `settings:005_pin_curation_nav` | Lock-mutates `config/convey.json`: appends `curation` to nonempty `apps.order` if absent and any `apps.starred` list if absent. Missing/malformed lists are preserved. | [test_maint_005_pin_curation_nav.py](../../solstone/apps/settings/tests/test_maint_005_pin_curation_nav.py): populated/empty variants, byte-identical no-op, persistence failure. |
| `settings:006_drop_services_nav` | Lock-mutates `config/convey.json`, filtering every `services` occurrence from list-valued `apps.order` and `apps.starred`. | [test_maint_006_drop_services_nav.py](../../solstone/apps/settings/tests/test_maint_006_drop_services_nav.py): duplicate removal, missing-lists no-op, byte-identical no-op, failure. |
| `settings:007_migrate_pdf_extractions` | Scans `chronicle/*/*/*/*.jsonl`. For document/image JSONL beside a readable nonempty transcript, deletes duplicate; header-only transcript refuses deletion. Orphan document JSONL with readable extracted text gets verified nonempty `document_transcript.md`, then deletion. Unparseable/no-text stays and is reported. | [test_maint_007_migrate_pdf_extractions.py](../../solstone/apps/settings/tests/test_maint_007_migrate_pdf_extractions.py): duplicate/image removal, cross-stream conversion/idempotence, unconvertible and header-only preservation. |
| `settings:008_migrate_pairing_home_address` | In `config/journal.json`, valid bare `http://host:port[/]` (no creds/query/fragment/path) is validated and moved from `pairing.host_url` to `home_address`; host_url always removed if present, invalid value does not overwrite an existing new value. | [test_maint_008_migrate_pairing_home_address.py](../../solstone/apps/settings/tests/test_maint_008_migrate_pairing_home_address.py): valid/invalid/existing/noop shapes. |
| `sol:000_migrate_agent_layout` | Moves segment root markdown plus known JSON and facet activity-state into segment `agents/`; moves daily `agents/<topic>_<facet>.<md/json>` into `agents/<facet>/<topic>`. Identical destination deletes source; nonidentical skips; dry-run. | No direct task test found. |
| `sol:001_migrate_agent_run_logs` | Under root `agents/`, removes `.jsonl` symlinks, moves flat run JSONLs into named directories (unnamed defaults `chat`), creates latest completed `<name>.log` symlinks, and appends nonduplicate day indexes. Existing destination skips rather than overwrites. | No direct task test found. |
| `sol:002_migrate_chronicle` | Moves root YYYYMMDD dirs to `chronicle/`; if target exists, copytree-merge then deletes root source. Afterwards deletes index sqlite, WAL, SHM and validates no root days/sqlite survive. | No direct task test found. |
| `sol:004_rename_agents_to_talents` | Plans root/day/segment `agents -> talents` plus `health/agents.json -> health/talents.json`. **Any collision aborts all moves**, exit 2; destination-only is already migrated; dry-run counts planned as moved. | [tests/test_maint_004_rename.py](../../tests/test_maint_004_rename.py): four-path move, all-or-nothing collision, destination-only no-op. |
| `sol:005_migrate_dream_to_think_schedules` | Reads `config/schedules.json`, only rewrites dict `cmd` arrays starting `['sol','dream']` to `['journal','think']`, writes only changed named entries; missing/empty/malformed skips without write. | [tests/test_maint_005_migrate_dream_to_think_schedules.py](../../tests/test_maint_005_migrate_dream_to_think_schedules.py): preserve metadata/order/bytes/mtime, dry-run, write failure. |
| `sol:006_rename_unified_triage_providers` | Confirmed retired no-op: `run_migration` discards both inputs and always returns all-zero `MigrationSummary(skipped_reason='retired')`. | [tests/test_maint_006_rename_unified_triage_providers.py](../../tests/test_maint_006_rename_unified_triage_providers.py): normal and dry-run retain bytes/mtime. |
| `sol:007_migrate_sol_service_schedules` | Rewrites stale `sol <service>` to `journal <service>` only for frozen service list; special `sol import --sync` becomes `journal importer …`; untouched entries preserve order/values; malformed/missing/empty skips. | [tests/test_maint_007_migrate_sol_service_schedules.py](../../tests/test_maint_007_migrate_sol_service_schedules.py): all stale forms, non-services, special backends, metadata, idempotence, dry-run/failure. |
| `sol:008_migrate_provider_check_schedule` | Retry opt-in. Installs `brain` refresh from old provider schedule cadence/enabled policy unless exact brain exists; removes all provider-check matches; only after schedule processing deletes health `talents.json`, `.lock`, `recheck.lock`, `providers.log`, and `.talents.json.*.tmp`. Cleanup failure is error/retryable. | [tests/test_maint_008_migrate_provider_check_schedule.py](../../tests/test_maint_008_migrate_provider_check_schedule.py): coalescing, no duplicate brain, cleanup ordering/failure, malformed no cleanup. |
| `sol:009_remove_granola_sync_schedule` | Retry opt-in only. Removes only exact key `sync:granola` when missing cmd or a recognized journal/sol importer/import `--sync granola` spelling; owner-divergent value is preserved with warning. Does not touch `health/scheduler.json`. | [tests/test_maint_009_remove_granola_sync_schedule.py](../../tests/test_maint_009_remove_granola_sync_schedule.py): current/legacy/equal syntax, exact-key policy, mtime/noop, error/CLI exit, scheduler untouched. |
| `thinking:000_unify_provider_config` | Transactionally rewrites `config/journal.json`: selects active from active/cogitate/generate then cloud env GOOGLE→ANTHROPIC→OPENAI then local; moves context disabled/extract overrides, confidential prior fields, key validation, removes retired fields; separately removes `.config/vertex-credentials.json`. | [test_provider_config_migration.py](../../solstone/apps/thinking/tests/test_provider_config_migration.py): precedence, malformed recovery, confidential and exact cleanup. |
| `thinking:001_migrate_provider_install_state` | Retry + supervisor-blocking opt-ins. Delegates provider-owned migration that promotes verified old install truth/status then moves local Vulkan override and cleans legacy bundled config; busy/proof unavailable/mismatch are non-destructive retry states. | [test_provider_install_state_migration.py](../../solstone/apps/thinking/tests/test_provider_install_state_migration.py): opt-ins, ready/mismatch/proof/busy/cache-write behavior. |
| `thinking:002_pin_google_model_aliases` | Retry + supervisor-blocking. Transactionally pins exact Google aliases and reports secret-free history lines; pro alias remains advisory (`choose exact Gemini model`). | [test_google_model_pin_migration.py](../../solstone/apps/thinking/tests/test_google_model_pin_migration.py): slots/pro/custom IDs, no-op byte preservation, atomic transaction failure. |
| `timeline:002_migrate_rollup_schedules` | Deletes only values exactly equal to the two legacy rollup entries; divergent owner entries are warned/preserved; absent is no-op. | [test_migrate_rollup_schedules.py](../../solstone/apps/timeline/tests/test_migrate_rollup_schedules.py): exact/divergent/absent/idempotent/dry-run. |
| `timeline:002_register_segment_summary_model` | Confirmed retired no-op: inputs ignored and always all-zero `RegistrationSummary`, regardless of normal/dry run. | [test_register_segment_summary_model.py](../../solstone/apps/timeline/tests/test_register_segment_summary_model.py): no write and main tolerates malformed config. |

### `settings:001_backfill_streams`: exact nine signals

The precedence, including the important placement of host after all import
signals, is: (1) existing `stream.json` with stream; (2) `audio.jsonl` or a
non-imported `*_audio.jsonl` header `stream`; (3) that header’s `remote` via
`stream_name(remote=...)`; (4) that header’s truthy `imported`, source from
`raw` extension (`.m4a=apple`, `.txt/.md/.pdf=text`, else audio); (5)
`imported_audio.jsonl`, same raw extension decision; (6) reverse index from
`imports/*/segments.json`, source from `import.json` filename then MIME then
audio; (7) **only `audio.jsonl`** header host via `stream_name(host=...)`; (8)
tmux-only capture (`tmux_*_screen.jsonl`, no recognized audio), host qualifier
`tmux`; (9) fallback host from explicit `--host`, first observer stream state,
or `socket.gethostname`, with hostname stripping. It then groups by stream,
sorts `(day,segment)`, assigns 1-based seq and preceding day/segment, and
rebuilds state preserving existing type/host/platform/created_at when it can
([001_backfill_streams.py:171](../../solstone/apps/settings/maint/001_backfill_streams.py:171)).

## Existing native read side and process cutover seams

`MaintTaskStatus` is `Pending`, `InProgress`, `Unreadable`, `Success`,
`Failed`; `MaintTaskState` has only app/task/status/exit_code/ran_ts
([maint.rs:11](../../core/crates/solstone-core-system-health/src/maint.rs:11)).
`read_maint_task_states()` walks existing sorted `maint/<app>/*.jsonl`, so
source tasks without a state file are invisible. `read_maint_task_state()`
constructs the matching state path and treats a missing one as pending.
`latest_attempt_events()` parses object JSON only and follows the same exec and
attempt-ID split rule, returning last block. It currently lacks source task
discovery and therefore description and opt-ins; state metadata has no
duration_ms or line_count; no record carries log lines/attempt replay. These
must be added for acceptance criteria 4–5
([maint.rs:29](../../core/crates/solstone-core-system-health/src/maint.rs:29)).

Doctor consumes only failed tasks (fail, with `app.task (exit N)`), old
in-progress tasks (warn if `now - ran_ts > 300_000`), then unreadable or
timestamp-less in-progress tasks (warn); all else OK
([journal_maint_tasks.rs:7](../../core/crates/solstone-core-doctor/src/checks/journal_maint_tasks.rs:7)).

`PROCESS_SPECS` still maps `maint` to Python `solstone.convey.maint_cli`
([processes.rs:504](../../core/crates/solstone-core-journal-cli/src/processes.rs:504)).
Native examples: `backup -> solstone-core [backup]`, `maintenance ->
solstone-core [maintenance]`, `brain -> solstone-core
[private-owner-sentinel,brain]` ([processes.rs:59](../../core/crates/solstone-core-journal-cli/src/processes.rs:59)).
`process_spec_for` searches Python/table records; `native_process_spec_for`
searches explicit native records. `dispatch_process()` first gets Python spec
and runs installation coherence for service/alias, then uses the native record
if present (sibling native executable + preset argv + owner args); otherwise it
uses sibling Python and its fixed bootstrap ([lib.rs:201](../../core/crates/solstone-core-journal-cli/src/lib.rs:201)).

The self-spawn model is `env::current_exe()`, private subcommand `cogitate`
and private flag `--one-shot`, with piped stdio. A worker thread writes stdin
then waits, sends child PID through sync channel, while outer receives PID and
result under one 60-second deadline and SIGKILLs timeout
([brain_owner.rs:565](../../core/crates/solstone-core/src/brain_owner.rs:565)).

The requested settings poison template has `struct Harness`, not
`PoisonHarness`; it copies journal/core binaries into a temp `bin`, installs
poison `python/python3/pytest/uv/ruff`, runs with its marker env var, and has
`assert_python_was_not_invoked` ([journal_settings_native_cutover.rs:79](../../core/crates/solstone-core-journal-bin/tests/journal_settings_native_cutover.rs:79)). Its positive Python control is `backup`, not maint:
`poison_remains_live_for_a_python_token` runs `journal backup` and asserts exit
97, poison marker exists ([journal_settings_native_cutover.rs:482](../../core/crates/solstone-core-journal-bin/tests/journal_settings_native_cutover.rs:482)). Therefore maint can become the negative/cutover subject; backup stays a valid positive control today.

The native process contract uses a `const FOO_USAGE_ANCHOR: &[u8] = b"usage:
…\n";` and a `Probe { token, argv: &["--nonsense"], expected_exit: 2,
stderr_anchor: Some(FOO_USAGE_ANCHOR) }` entry. Current `MAINTENANCE_USAGE_ANCHOR`
and probe cover `maintenance`, not `maint`; add parallel `MAINT_USAGE_ANCHOR`
and `maint` probe at the same locations
([journal_native_process_contract.rs:152](../../core/crates/solstone-core-journal-bin/tests/journal_native_process_contract.rs:152)).

The process census fixture is an exact four-field JSON record inventory and
its test requires schema/provenance/counts, calculates an ordered length-
prefixed SHA-256 over kind/token/module/surface/preset argv, and compares the
production `PROCESS_SPECS` digest to fixed
`088aa7609828a9074ca68cd446ca2616d3f5b76e0b39ac424b9690ade1e8a8cd`
([process_census_fixture.rs:13](../../core/crates/solstone-core-journal-cli/tests/process_census_fixture.rs:13)). A native cutover changes no Python census entry unless the fixture/contract deliberately evolves; retain and update its hash only as specified by the census policy.

## Domain authority verification and proposed routing

The referenced scope pack itself is not present in this working tree, so I
could not verify its *quoted* function names. I verified actual APIs and line
locations instead. `install_file` is in **solstone-core-journal-io**
`src/atomic.rs:354`, not local/import. `solstone-core-system/src/schedule/config.rs`
already owns `remove_schedule_entry(path,name)` at line 110; it has no general
“rewrite arbitrary schedule map” operation, so bulk schedule changes need a
narrow explicit edit function or repeated named setters, not raw journal I/O.
Other confirmed reusable seams include config-write
`mutate_journal_config` ([config.rs:72](../../core/crates/solstone-core-journal-config-write/src/config.rs:72)),
indexer `reset_index` ([db.rs:130](../../core/crates/solstone-core-indexer-store/src/db.rs:130)),
observer `save_observer` ([write.rs:11](../../core/crates/solstone-core-observer/src/store/write.rs:11)),
and local install `write_status` ([status.rs:338](../../core/crates/solstone-core-local/src/install/status.rs:338)).

The required per-body routing table follows. `new` means no existing API was
found that owns precisely this historical migration without exposing unsafe
generic raw writes; signatures are intentionally narrow.

| Task | Domain crate | Reuse or proposed narrow API | File |
|---|---|---|---|
| activities:000 | facets | new `migrate_custom_activity_icons_to_emoji(journal: &Path, dry_run: bool) -> Result<ActivityIconMigration, FacetWriteError>` | `solstone-core-facets/src/store/activities.rs` |
| entities:001 | entity + facets + entity-matching | reuse `entity_slug`; new `migrate_legacy_facet_entities(journal:&Path, threshold:u8, dry_run:bool) -> Result<..., ...>` coordinating `create_journal_entity` and `save_facet_entity_link` | `entity/src/store/`, `facets/src/store/`, new coordinator under journal CLI owner |
| observer:000 | observer + journal-config-write | new `migrate_remote_observer_storage(journal:&Path)->Result<...>`; reuse config mutation | `observer/src/store/remote_migration.rs` (new); config `config.rs` |
| search:003 | indexer-store | reuse `reset_index(journal)` plus `scan_journal(journal,true)` | `indexer-store/src/db.rs`, `scan.rs` |
| search:004 | facets + indexer-store | new `migrate_topic_keys_to_agent(journal:&Path,dry_run:bool)->Result<...>` | `facets/src/store/events_migration.rs` (new); stats owner must be confirmed before write |
| settings:001 | segment | new `backfill_stream_records(journal:&Path, host:Option<&str>, verbose:bool)->Result<...>` | `segment/src/stream_repair.rs` |
| settings:002 | segment + journal-io | new `restructure_segments_by_stream(journal:&Path,dry_run:bool)->Result<...>` using `rename_within`/owned delete | `segment/src/relocate.rs` |
| settings:003 | journal-config-write | new `seed_default_convey_navigation(config:&mut Map)->bool` through `mutate_journal_config` (note current path is `config/convey.json`, requiring its actual owner decision) | `journal-config-write/src/` new convey module |
| settings:004 | import | reuse `write_manifest(ManifestWriteRequest)`; new retained-import scan/backfill wrapper | `import/src/dedupe.rs` |
| settings:005 | journal-config-write | new `pin_convey_curation(config:&mut Map)->bool` | `journal-config-write/src/` new convey module |
| settings:006 | journal-config-write | new `drop_convey_services(config:&mut Map)->bool` | `journal-config-write/src/` new convey module |
| settings:007 | segment + journal-io | new `migrate_pdf_extractions(journal:&Path)->Result<...>` with verified replacement before deletion | `segment/src/` new document migration module |
| settings:008 | journal-config-write + sol-link | new `migrate_pairing_home_address(config:&mut Map)->bool`; reuse link address validation if public | `journal-config-write/src/config.rs`; `sol-link/src/pairing/addresses.rs` |
| sol:000 | segment + journal-io | new `migrate_agent_layout(journal:&Path,dry_run:bool)->Result<...>` | `segment/src/relocate.rs` |
| sol:001 | journal-io | new `migrate_talent_run_logs(journal:&Path,dry_run:bool)->Result<...>` | `journal-io/src/` new talent-log module (or sol owner if it owns talents) |
| sol:002 | journal-io + indexer-store | new `migrate_root_days_to_chronicle(journal:&Path,dry_run:bool)->Result<...>`; index reset/delete belongs indexer owner | `segment/src/relocate.rs`; `indexer-store/src/db.rs` |
| sol:004 | journal-io | new `rename_agents_to_talents(journal:&Path,dry_run:bool)->Result<...>` with preflight collision list | `journal-io/src/` new migration module |
| sol:005 | system schedule | new `rewrite_schedule_commands(path:&Path, edits:&BTreeMap<String,Vec<String>>)->Result<...>` | `system/src/schedule/config.rs` |
| sol:006 | none | native body should return retired zero summary; no domain access | maint runner crate |
| sol:007 | system schedule | reuse proposed `rewrite_schedule_commands` | `system/src/schedule/config.rs` |
| sol:008 | system schedule + journal-io | new `migrate_provider_check_schedule(path:&Path)->Result<...>`; cleanup must be explicit dated health-path removal | `system/src/schedule/config.rs` plus bounded journal-io helper |
| sol:009 | system schedule | reuse `remove_schedule_entry(path,"sync:granola")` after native guard | `system/src/schedule/config.rs:110` |
| thinking:000 | journal-config-write | reuse `mutate_journal_config`; new provider-specific transform, bounded credential delete | `journal-config-write/src/config.rs`; thinking provider helper |
| thinking:001 | local install/status + journal-config-write | reuse `write_status` / provider migration seams; new legacy truth bridge if absent | `local/src/install/status.rs`, `local/src/install/` |
| thinking:002 | thinking + journal-config-write | reuse provider config transform shape; new exact Google alias pin | `thinking/src/providers.rs` |
| timeline:002 rollup | system schedule | reuse `remove_schedule_entry` after exact-value guard | `system/src/schedule/config.rs:110` |
| timeline:002 register | none | native body should return retired zero summary; no domain access | maint runner crate |

## Baseline

Run through `hop check --allow-capture` as required:

* `cargo test --manifest-path core/Cargo.toml -p solstone-core-system-health --lib`: **15 passed, 0 failed**.
* `cargo test --manifest-path core/Cargo.toml -p solstone-core-doctor --lib`: **60 passed, 0 failed**. It completed successfully through `hop check`; the tool lost its final captured Cargo line, but the crate contains the corresponding 60 `#[test]` library cases (verified read-only) and no parameterized test macro in its lib test graph.

## Material contradictions / follow-ups

1. The requested “scope pack” is not in this checkout, so exact cited names in
   its §3 cannot be truthfully verified from it; actual current APIs are above.
2. `discover_tasks` behavior is `(name,app)` while its own function docstring
   says `(app,name)`.
3. Ambiguous unqualified task names log a warning then return `None`; the CLI
   makes that a generic not-found exit 1. Native acceptance must preserve that
   observable CLI failure (or deliberately add an exact ambiguity diagnostic).
4. `maint` remains Python in `PROCESS_SPECS` and is absent from
   `NATIVE_PROCESS_SPECS`. The contract test’s required-token list contains
   `maint`, but its actual usage probe only covers `maintenance`; add the
   missing maint anchor/probe on cutover.
5. The cited poison template’s type is `Harness`, not `PoisonHarness`; its
   current live-Python positive control is `backup`.
