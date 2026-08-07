# Native Indexer Atomicity And Reset

This is the review-gate design for making the native Rust indexer's compound
SQLite writes atomic and replacing file-deletion reset with a SQLite-native
transactional reset. It does not implement the changes.

## Decisions And Scope

- Do not add the rusqlite `hooks` feature. `rusqlite 0.40.1` is pinned with
  workspace features `["bundled"]`; there is no public
  `Connection::set_authorizer`, and the available `Connection::authorizer` API
  is behind the currently disabled `hooks` feature.
- Use `Connection::transaction()` at logical write-unit boundaries. Existing
  helper signatures that take `&Connection` stay unchanged because
  `rusqlite::Transaction<'_>` derefs to `Connection`.
- Treat `index_entity_search` as the fifth logical replacement unit. It has the
  same defect shape as file and edge replacement: destructive deletes, then many
  inserts, then watermark writes, all currently independent autocommits. The operator
  approved this D0 scope at the gate, and the native implementation now ships
  it.
- Replace `reset_index` file removal with transactional SQL reset: drop known
  index objects, recreate schema, and never unlink `journal.sqlite`,
  `journal.sqlite-wal`, or `journal.sqlite-shm`.
- Add `PRAGMA busy_timeout=5000` in `open_index`, next to the existing WAL and
  synchronous pragmas. This matches Python sqlite3's inherited 5 second default.
- Dedupe `EDGES_SCHEMA_PATH` and `EDGES_SCHEMA_VERSION` to one store-crate
  source of truth. Keep values unchanged; `core/fixtures/edge_schema.json` must
  not change.
- Close the silent `continue` in `rebuild_edges` on `file_mtime_secs` failure:
  warn, count it as failed, and make the command exit 75.

## Write Units

`rescan_file`:
- Open a mutable connection and wrap the selected content and/or edge
  replacement in a transaction.
- For content files, delete old chunks, insert new chunks, and update `files`
  inside the same transaction.
- For edge files, delete old edge rows and edge mtime, extract, insert edge
  rows, and update `edge_files` inside the same transaction.
- On non-benign failure, roll back and return an error so `main.rs` exits 75.
- `invalid_segment` is benign here too: skip edge insertion, advance the
  `edge_files` mtime inside the committing transaction, warn, and exit 0.

`scan_journal` per-file content replacement:
- Keep discovery and mtime reads outside write transactions.
- For each changed content file, open a per-file transaction. Delete old chunks,
  index the file, update `files`, then commit.
- If a content production warning occurs before any successful replacement,
  roll back that file transaction, increment `skipped`, warn, and continue.
- If a SQL error occurs, roll back that file transaction and let the error
  propagate exactly as it does today.

`scan_journal` removed-file phase:
- For each removed content file, open a transaction, delete chunks and the
  `files` row, then commit. SQL failures roll back and propagate exactly as
  today.

`scan_journal` edge reconciliation:
- Open one transaction for the reconciliation phase.
- Hoist the savepoint from `insert_normalized_edges` to the whole edge-file
  replacement: delete old edge rows and `edge_files`, extract, insert rows, and
  update `edge_files`.
- A failed edge file rolls back only its savepoint, increments `failed`, warns,
  and allows sibling edge files and removed-edge cleanup to continue. This is a
  warning-only scan failure and still exits 0.
- Use raw SQL savepoints here, not `Transaction::savepoint()`, because the
  selected idiom keeps helpers on `&Connection`; high-level rusqlite savepoints
  require mutable access. Every path must issue a matching rollback/release or
  release before continuing.
- Remove the raw savepoint from `insert_normalized_edges`; once whole-file
  savepoints exist, insert-batch isolation is the wrong boundary.

`rebuild_edges`:
- Open one transaction for the full rebuild. Delete `edges` and `edge_files`,
  write the schema sentinel, process every edge file, and commit only if there
  were no non-benign failures.
- Invalid segments remain skipped and warned. Drops remain counted. Extraction
  failures, SQL errors, and mtime read failures make the rebuild non-committable.
- If any non-benign failure was recorded, roll back the full rebuild, return a
  report with `failed > 0`, and make `main.rs` exit 75 after printing warnings.

`index_entity_search`:
- Keep it private and callable only from `scan_journal`.
- Wrap the three deletes, row inserts, and two watermark `REPLACE`s in one
  transaction at the caller frame.
- On SQL failure, roll back entity-search rows and watermarks and propagate the
  error exactly as today. This unit does not increment `ScanReport.failed`.

## Reset Design

`reset_index` should open the DB instead of deleting the file. It should run one
transaction that drops known index objects and recreates the schema. The normal
path drops the FTS5 virtual table with `DROP TABLE chunks`; SQLite drops that
table's shadow tables itself. Do not hand-drop `chunks_data`, `chunks_idx`, or
other FTS5 shadow tables while the virtual table exists.

Use a distinct orphan-shadow cleanup branch only when the virtual table is
absent but one or more known FTS5 shadow tables remain, which is reachable from
the deliberately incomplete-schema reset case. In that branch, drop orphaned
shadow tables directly in deterministic order: `chunks_config`,
`chunks_docsize`, `chunks_content`, `chunks_idx`, `chunks_data`.

The full deterministic reset order is:
- drop the `chunks` virtual table if present;
- if `chunks` is absent, run the orphan-shadow cleanup branch above;
- drop edge indexes `edges_path`, `idx_edges_src`, and `idx_edges_dst`;
- drop `edges`;
- drop `edge_files`;
- drop `files`;
- run the shared schema DDL body and commit.

`ensure_schema` currently takes `&mut Connection` and opens its own transaction,
so reset cannot call it from inside another transaction. Split the DDL body into
a new non-shim helper over `&Connection`. `ensure_schema` remains a thin caller
that opens a transaction and invokes the shared DDL body; `reset_index` opens
its own transaction, performs drops, invokes the same DDL body with `&tx`, and
commits.

The reset remains SQLite-native and should not unlink the main DB, WAL, or SHM
files. It must work from an incomplete schema and leave `PRAGMA integrity_check`
and FTS5 integrity clean.

## Failure Disposition Matrix

| Unit | Marker | Rolls back? | Counted? | Warned? | Exit |
| --- | --- | --- | --- | --- | --- |
| `rescan_file` | `failed` | Whole rescan transaction | No report; returns error | stderr via `main.rs` | 75 |
| `rescan_file` | `invalid_segment` | No; marker commit remains | Existing skip semantics | Yes | 0 |
| `rescan_file` | drop | No | Not surfaced | No | 0 if otherwise clean |
| `rescan_file` | SQL error | Whole rescan transaction | No report; returns error | stderr via `main.rs` | 75 |
| `scan_journal` per-file | `failed` | Not applicable | Not applicable | Not applicable | Not applicable |
| `scan_journal` per-file | `invalid_segment` | Not applicable | Not applicable | Not applicable | Not applicable |
| `scan_journal` per-file | drop | Not applicable | Not applicable | Not applicable | Not applicable |
| `scan_journal` per-file | SQL error | Current file/removal transaction | Not counted; error propagates | stderr via `main.rs` | 75 |
| `scan_journal` edge reconcile | `failed` | Current edge-file savepoint | `ScanReport.failed += 1` | Yes | 0 |
| `scan_journal` edge reconcile | `invalid_segment` | No; marker commit remains | Existing skip semantics | Yes | 0 if otherwise clean |
| `scan_journal` edge reconcile | drop | No | Not surfaced in `ScanReport` | No | 0 if otherwise clean |
| `scan_journal` edge reconcile | SQL error while processing one edge file | Current edge-file savepoint | `ScanReport.failed += 1` | Yes | 0 |
| `scan_journal` edge reconcile | phase commit SQL error | Phase transaction | Not counted; error propagates | stderr via `main.rs` | 75 |
| `rebuild_edges` | `failed` | Full rebuild transaction | `EdgeRebuildReport.failed += 1` | Yes | 75 |
| `rebuild_edges` | `invalid_segment` | No if no failed files | `EdgeRebuildReport.skipped += 1` | Yes | 0 if otherwise clean |
| `rebuild_edges` | drop | No | `EdgeRebuildReport.drops += n` | No | 0 if otherwise clean |
| `rebuild_edges` | SQL error | Full rebuild transaction | Fatal error | stderr via `main.rs` | 75 |
| `index_entity_search` | `failed` | Not applicable | Not applicable | Not applicable | Not applicable |
| `index_entity_search` | `invalid_segment` | Not applicable | Not applicable | Not applicable | Not applicable |
| `index_entity_search` | drop | Not applicable | Not applicable | Not applicable | Not applicable |
| `index_entity_search` | SQL error | Entity-search transaction | Not counted; error propagates | stderr via `main.rs` | 75 |

The encoded marker split is exactly:
- `failed: true` comes from insertion failure after extraction and from
  extraction `Err`.
- `invalid_segment` comes from the pre-read invalid segment guard.
- Drops come from resolver empty/unresolved names and observation relations with
  falsy `target_entity_id`.

Add `failed: usize` to `ScanReport` so soft edge failed results are no longer
discarded. It counts only `EdgeProcessResult.failed` from the two existing
producers: insertion failure after extraction and extraction `Err`. It does not
count SQL errors, invalid segments, drops, or content production warnings.

`main.rs` keeps scan exit behavior unchanged: print all scan warnings as today
and return success when `scan_journal` returns `Ok(report)`, even when
`report.failed > 0`. SQL errors still escape as `Err` and return 75 as today.
For `rebuild_edges`, `main.rs` should print warnings and return
`EXIT_TEMPFAIL` when `report.failed > 0`. No new exit codes.

## Trigger Injection Tests

Do not use rusqlite authorizers or production test hooks. Add test-only helper
code in the Rust unit tests that creates temporary SQLite triggers on ordinary
tables:
- `BEFORE INSERT` on `files` to fail after content or entity-search chunk
  deletes and before mtime/watermark replacement.
- `BEFORE INSERT` on `edge_files` or `edges` to fail after edge deletes and
  before edge replacement commits.
- `BEFORE DELETE` on `files` or `edge_files` for removal-only units, because
  removed-file cleanup has no post-delete insert.

The helper must use triggers only, generate unique trigger names, and drop them
before the connection is closed when the test continues to reuse the connection.
It must not require feature flags, lock files, retry loops, sleeps, or
test-only production hooks.

## Test Rewrite List

Rewrite only the verified section 3.6 list.

`scan_observation_container_passthrough_fails_before_partial_insert`
(`scan.rs:906`):
- Keep `edges_indexed == 1` and `edge_rows_inserted == 0`.
- Add `report.failed == 1`.
- Assert no edge rows for the failed file.
- Change the `edge_files` assertion at `scan.rs:926-932` to expect no marker for
  the failed file.
- Add a second unchanged scan assertion proving the file retries because the
  mtime was not advanced.

`scan_edge_failure_deletes_prior_rows_advances_mtime_and_keeps_sibling`
(`scan.rs:992`):
- Rename to reflect preserved stale rows and no mtime advance.
- Keep sibling success assertions.
- Change `scan.rs:1038-1044` to expect the prior failed-file edge row remains.
- Change `scan.rs:1052-1058` to expect the failed file's prior `edge_files`
  mtime remains unchanged, not advanced.
- Add `report.failed == 1` and assert the scan still exits 0 through `main.rs`.

`scan_candidate_load_failure_warns_and_advances_mtime` (`scan.rs:1063`):
- Rename to no-advance.
- Change `scan.rs:1103-1109` to expect stale rows preserved.
- Change `scan.rs:1110-1116` to expect the old mtime marker preserved, not
  replaced.
- Add `report.failed == 1`, retry-on-unchanged scan coverage, and assert the
  scan still exits 0 through `main.rs`.

`reset_removes_only_main_database_file` (`db.rs:333`):
- Rename to transactional reset keeps database files and recreates schema.
- Change `db.rs:340-342` expectations: main DB, WAL, and SHM are not manually
  unlinked.
- Assert known tables and sentinel exist after reset.
- Add `PRAGMA integrity_check` and FTS5 integrity assertions.

`reset_semantics_remove_only_main_database` (`scan.rs:3114`):
- Do not delete it.
- Rewrite it to cover what the `db.rs` test cannot: `reset_index` followed by a
  full `scan_journal` reindexes correctly from empty.
- Assert reset preserves DB files, recreates schema, and leaves the store usable
  for a following full rescan.

## New Tests

- Post-delete trigger injection for content per-file replacement. Seed stale
  chunks and files row, force reindex, abort on `files` insert, and verify from a
  second connection that stale chunks and mtime remain.
- Post-delete trigger injection for edge reconciliation. Seed stale edge rows and
  `edge_files`, abort on `edge_files` or `edges`, verify from a second
  connection that the failed file is preserved and a sibling file can commit.
- Post-delete trigger injection for entity search. Seed stale entity-search rows
  and watermarks, abort on one watermark insert, verify from a second connection
  that stale rows and both watermarks remain.
- Removal-only trigger injection with `BEFORE DELETE` on `files` and
  `edge_files`, proving removed-file cleanup does not commit a chunk/edge delete
  without its marker deletion.
- Retry-and-converge on unchanged rescan for SQL-level failure and
  extraction-level failure. After a failed scan, remove the trigger or repair the
  source without changing mtime where possible, rerun, and assert convergence.
- Extraction-level failure coverage separate from SQL injection. Use malformed
  edge extraction and candidate-load failure to prove failed edge files preserve
  stale rows and retry.
- Reset from incomplete schema. Create a DB with only a subset of objects or
  stale shadow objects, run reset, assert schema, sentinel, `PRAGMA
  integrity_check`, and FTS5 integrity.
- AC6 busy timeout exit 75. Hold `BEGIN EXCLUSIVE` on a second connection and
  invoke the native indexer write path. With `busy_timeout=5000`, accept the
  roughly 5 second wall-clock cost. I do not see a deterministic cheaper test
  that both exercises production `open_index` timeout behavior and avoids a
  test-only hook or sleep.
- Re-verify `scan_invalid_segment_edge_file_skips_and_advances_mtime`
  (`scan.rs:1121`) and `invalid_markdown_isolated_during_scan` (`scan.rs:1494`)
  against the new per-file transaction semantics during implementation. Prep
  predicted they survive; if either asserts reversed behavior, rewriting it is
  in scope and does not require a new gate.

## Validation Plan

Use focused checks first:
- `cargo test --manifest-path core/Cargo.toml -p solstone-core-indexer-store`
- `cargo test --manifest-path core/Cargo.toml -p solstone-core`
- `make check-rust-ios`

Because D4 rejects the `hooks` feature, this design avoids a Cargo feature or
lock-file change. If implementation unexpectedly changes Cargo metadata or
`Cargo.lock`, escalate to full `make ci`.

## PORTING.md Native Routing Text

Add this text to `docs/PORTING.md` under `Indexer Native Write Routing`:

> Native indexer compound writes are atomic at the logical replacement-unit
> boundary. A content file replacement deletes old chunks, inserts new chunks,
> and writes its `files` mtime. An edge
> file replacement deletes old edge rows and `edge_files` state, extracts and
> inserts replacement rows, and writes the `edge_files` mtime as one unit.
> Entity search deletes stale entity-search chunks, inserts replacement chunks,
> and writes both watermarks as one unit. Reset is SQLite-native: it drops and
> recreates index objects transactionally and does not unlink the database, WAL,
> or SHM files.
>
> Command writes now use the native path only. Journals containing edge source
> files whose extraction fails preserve prior native `edge_files` rows and mtime
> so the unchanged file retries on the next scan. The remaining Python
> in-process bypass consumers keep their existing Python semantics because they
> do not enter `journal indexer`.

## Risks And Open Questions

- D0 is included, approved, and shipped: `index_entity_search` is covered by the
  same transaction model because it has the same destructive replacement defect
  and only one call frame.
- Removal-only trigger injection needs `BEFORE DELETE` triggers, not only
  `BEFORE INSERT`, because those units have no post-delete insert.
- `ScanReport.failed` is a public struct-field addition. Existing callers should
  be updated directly; do not add compatibility shims.

## Gate Summary

- Boundaries: `rescan_file` one transaction; `scan_journal` per content file and
  per removal transaction; `scan_journal` edge phase one transaction with
  per-edge-file savepoints; `rebuild_edges` one full transaction; entity search
  one transaction.
- D0: include `index_entity_search`; it has three destructive deletes, N chunk
  inserts, and two watermark writes that must co-commit.
- Failure matrix: only `rescan_file` failed results and `rebuild_edges`
  non-benign failures change exit disposition to 75. `scan_journal` soft edge
  failures warn, continue, set `ScanReport.failed`, and still exit 0. Scan SQL
  errors propagate as `Err` and return 75 exactly as today. Invalid segments are
  benign everywhere, including `rescan_file`; drops stay non-fatal.
- Test rewrites: `scan.rs:906`, `scan.rs:992`, `scan.rs:1063`, `db.rs:333`,
  and `scan.rs:3114` only.
- New tests: trigger injection after destructive deletes, second-connection
  verification, integrity checks, retry-and-converge, extraction-level failure,
  incomplete-schema reset, and AC6 held `BEGIN EXCLUSIVE` busy-timeout exit 75.
- PORTING.md text: native compound writes are atomic; native reset is
  SQLite-native; edge extraction failures intentionally differ because native
  preserves prior rows and mtime while Python may advance `edge_files`; journals
  with no failing edge sources must stay byte-identical.
