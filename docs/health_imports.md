# body imports

This document is the current engineering boundary for Apple Health and Oura
body data entering the journal. The filename is retained for links; `health` in
owner-facing language means the journal system's health, while physiological
records are body data.

## current ownership

Rust owns every production mutation in this lane:

- `solstone-core-body-ingest` reads Apple exports, performs Oura OAuth and API
  sync, normalizes rows, computes identities, publishes immutable native
  bundles, advances the Oura cursor, and requests dedupe rebuilds.
- `solstone-core-body-source` owns the normalized row, hash, manifest,
  envelope, and ledger contracts.
- `solstone-core-body-store` replays row-agreed ledger events into dedupe
  state; `solstone-core-body-rebuild` publishes that state as
  `imports/health-dedupe.sqlite`.
- `solstone-core-journal-io` owns the filesystem mutation mechanics around
  bundle publication, cursor/config writes, locks, and database replacement.

Python remains in two non-writing roles: `body_native.py` selects and validates
the version-matched native process, while `apple_health.py` and `oura.py` keep
read-only detect/preview parsers. There is no Python Apple/Oura writer,
network client, OAuth/token owner, cursor writer, or dedupe writer. Native
coverage for Apple save/preview, bounded ZIP/symlink detect refusals, and
Oura missing-token refusal lives in
`core/crates/solstone-core/tests/body_cli.rs`.

## owner commands

The existing dispatcher reaches the native owner:

```text
journal importer /path/to/apple_health_export --confirm-body-save
journal importer --connect oura
journal importer --sync oura
journal importer --sync oura --save --confirm-body-save
```

`--dry-run` / catalog mode makes no journal, token, cursor, bundle, or derived
database mutation. The Apple preview and Oura catalog tests in
`solstone-core-body-ingest` exercise that boundary. `--confirm-health-save`
remains a hidden compatibility alias; new instructions use
`--confirm-body-save`. Apple day-summary transcript generation is retired: the
body app reads the normalized bundle rows directly.

The underlying native commands are:

```text
solstone-core body apple --source PATH [--date-from DAY] [--date-to DAY] [--force] [--save --confirm-body-save]
solstone-core body oura connect
solstone-core body oura sync [--window-days N] [--save [--confirm-body-save | --scheduled]]
solstone-core body rebuild
```

## durable native bundle

Each nonempty save publishes one immutable `imports/body-<ULID>/` directory:

```text
body-bundle.json
body-ledger.jsonl
body-raw-inventory.jsonl        # only when raw assets are retained
manifest.json
normalized/<YYYY-MM>.jsonl
raw/...                         # only when the approved policy retains it
```

The manifest and envelope bind the source family, source hash, raw-retention
decision, affected days, row count, shard inventory, and ledger digest. The
ledger binds every normalized row to its identity/value hashes and physical
reference. When raw assets are retained, a canonical inventory records every
path, byte count, and SHA-256 digest; its own digest is bound into every
normalized row. Rebuild verifies the complete raw inventory before accepting
those rows, so a missing, replaced, or truncated retained source fails closed.
Publication is staged, fsynced, and renamed only after the complete bundle
validates.

`imports/health-dedupe.sqlite` is derived state, not source history. It is
excluded by the existing `*.sqlite*` backup rule and rebuilt atomically from
the immutable bundles. Restore runs this rebuild before saving destination,
recovery-key, or recovery-confirmation state or reporting success; a torn or
invalid native bundle records a `body_rebuild_failed` restore outcome rather
than returning an empty body history. The `*.sqlite*` exclude list is pinned
by `solstone-core-backup-runtime`'s `excludes_are_exact_and_keep_durable_health`.
Torn native history is refused
by `body_rebuild_command_emits_machine_result_and_refuses_torn_native_history`
in `core/crates/solstone-core/tests/body_rebuild.rs`. Rebuild reconstructs
Apple and Oura rows in
`solstone-core-body-rebuild/tests/rebuild.rs`. There is no live restic
backup/restore/rebuild end-to-end in `core/`.

## source behavior

Apple Health accepts a directory or zip containing `export.xml`, including
DTD-bearing Apple exports without resolving external entities. Record and
Workout elements stream through a 1 MiB per-event parser. A save first makes a
private, bounded source snapshot; parsing, source hashing, and approved raw
retention all use that one snapshot. One import accepts at most 50,000,000 XML
events and 50,000,000 Record/Workout elements, 100,000 selected rows, 8 GiB of
uncompressed `export.xml`,
128 MiB of normalized JSONL, and 8 GiB of snapshotted source files. Use the
inclusive date window to split an export that exceeds the selected-row or
normalized-byte limit. Date windows use the local calendar date written in
`startDate`. WorkoutStatistics are flattened into the persisted row after the
pre-enrichment identity/value hash has been frozen, preserving the shipping
compatibility contract.

Oura polls the supported v2 endpoint roster with bounded date chunks and
pagination. A run accepts at most 16 MiB per response, 128 MiB across all
responses, 5,000 pages, and 100,000 source items. Daily documents keep Oura's attributed day. Instant series such as
heartrate convert their UTC timestamps into the journal timezone. A fresh
cursor begins with a 30-day window, unfinished history walks back to
2015-01-01, and completed endpoints retain a trailing revision window.
Temporary permission loss is reported per endpoint without erasing prior
backfill completion; membership loss and unknown 403 responses fail closed.
One 401 refresh is allowed on save and the rotated token is persisted before
the request is retried. Catalog mode refuses an expired token rather than
consume a rotating refresh grant it cannot persist. Tokens and refresh
rotation live only in journal config and the cursor advances only after
successful publication.

Oura authorization is owner-present: loopback binds only `127.0.0.1:8765`,
uses a random state and PKCE S256, accepts one bounded callback request, and
persists tokens only after state/code and token-response validation.

## identity and dedupe

Source family is part of every identity, so Apple and Oura records never
collide at import. Stable source record IDs use the source-ID identity branch;
records without one use record type, time, source name, value, unit, and
canonical metadata. Replayed observations preserve first import and update
last-seen/value/reference fields in ledger order.

The body app may reconcile mirror overlap at presentation time. Import must
not collapse records across source families.

## privacy and approval gates

Before any save:

- show the target journal and require per-run confirmation;
- validate every replication-destination decision;
- apply exactly one raw-retention decision;
- never send body data, tokens, exports, or fixtures to a remote model;
- use only synthetic fixtures in the repository.

Apple uses `imports/_approvals/health_import_preflight.json` with
`solstone.health_import_preflight.checklist.v3`. Its retention choices are
`discard`, `retain_parsed`, and `retain_complete`; complete retention requires
`unparsed_sensitive_modalities_acknowledged: true`.

Oura uses `imports/_approvals/oura_sync_preflight.json` with
`solstone.oura_sync_preflight.checklist.v2`. Its choices are `discard` and
`retain_parsed`; `retain_complete` is source-incompatible. Scheduled sync also
requires unexpired, timezone-aware standing consent. The save gate runs before
lock, network, token refresh, cursor, bundle, or database mutation.

## deferred work

- Oura webhooks.
- A general owner-facing Oura file-import save path; the retained Python file
  reader remains preview-only.
- Health Auto Export or a custom HealthKit ingest service.
- Any LAN, public, or phone-to-Mac body ingest service.
- Medical advice, recommendations, or anomaly interpretation.

## verification

Focused gates:

```text
cargo test -p solstone-core-body-ingest
cargo test --manifest-path core/Cargo.toml -p solstone-core --test core_journal_contracts
cargo clippy -p solstone-core-body-ingest --all-targets -- -D warnings
cargo clippy -p solstone-core --all-targets -- -D warnings
```

No new Python test may be used to prove a writer or process boundary in this
lane. `rust_body_readers_match_the_frozen_synthetic_corpora` in
`core/crates/solstone-core/tests/body_reader_corpus.rs` compares every
synthetic Apple and Oura normalized row, identity, dedupe key, and value hash
against the frozen native fixture.
