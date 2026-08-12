# W9 Import Sync and Connect Design

## Purpose and boundary

W9 replaces the six remaining library seams that own the `journal importer`
sync/connect behavior in `solstone-core-import`:

- `sync_state`: typed, private, atomic state at `imports/<backend>.json`.
- `sync_plaud`, `sync_obsidian`, and `sync_audio`: catalog the reference's
  three ordered file backends and invoke a caller-owned per-item action only
  for an explicit save.
- `connect`: dispatch the owner-present Oura OAuth flow.
- `consent_gate`: adapt Oura's native fail-closed gate and expose valid
  scheduled-sync guidance without scheduling anything.

It also adds one narrow, read-only public accessor to
`solstone-core-body-ingest`: the body crate already owns the Oura approval
artifact and is the only appropriate place to interpret its `scheduled_sync`
object. That accessor is additive and does not alter `oura_approval` or the
native sync behavior.

This wave does not parse argv, print, choose process exits, read environment
variables, install a crontab entry, write schedule configuration, implement
the per-item download/import/segment/entity action, or move the native Oura
OAuth/sync owner. A later `cli_argv` wave supplies request fields and renders
the returned data/errors. `cli_argv` deliberately remains a stub: it parses
`--sync`, `--path`, `--window-days`, and `--connect` at
`core/crates/solstone-core-import/src/cli_argv.rs:176-205`, then returns its
`Unimplemented` failure at `:68-79`.

The reference sync registry is ordered `plaud`, `obsidian`, `audio`
(`solstone/think/importers/sync.py:29-33`); W9 adds native Oura after those
three only for the public backend listing. Oura is not put into the file-sync
state registry: it retains its separately owned native cursor at
`imports/oura.json` (`core/crates/solstone-core-body-ingest/src/oura_sync.rs:930-949`).

## Acceptance criteria (verbatim)

1. `--backends` (or equivalent) returns exactly plaud, obsidian, audio, oura matching oracle sync.registry_ordered plus oura.
2. Sync without save performs zero downloads/imports and zero calls to any per-item action seam; with save, the seam is invoked for each available item. Must fail against an implementation that ever calls the download/import path when save wasn't requested.
3. `--path`/source_path overrides the source directory for obsidian/audio; `--window-days` reaches only the oura branch and is never forwarded into sync_plaud/sync_obsidian/sync_audio.
4. A save attempt against oura without confirmation is refused with a distinct exit code and owner-facing message naming the remedy. A scheduled run cannot satisfy the gate by also passing the interactive confirm flag — it must have standing scheduled_sync consent; test that this crate's routing doesn't loosen approval.rs's enforcement.
5. Missing, unreadable, or schema-invalid consent state refuses (fail closed) — exercise this crate's own routing path, not just approval.rs's existing unit tests.
6. When standing scheduled-sync consent exists, guidance text surfaces to the caller as data/text only — assert nothing is written to any crontab/schedule-config path and no new file is created as a side effect of computing the guidance.
7. sync_state.rs writes atomically at file mode 0o600 (dir 0o700); a real per-backend fixture file in the exact §3.4 shapes round-trips through the Rust reader with every field preserved.
8. `--connect oura` calls the native connect_oura path and produces its scopes/success shape; `--connect` for any other name is refused before reaching any file-import code path (assert no imports/ directory or manifest is created as a side effect).
9. A backend whose per-item action fails partway leaves already-succeeded items marked imported in sync state and reports the failed items by name/reason — never reports the run as fully complete when errors occurred.
10. No credential/token is written or logged by this crate's code outside paths the reference already uses; grep the diff for any new outbound path for body data outside the existing native body crates.
11. make ci run and result reported honestly.

## Current facts and deliberate decisions

1. **Keep cataloging and saving separate.** The three file backends have
   `dry_run=True` defaults in the trusted W9/W10 oracle, and the reference only
   crosses into `import_one`, `write_markdown_segments`, or `seed_entities` in
   save branches (`plaud.py:441-578`, `obsidian.py:660-731`,
   `audio.py:352-410`). Those operations do not have native owners yet. W9
   ports catalog/state decisions and carries an injected
   `SyncActionSeams<A> { per_item_action: A }`, where the action receives a
   typed `SyncActionRequest` and returns a typed item result. The dispatcher
   invokes it only for `save=true`, once per currently available item, writes
   state after every success, and retains failures in the report. This mirrors
   the named injected fields of `ResolutionSeams<A,C,D,M,L,T>` at
   `detect.rs:103-116`, rather than inventing ambient per-backend behavior.

2. **Plaud catalog HTTP is real production HTTPS, while tests inject it.**
   `sync_plaud` defines a crate-public `PlaudHttp` trait with one catalog
   operation: list the authenticated remote file index. `LivePlaudHttp` owns a
   `ureq::Agent`, uses HTTPS to the existing Plaud list endpoint and passes the
   supplied token only in the Authorization header; `sync_plaud_with_http`
   accepts `&mut dyn PlaudHttp`. This is the same live-implementation plus
   internal-trait pattern as `OuraHttp`/`LiveOuraHttp` at
   `oura_sync.rs:170-264`. Tests use a local double and never make network
   calls. The action seam starts before temp-URL retrieval/download/import, so
   W9 does not falsely claim a native implementation of the later save path.
   `ureq` is added with `rustls`, matching body-ingest's approved use at
   `solstone-core-body-ingest/Cargo.toml:28`.

3. **Parse M4A duration directly, without FFmpeg or a subprocess.** The
   reference classifies unreadable and shorter-than-30-second audio before an
   item becomes available (`audio.py:217-324`). W9 must not use a subprocess in
   library code, and it must not add `solstone-core-observe-audio`: that crate
   compiles FFmpeg from source through `ffmpeg-next`/`ffmpeg-sys-next`, is
   excluded from the iOS check at `Makefile:421`, while
   `solstone-core-import` is currently iOS-eligible. Making the import crate
   depend on it would be a whole-crate iOS regression.

   `sync_audio` instead owns a bounded, dependency-free MP4/M4A box reader. It
   verifies a top-level `ftyp`, finds `moov`, descends to `mvhd`, checks every
   box boundary (including 64-bit extended-size boxes), and reads the
   version-dependent `timescale` and duration fields to compute seconds. A
   zero timescale, missing required box, truncated box, overflow, or corrupt
   layout is `unreadable`; no extension-only success path exists. This is a
   small catalog-only parser, not a media decoder: it reads only sufficient
   container metadata and never allocates f32 sample buffers. The two real
   fixture files, `tests/fixtures/audio/aac_multi_track.m4a` and
   `tests/fixtures/audio/aac_single_track.m4a`, prove this against actual M4A
   bytes. Test-only code may invoke the system `ffprobe` as an external oracle
   to compare its duration to the parser; `ffprobe` and `ffmpeg` are available
   in the current checkout environment at `/usr/bin/ffprobe` and
   `/usr/bin/ffmpeg`. The test skips with a clear message if that optional
   tool is absent in another sandbox.

4. **The body crate owns scheduled-consent interpretation.** Choose the
   cross-crate accessor, not a second parser in `consent_gate`. Specifically,
   refactor only `solstone-core-body-ingest/src/approval.rs` to share its Oura
   bounded-read/schema/root-binding/retention validation between the unchanged
   `oura_approval` (`approval.rs:99-173`) and a new public read-only
   `oura_scheduled_sync_guidance`; re-export it from `body-ingest/src/lib.rs`.
   The accessor returns guidance only after the same `scheduled_sync.approved`,
   nonblank cadence, RFC3339 `valid_until`, and non-expiry checks now enforced
   at `approval.rs:150-171`. It neither calls clock/scheduler writers nor
   changes the gate. This avoids duplicate parsers for the same private,
   bounded artifact and preserves the single native owner of body consent.

5. **Map native body failures into the import crate's error surface.**
   `ImportError` is currently the crate-wide library error at
   `core/crates/solstone-core-import/src/lib.rs:68-161`; it already owns
   staging/metadata/publish failures. `ResolutionError` deliberately remains a
   separate generic resolver result because it carries injected detector causes
   and has no CLI route (`detect.rs:160-210`). W9 public dispatch/connect/gate
   functions therefore return `Result<_, ImportError>`, not
   `BodyIngestError`. Add a structured `ImportError::Refusal` variant holding
   a stable refusal kind, an exit code, and an owner-facing message, alongside
   state/catalog/action variants with path/backend context. The adapter maps
   `BodyIngestErrorKind::Gate` to exit 2 and the same owner message policy as
   `main.rs:4470-4476` and `:4543-4549`; Source/Normalize map to
   `EXIT_DATAERR`, Publication/Rebuild to `EXIT_IOERR`. The per-run Oura
   refusal specifically names `--confirm-body-save`, while scheduled requests
   name valid standing `scheduled_sync` consent. A later renderer receives one
   import error enum and does not need a body-ingest dependency.

6. **Centralize backend dispatch in `dispatch_sync`.** `sync_state` exposes
   `dispatch_sync(request, seams)` as this wave's library entry point. It
   branches exactly once: `oura` calls the native `sync_oura`; `plaud`,
   `obsidian`, and `audio` call their ported catalogs. `SyncRequest` is typed so
   `window_days` is read only in the Oura arm. The three backend option structs
   intentionally omit it, reflecting the oracle signatures and preventing
   accidental forwarding. `available_sync_backends()` returns the four-item
   listing in its fixed order. This helper is immediately testable and becomes
   the sole later `cli_argv` call site.

7. **Sync state is exact, private, and independently typed.**
   `sync_state` owns the three file-backend envelopes and does not read or
   write Oura's native cursor. `write_json` defaults to two-space indentation
   (`journal-io/src/atomic.rs:38-45`) and appends one newline
   (`:187-202`), matching `json.dumps(state, indent=2)` formatting. W9 calls
   it with file mode `0o600`, after repairing/creating `imports/` at `0o700`;
   `write_json` publishes atomically (`atomic.rs:48-90`). State preserves the
   actual reference fields:

   - Plaud envelope: `backend`, `files`, `last_sync`; file entries carry
     `filename`, `fullname`, `filesize`, `start_time`, `duration`, `is_trash`,
     `status`, and the applicable `skip_reason`, `import_timestamp`,
     `matched_at`, or `imported_at` fields (`plaud.py:362-422`, `:558-578`).
   - Obsidian envelope: `backend`, `source_path`, `files`, `last_sync`; entries
     carry `filename`, `title`, `mtime`, `content_hash`, `status`, `edit_count`,
     and the applicable `imported_at`/`segments` fields
     (`obsidian.py:569-642`, `:702-731`).
   - Audio envelope: `backend`, `source_path`, `files`, `last_sync`; entries
     carry `filename`, `filesize`, `hash`, `status`, and the applicable
     `duration`, `skip_reason`, `imported_at`, or `last_error` fields
     (`audio.py:228-321`, `:346-410`).

   The reader rejects malformed/envelope-backend-mismatched records rather
   than treating corrupt state as empty. The catalog writes state in both
   preview and save modes because the reference catalog records availability in
   either mode; no per-item action runs in preview.

8. **Connect is Oura-only and native; no new body-ingest seam is needed.**
   `connect_oura` is the real owner-present OAuth entry point
   (`oura_connect.rs:129-175`), and its `OuraConnectReport` exposes scopes
   (`:52-60`). Its internal `ConnectPlatform`/`connect_with_platform` test seam
   is deliberately not public; body-ingest already tests the full successful
   PKCE/token-persistence flow with `FakePlatform` at
   `oura_connect.rs:525-603`. W9 calls the sole public function directly,
   maps its scopes to an import-owned `ConnectOutcome`, and refuses every other
   name before a file catalog/state function is entered.

   Import tests split the evidence. A pure, private mapping helper is tested
   with synthetic scopes, because `OuraConnectReport` has private fields and
   no public constructor. Separately, the public routing test uses a
   canonical temporary journal with no `config/journal.json`: after
   `connect_oura` pins the journal, `read_client` deterministically returns the
   native `Source` failure `journal_config_missing` at
   `oura_connect.rs:142-147` and `:178-192`, before it binds the callback port,
   opens a browser, or makes a network request. `connect_backend` must surface
   that mapped native failure, not an unknown-backend refusal or a swallowed
   success. Token persistence remains inside body-ingest's journal-config CAS
   (`oura_connect.rs:240-259`); W9 never serializes or logs credentials.

9. **No cron hint is invented.** `OuraSyncReport` has no cadence or cron-hint
   field (`oura_sync.rs:75-118`). The Python generic renderer's `cron_hint`
   branch is after non-Oura dispatch (`cli.py:384-391`), whereas Oura returns
   before it (`cli.py:222-250`). W9 returns optional typed
   `ScheduledSyncGuidance` data from the read-only owner accessor; a later
   renderer may present it, but no code in this wave installs or writes a
   schedule.

## Public library surface

All new source files and modified Rust sources retain the repository SPDX
header. Types are re-exported from `solstone-core-import/src/lib.rs` only when
they are part of the later CLI boundary.

| Module | Public surface |
|---|---|
| `sync_state.rs` | `enum SyncBackend { Plaud, Obsidian, Audio, Oura }`; `struct SyncRequest<'a> { journal: &'a Path, backend: &'a str, save: bool, source_path: Option<&'a Path>, window_days: Option<u64>, confirm_body_save: bool, scheduled: bool, force: bool, auto: AutoTimestamp, plaud_access_token: Option<&'a str> }`; `struct SyncActionSeams<A> { per_item_action: A }`; `struct SyncActionRequest<'a>` identifying backend, stable item key/name, and source metadata; `struct SyncReport` with totals, available/imported/skipped/downloaded, per-item failures, optional Oura report, and optional scheduled guidance; `available_sync_backends() -> &'static [SyncBackend]`; `load_sync_state(journal: &Path, backend: FileSyncBackend) -> Result<Option<FileSyncState>, ImportError>`; `write_sync_state(journal: &Path, state: &FileSyncState) -> Result<(), ImportError>`; `dispatch_sync<A>(request: &SyncRequest<'_>, seams: &mut SyncActionSeams<A>) -> Result<SyncReport, ImportError>` where `A: FnMut(SyncActionRequest<'_>) -> Result<(), SyncActionFailure>`. |
| `sync_plaud.rs` | `struct PlaudSyncOptions<'a> { journal: &'a Path, save: bool, access_token: &'a str }`; typed `PlaudSyncState` / `PlaudFileState`; `trait PlaudHttp { fn list_files(&mut self, access_token: &str) -> Result<Vec<PlaudRemoteFile>, ImportError>; }`; `struct LivePlaudHttp`; `sync_plaud_with_http<A>(options: &PlaudSyncOptions<'_>, http: &mut dyn PlaudHttp, seams: &mut SyncActionSeams<A>) -> Result<SyncReport, ImportError>`. `LivePlaudHttp` is used only by the production dispatcher; tests pass a double. |
| `sync_obsidian.rs` | `struct ObsidianSyncOptions<'a> { journal: &'a Path, save: bool, source_path: Option<&'a Path>, force: bool }`; typed `ObsidianSyncState` / `ObsidianFileState`; `sync_obsidian<A>(options: &ObsidianSyncOptions<'_>, seams: &mut SyncActionSeams<A>) -> Result<SyncReport, ImportError>`. Its source selection observes explicit `--path`, then valid stored state, then the reference default vault locations; all save work remains the seam. |
| `sync_audio.rs` | `struct AudioSyncOptions<'a> { journal: &'a Path, save: bool, source_path: Option<&'a Path>, force: bool, auto: AutoTimestamp }`; typed `AudioSyncState` / `AudioFileState`; `sync_audio<A>(options: &AudioSyncOptions<'_>, seams: &mut SyncActionSeams<A>) -> Result<SyncReport, ImportError>`. It walks supported audio files, hashes the source, uses its private bounded MP4/M4A `moov`/`mvhd` duration parser for readable/duration status, and passes only available items to the seam in save mode. |
| `consent_gate.rs` | `struct ScheduledSyncGuidance { cadence: String, valid_until: String }`; `oura_save_refusal(error: BodyIngestError) -> ImportError`; `read_oura_scheduled_sync_guidance(journal: &Path) -> Result<Option<ScheduledSyncGuidance>, ImportError>`. The public dispatch path does not duplicate the artifact parser. |
| `connect.rs` | `struct ConnectOutcome { backend: SyncBackend, scopes: Vec<String> }`; `connect_backend(journal: &Path, backend: &str) -> Result<ConnectOutcome, ImportError>`. Only `oura` calls native `connect_oura`; all other strings return `ImportError::Refusal`. A private scope-to-outcome mapper is unit-tested in this module; it is not a new body-ingest seam or public API. |
| `body-ingest approval.rs` (cross-crate addition) | `struct OuraScheduledSyncGuidance { cadence: String, valid_until: String }`; `oura_scheduled_sync_guidance(journal: &Path) -> Result<Option<OuraScheduledSyncGuidance>, BodyIngestError>`. It is read-only, validates through the same extracted Oura-approval parser, and is re-exported at `body-ingest/src/lib.rs`. |
| `lib.rs` | Remove the six completed rows from `MODULE_STUBS`; add the W9 types/re-exports and structured `ImportError` variants for state/catalog/action/refusal errors. Keep `audio`, `text`, `cli_argv`, `cli_journal_source`, and `cli_render` as the remaining five stub rows. |

`SyncActionRequest` uses backend-specific typed metadata behind an enum, so the
later Plaud action can request a temporary URL/download while Obsidian and
Audio actions receive local source paths. Neither token nor raw authorization
header is included in a report, action request, state file, or error.

## Resolution and data flow

1. A later argv layer creates `SyncRequest`; this wave never reads argv or
   environment. It supplies a Plaud token explicitly when selecting Plaud.
2. `available_sync_backends` returns the literal fixed listing. Otherwise,
   `dispatch_sync` validates the backend and selects the sole matching arm.
3. `oura` builds native `OuraSyncOptions` from only `save`, confirmation,
   scheduled, and `window_days`, invokes `sync_oura`, maps its report/errors,
   and reads scheduled guidance only after a valid scheduled consent path.
   `sync_oura` itself checks approval before lock/transport and again under the
   lock (`oura_sync.rs:313-330`), so the import route cannot weaken it.
4. Each file backend loads its typed state, performs only its catalog scan,
   computes available/imported/skipped/unreadable statuses, and atomically
   persists that state. `source_path` is visible only to Obsidian and Audio.
5. If `save=false`, it returns without calling `per_item_action`. If true, it
   calls the action for each available item. On success it records the item as
   imported and immediately writes private state; on failure it leaves the
   item available, records a name/reason in the report/state as applicable,
   continues to remaining items, and returns a non-complete report.
6. `connect_backend` accepts only `oura`, calls native OAuth, and returns its
   scopes. It does not construct `imports/`, a manifest, or a file-backend
   request.

## Files and dependency changes

| Area | Change |
|---|---|
| `core/crates/solstone-core-import/src/{sync_state,sync_plaud,sync_obsidian,sync_audio,connect,consent_gate}.rs` | Replace all six `reserved_seam` bodies with the surfaces above; no compatibility shims. |
| `core/crates/solstone-core-import/src/lib.rs` | Re-export W9 API, add error variants/display/exit-code helper, and reduce `MODULE_STUBS` from 11 to 5. |
| `core/crates/solstone-core-import/Cargo.toml` | Add `solstone-core-body-ingest` and `ureq` with `rustls`; retain existing journal-I/O and `tempfile`. No audio-decoder or FFmpeg dependency is added. |
| `core/crates/solstone-core-body-ingest/src/{approval,lib}.rs` | Add and export only the read-only scheduled-guidance accessor; share parsing internally without changing `oura_approval`. |
| `core/crates/solstone-core-import/tests/stub_table.rs` | Change the count to 5 and add all six W9 modules to the implemented list. |
| `core/crates/solstone-core-import/tests/{sync_state,sync_dispatch,sync_plaud,sync_obsidian,sync_audio,consent_gate,connect,sync_security}.rs` | Add focused unit/integration coverage described below. |
| `core/fixtures/import_sync_state_oracle.json` | Add three exact, full-field state envelopes derived from the live Plaud/Obsidian/Audio state shapes above. Tests consume this fixture through `include_str!`; no state shape is transcribed into test code. |
| `docs/design/import-sync-connect-w9.md` | This design and the subsequent implementation findings. |

## Fixture and test plan

The W9/W10 oracle remains the order/signature authority. The new state fixture
is a literal JSON fixture containing one full Plaud, Obsidian, and Audio
envelope, including every optional per-item field actually written by the
reference. It intentionally uses data values that exercise round-trip rather
than asserting a clock-dependent catalog snapshot.

| AC | Test location and test name |
|---|---|
| 1 | `tests/sync_dispatch.rs::backends_are_oracle_order_plus_oura` compares the public listing literally with the oracle ordering plus trailing Oura. |
| 2 | `tests/sync_dispatch.rs::catalog_never_calls_action_and_save_calls_each_available_action` uses a counting action seam for all three backends. |
| 3 | `tests/sync_dispatch.rs::path_and_window_are_routed_only_to_their_supported_backends` supplies distinct Obsidian/Audio trees and inspects the pure typed backend-option construction used by `dispatch_sync`; it proves no file-backend option can receive `window_days`, without invoking live Oura sync. |
| 4 | `tests/consent_gate.rs::oura_save_refusal_preserves_gate_exit_and_scheduled_consent_requirement` routes both confirmation combinations into native Oura validation and asserts exit 2/remedy text. |
| 5 | `tests/consent_gate.rs::dispatch_fails_closed_for_missing_unreadable_and_invalid_oura_approval` constructs each artifact condition and calls `dispatch_sync`, not the body crate's private unit helper. |
| 6 | `tests/consent_gate.rs::scheduled_guidance_is_read_only_data` snapshots the journal tree, calls the importer guidance path, asserts cadence/expiry data, and proves no crontab, schedule-config, or new file exists. |
| 7 | `tests/sync_state.rs::oracle_envelopes_round_trip_atomically_and_owner_private` loads all fixture envelopes, writes/reads them, checks exact JSON values, file mode `0600`, directory mode `0700`, and no temporary residue. |
| 8 | `tests/connect.rs::synthetic_scopes_map_to_connect_outcome` tests the private report-scope mapping with synthetic values; `tests/connect.rs::oura_route_reaches_native_config_validation_before_oauth` calls public `connect_backend` against a canonical journal without `config/journal.json` and asserts the mapped native `journal_config_missing` source failure before any browser/network path. `tests/connect.rs::non_oura_refusal_has_no_file_side_effects` snapshots the journal for the fully local non-Oura refusal. |
| 9 | `tests/sync_dispatch.rs::partial_action_failure_persists_prior_successes_and_reports_each_failure` makes the second action fail; it reloads state and asserts success, availability, and named error reporting. |
| 10 | `tests/sync_security.rs::supplied_credential_never_enters_state_report_error_or_journal_tree` drives a Plaud double with a sentinel token; an implementation-stage `git diff --check` plus targeted diff grep audits that no new body-data transport or credential write escaped the existing body crates. This is both dynamic evidence and the required source audit. |
| 11 | No unit test can prove the repository gate. The implementation-stage validation record runs `hop check --allow-capture -- make ci` once and reports its actual result; it is not represented as a passing test in advance. |

Additional focused tests cover Plaud list request parsing/non-200 failures with
the `PlaudHttp` double; Obsidian hidden-directory, content-hash, force, and
stored-source-path behavior; Audio readable/unreadable/short classifications
using the two repository M4A fixtures and a malformed file, with test-only
`ffprobe` duration comparison when installed; and body-ingest's new accessor
equivalence with the unchanged scheduled branch.

## What a wrong port would have done

- It would have treated the reference's preview default as merely a rendering
  choice and called a download/import action while `save=false`.
- It would have attached `window_days` to a generic backend option bag and
  forwarded it into Plaud, Obsidian, or Audio despite none declaring it in the
  oracle.
- It would have allowed `confirm_body_save=true` to bypass a scheduled
  request's missing/expired standing consent, rather than forwarding both
  booleans to the native gate unchanged.
- It would have re-parsed `imports/_approvals/oura_sync_preflight.json` in
  `solstone-core-import`, causing its cadence/expiry interpretation to drift
  from the body owner.
- It would have used `fs::write` or default permissions for sync state, losing
  atomic publication and the required `0600`/`0700` privacy contract.
- It would have put tokens in `SyncReport`, action metadata, or `imports/*.json`
  while adding a supposedly native Plaud client.
- It would have marked all available items imported only after the whole save
  pass, thereby losing a successful prefix when a later action failed and
  reporting a false full success.
- It would have made Plaud HTTP a test-only stub, or made tests call live
  Plaud with unavailable credentials, instead of using the production
  `LivePlaudHttp` plus injected double.
- It would have pulled the FFmpeg-building observe-audio crate into this
  iOS-eligible crate merely to count samples, or copied Python's `ffprobe`
  subprocess into library code, instead of reading bounded M4A container
  metadata.
- It would have treated an `.m4a` suffix as sufficient, accepting missing,
  truncated, or malformed `ftyp`/`moov`/`mvhd` boxes as readable audio.

## File sequence

1. Add the narrow body-ingest scheduled-guidance accessor and its direct
   equivalence/read-only tests; leave `oura_approval` behavior unchanged.
2. Add import-crate dependencies, W9 error/refusal shape, backend/request/
   report/action types, and reduce the stub inventory without touching argv.
3. Implement typed atomic sync state and vendor the literal state fixture with
   its mode/round-trip test.
4. Implement individual catalogs: Plaud's live/double HTTP boundary,
   Obsidian source/catalog scan, and Audio's bounded MP4/M4A box-parser-based
   catalog.
5. Implement `dispatch_sync`, save action sequencing/partial-failure state,
   native Oura adaptation/guidance, and Oura-only connect dispatch.
6. Add routing, fail-closed, no-side-effect, credential-boundary, and
   real-audio-fixture tests; update the two stub-table assertions.
7. During implementation validation only, run the narrow affected tests and
   the stipulated `make ci` through `hop check --allow-capture`; report actual
   results and the AC 10 diff audit honestly.

## Risks and open implementation checks

- The Python sync-state loader swallows corrupt JSON as no state
  (`sync.py:36-47`), while W9 deliberately refuses malformed/native-typed
  state to preserve durable evidence. The implementation must record this as a
  deliberate native fail-closed divergence and ensure callers get a remedy.
- The Python implementation currently writes catalog state even in preview.
  AC 2's “zero downloads/imports” must not be misread as “zero state writes.”
- The scheduled cadence is only required to be nonblank by the current gate;
  W9 exposes it as uninstalled guidance and must not validate, normalize, or
  execute it as a cron expression.
- `connect_oura` presently needs a live owner-present browser/callback flow,
  but W9 does not test that success path from the import crate. Body-ingest's
  existing internal `FakePlatform` test owns it. The import crate's
  deterministic missing-config route proves public wiring without opening a
  listener/browser or requesting a new cross-crate seam.
