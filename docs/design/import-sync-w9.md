# W9 Import Sync Design

## Purpose and boundary

W9 fills the six reserved seams in `solstone-core-import` for the four
`journal importer` modes that reach outside the local import resolver:

- sync-state read/write (`sync_state`);
- Plaud catalogue, download, and import orchestration (`sync_plaud`);
- Obsidian vault sync (`sync_obsidian`);
- local audio-folder sync (`sync_audio`);
- owner-present Oura connect routing (`connect`); and
- Oura save-mode consent routing (`consent_gate`).

It is a **library-only** port. It owns typed mode semantics, backend inventory,
state transitions, outcome values, and owner-facing gate values. It does not
parse argv, dispatch a parsed command, read environment variables, resolve a
home directory, print, exit, launch a process, or install a schedule. The
dispatcher and command cut are W10. `cli_argv::ParsedCommand` remains unchanged.

W9 does not touch Python; it does not add body-ingress outbound behavior; it
does not persist a credential or token; it does not write a schedule file; and
it does not create a second consent implementation. The native body owner
continues to own Oura authorization, API ingress, and approval validation.

The governing layering rule is: “Subsystem libraries added in later waves take
config and paths as parameters, own no process-global state, and do not parse
argv.” (`docs/PORTING.md`, Layering). The direct body-ingest dependency selected
below complies with that rule: both sides remain subsystem libraries, and the
dependency crosses a typed in-process API rather than an argv or process
boundary.

The recorded baseline before this wave is green: 78 tests plus 5 doctests, with
no failures. Python cannot run in this checkout: neither `.venv/bin/python` nor
a system `python` exists. Consequently AC7 cannot validate a real
reference-written `imports/<backend>.json`; it uses a checked-in fixture derived
from the preparation schema tables, expressed as an inline test literal, and
states this substitution explicitly.

## Acceptance criteria (verbatim)

1. `[test]` `--backends` lists the three Python-side backends plus the native one, matching the grammar fixture.
2. `[test]` `--sync <backend>` without `--save` performs no download and no import; with `--save` it acts. Must fail against a download-by-default implementation.
3. `[test]` `--path` overrides the source directory; `--window-days` reaches only the backend whose signature accepts it, and does not leak to the others.
4. `[test]` The consent gate refuses a sensitive save without confirmation, with its own exit code and an owner-facing message naming the remedy; and `[test]` a scheduled run cannot satisfy it with the interactive flag — it requires the standing consent.
5. `[test]` An unknown, missing or unreadable consent state refuses. Fail closed.
6. `[test]` A backend returning scheduling guidance surfaces it as text; nothing is installed and no schedule file is written.
7. `[test]` Sync state is written atomically with the private mode, and a state file written by the reference is read back with every field preserved.
8. `[test]` `--connect` runs the owner-present authorization flow for the native backend and does not reach the retired file-import route.
9. `[test]` A backend that fails mid-sync leaves the already-imported items intact and reports what did not arrive — never a partial state reported as complete.
10. `[check]` No credential or token is written outside the paths the reference uses, and none is logged. `[check]` Nothing in this wave gives body data an outbound path.
11. `[check]` `make ci` reported honestly.

## Deliberate decisions

1. **Keep `cli_argv::ParsedCommand` unchanged.** W9 has no dispatcher cut and
   no argv authority. It will provide typed library requests and typed rendered
   values for W10 to map from `ParsedCommand`. `SyncBackendRequest` is the one
   typed non-argv request surface; a parallel `SyncOptions` enum and identity
   mapping would create two field sources for no W9 consumer. Changing parser
   shape now would claim dispatcher reachability that this wave explicitly does
   not ship.

   `SyncBackendRequest` has `Plaud`, `Obsidian`, `Audio`, and `Oura` variants. Its
   `Obsidian` and `Audio` variants alone carry an optional source-path override;
   its `Oura` variant alone carries `window_days`. The returned
   `PlaudSyncRequest`, `ObsidianSyncRequest`, and `AudioSyncRequest` have no
   `window_days` member, making a leak to a Python-side backend
   unrepresentable. W10 later supplies only argv-to-request parsing.

2. **Use named seam structs, never positional closures or ambient reads.**
   Each backend receives a named `*Seams` structure, following W1b's
   `ResolutionSeams` convention. Every field has a one-line invocation contract.
   This preserves the Python monkeypatch boundary as an explicit Rust API and
   prevents a caller from silently exchanging clock, scanner, or pipeline
   authority. The required seams and their conformance proof are:

   | Seam | Contract | Conformance test |
   |---|---|---|
   | `PlaudCredential` | caller supplies an optional token; it is never serialized, rendered, or logged | fake credential proves missing-token refusal and a recording logger/state writer proves the supplied token is absent |
   | `PlaudCatalogue` / `PlaudDownload` | catalogue is preview-safe; temporary-URL and streaming-download operations exist only in save seams | scripted fakes prove no live transport is needed and preview has no download field |
   | `PlaudManifestLookup` | caller matches remote metadata to existing imports before catalogue state is assigned and returns a closed failure kind | fake manifest lookup proves matched recordings are not downloaded and matching failures cannot render caller text |
   | `SyncClock` | supplies sync `last_sync`, match, and import-completion timestamps | fixed clock asserts state times without ambient time |
   | `ObsidianHomeCandidates` | caller provides the ordered fallback candidates; the library never reads home/environment | candidate fake selects the first existing directory and proves no candidate means named no-vault refusal |
   | `DirectoryScanner` | supplies Obsidian markdown and audio candidate enumeration | fake scanner asserts filtering and proves no direct recursive scan is required by orchestration |
   | `AudioProbe` | returns a duration or unreadable answer for an audio path; its free error text is reduced to the reference's generic unreadable outcome | fake probe drives `unreadable`, short-skip, and available outcomes |
   | `ImportPipeline` | caller performs one approved Plaud/audio import and returns a typed result; Plaud reduces failures to a closed kind, while audio retains the pipeline's `str(exc)`-equivalent as `last_error` | fake pipeline drives success, skip, no-result, and failure without importing data |

   Live Plaud catalogue and download adapters are not constructed by W9. W10's
   process adapter constructs them after it has obtained the credential and
   chooses the live operation; W9 receives only the narrow trait references.
   This follows the same injected-transport split as
   `OuraHttp`/`LiveOuraHttp`/`sync_with_http`, while avoiding a library-owned
   environment read or process decision.

3. **Route consent directly to `solstone-core-body-ingest`; do not fork it.**
   Add `solstone-core-body-ingest` as a direct dependency of
   `solstone-core-import`, widen the existing `oura_approval(journal,
   confirmed, scheduled) -> Result<BodyRawRetention, BodyIngestError>` from
   crate-visible to public, and re-export it from body-ingest `lib.rs`.
   `consent_gate.rs` maps the existing error's public `kind()` and `stage()`;
   `stage` is already the gate reason vocabulary. It does not restate approval
   checks or reason strings and introduces no new body-owned value type.

   Body-ingest has no import dependency, so this adds no Cargo cycle. The cost
   is a new sibling-library direction and a two-line public API widening; the
   benefit is that AC4 and AC5 exercise the real approval refusal rather than a
   fake gate seam. An injected-only gate would test orchestration but could not
   prove agreement with the actual native vocabulary, so it is rejected.

   The gate deliberately keeps its time inside the body owner:
   `oura_approval` continues to call `chrono::Utc::now()` for scheduled-consent
   expiry. Past and far-future fixture artifacts fully test that branch, so a
   clock parameter would widen an out-of-scope API without improving
   conformance. `SyncClock` is for sync state and Plaud progress only.

4. **Encode preview and save as separate request types and seams.** `SyncPreviewRequest`
   and `SyncSaveRequest` are distinct zero-sized mode markers, mirroring
   `PreviewRequest` and `SaveRequest`. Backend APIs accept the marker in their
   request generic or mode enum conversion occurs only in constructors:
   `PlaudPreviewSeams` has no download or import-pipeline field, while
   `PlaudSaveSeams` extends it with both. The analogous audio and Obsidian save
   seam types alone carry their pipeline and note-writer authority. No `bool
   save` is accepted by a public or internal operation. The AC2 negative twin
   uses preview seam construction itself: adding a download-by-default call no
   longer compiles.

5. **Use a loss-preserving raw sync-state map, with an explicit compatible writer.**
   `SyncState` deliberately holds an ordered `serde_json::Map` rather than
   typed records plus `flatten`: the raw map preserves unknown root and per-file
   members without claiming arbitrary reference key-order preservation. The
   orchestration code authors only the reference union keys: Plaud metadata,
   match, skip, and import keys; Obsidian source/title/hash/edit/segment keys;
   and audio hash/probe/skip/error keys. Status remains the reference string
   vocabulary in that raw map so unknown future values survive a round-trip.

   Two byte-compatibility hazards are deliberate:

   - Python writes `json.dumps(..., indent=2)` with ASCII escaping and no final
     newline. `serde_json::to_writer_pretty` is not assumed byte-compatible.
     W9 supplies a dedicated state serializer that produces two-space pretty
     JSON, ASCII escapes non-ASCII strings, preserves library-authored insertion
     order, and adds no newline. Tests compare exact bytes only for state the
     library authors.

     A reference-written input instead receives parsed field-for-field equality:
     every known and unknown root and per-file key/value survives a load/save
     round-trip. `#[serde(flatten)]` cannot preserve arbitrary interleaving of
     known and unknown keys, so reference key ordering is a deliberate
     non-byte-compatible divergence. AC7 requires preservation, not identical
     reference round-trip bytes.
   - Untyped `serde_json::Value` does not impose the `i64` overflow boundary and
     cannot read Python's bare non-finite floats. W9 checks only the reference's
     integer-constrained fields at the JSON boundary and rejects overflow;
     fractional Plaud `start_time`/`duration` and ordinary extras remain raw JSON
     values. A non-finite float is a strict sync-state decode failure, hence
     benign recatalogue; it is not silently transformed. This is documented as
     the intentional one-way incompatibility measured by `docs/PORTING.md`.

   The reader distinction is structural: `read_sync_state` returns a
   `SyncStateRead` value for every missing/unreadable/loaded condition, while
   `check_oura_sync_save` returns `ConsentGateOutcome::Blocked` for an invalid
   approval. Consent never reuses the sync-state reader or its error type.

6. **Retire all six W9 stub rows.** `sync_state`, `sync_plaud`,
   `sync_obsidian`, `sync_audio`, `connect`, and `consent_gate` are wholly
   complete at their library boundary after this wave, so each is removed from
   `MODULE_STUBS`. `audio` and `text` remain reserved because their generic
   import-pipeline implementations are not W9's sync orchestration. The result
   is `MODULE_STUBS.len() == 5`: `audio`, `text`, `cli_argv`,
   `cli_journal_source`, and `cli_render`. Update
   `tests/stub_table.rs` line 10 to 5 and its implemented-module list to include
   all six retired module names; its loop continues to require every remaining
   row to return its own `ImportError::Unimplemented`. The live Plaud transport
   is W10 adapter work, but `sync_plaud` is nevertheless wholly complete at its
   library boundary: it owns the typed operation and accepts complete catalogue,
   download, match, state, and pipeline seams; constructing production transport
   is expressly outside that boundary.

   The complete workspace scan finds these `MODULE_STUBS`/stub-table touch
   points. Only the first two files change for W9:

   | File | W9 disposition |
   |---|---|
   | `core/crates/solstone-core-import/src/lib.rs:167-179` | Remove the six completed rows. |
   | `core/crates/solstone-core-import/tests/stub_table.rs:10,26-35` | Change expected count to 5 and add the six completed modules to the implemented list. |
   | `core/crates/solstone-core-import-sources/src/lib.rs:43-56` | No change: this is the independent source-crate table. |
   | `core/crates/solstone-core-import-sources/tests/stub_table.rs:10-24` | No change: it asserts only the source-crate table. |
   | `core/crates/solstone-core-import-sources/tests/source_immutability.rs:9,21-24` | No change: it iterates only source-crate stubs. |

7. **Vendor the whole W9/W10 oracle once as
   `core/fixtures/import_sync_reference_oracle.json`.** Copy the complete source
   fixture byte-faithfully, including `schema`, `provenance`, `sync`, and
   `journal_source`; W9 reads only `.sync`. Splitting it would create a second
   derived artifact, lose the source fixture's identity, and force W10 to
   re-vendor its `journal_source` sibling. The neutral name declares shared
   ownership without falsely claiming that W9 owns the W10 subtree.

8. **Return a named gate value; callers map it to exit/text/JSON.**
   `GateFailure` carries the body-owned reason/stage plus importer, target,
   approval path, and flow. `CONSENT_GATE_EXIT_CODE` is the named caller-mapping
   constant with value **2**, matching the native body binary and the retained
   Python gate. It is presentation policy, not library exit behavior: W9 never
   prints or exits.
   `consent_gate.rs` renders the complete existing
   owner-facing explanation as a pure string formatter: blocked-before-write
   headline, importer/target/reason, no-import-directory assurance, approval
   path, and numbered next steps including the scheduled-consent alternative.
   It also offers the Python-compatible ten-key JSON shape
   (`skipped`, `reason`, `gate_reason`, `importer`, `flow`, `approval_path`,
   `target_journal`, `missing_fields`, `invalid_fields`, `checklist_version`).
   Native `BodyIngestError` supplies no `missing_fields` or `invalid_fields`, so
   W9 includes those two required payload keys as empty arrays rather than
   inventing populated values from a reason string. The body layer supplies the
   actual refusal; W9 supplies presentation data and never prints it.

   The text formatter deliberately paraphrases the reference gate formatter and
   appends a `Flow:` line. It preserves the blocked-before-write headline,
   importer/target/reason, owner approval path, and all four remedies including
   scheduled-sync consent, while keeping the native formatter compact and typed.
   This owner-facing text is therefore not byte-identical reference output.

9. **Make audio progress per-item and observable.** `AudioSyncOutcome` contains
   the final state snapshot, aggregate summary, and ordered `AudioItemOutcome`
   values. Each item records its relative path, terminal state transition,
   whether a checkpoint was requested, and optional error text. An import error,
   skipped pipeline outcome, absent pipeline outcome, or unknown pipeline
   outcome transitions that item back to `available`, writes `last_error`, and
   adds the same item-scoped message to aggregate `errors`. Manifest matches are
   resolved before the attempted queue and remain `imported`. This makes both
   halves of AC9 directly assertable; an error count alone is insufficient.

10. **Keep `cron_hint` out of the design surface.** The only observed consumer
    has no producer and the Oura branch returns before reaching it. W9 does not
    fabricate a result field, a schedule recommendation, or a schedule writer.
    `SyncGuidance` and its pure `format_text` make conditional guidance supplied
    by a backend renderable at the W9 library boundary. It does not make Oura
    produce `cron_hint` or promise scheduling.

11. **Expose separate typed preview and save operations.** The implemented
    `sync_plaud_preview`/`sync_plaud_save`, `sync_obsidian_preview`/
    `sync_obsidian_save`, and `sync_audio_preview`/`sync_audio_save` functions
    replace the design sketch's single generic operation name. This keeps the
   request-mode split visible at each public call site and avoids a runtime
   `save` switch. Consent routing returns only `Allowed` or `Blocked`: the body
   owner consumes raw-retention policy under its own lock, and W9 has no
   consumer for that policy value.

12. **Preserve the body owner's connect error type.** `connect_oura` returns
    `BodyIngestError` directly rather than wrapping it in a crate-local error.
    It is the exact body-owner authorization failure and the route performs no
    import-specific recovery or translation; wrapping it would obscure the
    public `kind()` and `stage()` vocabulary without giving a caller another
    actionable distinction.

13. **Match audio's removal ordering exactly.** The reference first promotes
    available manifest matches, then marks every path absent from the current
    scan `removed`, including an entry that was just promoted. W9 follows that
    ordering. This deliberately replaces the prior native-only exception that
    retained an unseen imported entry, because the reference's final state is
    authoritative.

## Public library surface

All new source files receive the repository SPDX header.

| Module | Public surface |
|---|---|
| `sync_state.rs` | `BackendName::{Plaud, Obsidian, Audio, Oura}` and ordered `SYNC_BACKEND_INVENTORY`, plus `read_sync_state(root, BackendName) -> SyncStateRead` and `write_sync_state(root, &SyncState) -> Result<(), SyncStateWriteError>`; an ordered raw JSON root retains known and unknown state fields alike. `SyncStateRead::{Absent, Unreadable { class }, Loaded(SyncState)}` makes benign recatalogue explicit and is not a `Result`. |
| `sync_plaud.rs` | `sync_plaud_preview(request, PlaudPreviewSeams)` and `sync_plaud_save(request, PlaudSaveSeams)` return `Result<PlaudSyncOutcome, PlaudSyncError>`; preview has credential, catalogue, manifest lookup, clock, and state writer only; save adds temporary-URL/download and pipeline authority. Catalogue, manifest, temporary-URL, download, and pipeline failures use closed `PlaudFailureKind` values in state and presentation; state-publication errors remain their separate local write-error surface. |
| `sync_obsidian.rs` | `sync_obsidian_preview(request, ObsidianPreviewSeams)` and `sync_obsidian_save(request, ObsidianSaveSeams)` return `Result<ObsidianSyncOutcome, ObsidianSyncError>`; request carries source override/force and mode marker; preview seams provide clock, retained-state path candidates, and scanner, while save adds the segment writer. |
| `sync_audio.rs` | `sync_audio_preview(request, AudioPreviewSeams)` and `sync_audio_save(request, AudioSaveSeams)` return `Result<AudioSyncOutcome, AudioSyncError>`; request carries required explicit source path, force, auto, and mode marker; preview seams provide scanner, hash/manifest lookup, fractional-duration probe, clock, and state writer, while save adds pipeline authority. |
| `connect.rs` | `connect_oura(request) -> Result<OuraConnectOutcome, BodyIngestError>` delegates to the already-public native body-owner `connect_oura` operation and returns data only. No token path is introduced. |
| `consent_gate.rs` | `CONSENT_GATE_EXIT_CODE: i32 = 2`; `check_oura_sync_save(request) -> ConsentGateOutcome`; `ConsentGateOutcome::{Allowed, Blocked(GateFailure)}`; pure `GateFailure::format_text` and `GateFailure::to_python_payload`. It calls the re-exported body-owned approval check and uses the body owner's `OURA_PATH` and `OURA_CHECKLIST`; it has no clock seam. |
| `contract.rs` | Adds `SyncPreviewRequest` and `SyncSaveRequest` mode markers beside existing preview/save request types; one `SyncBackendRequest` enum; shared backend/summary/status values; and `SyncGuidance::format_text`. `window_days` exists only on the Oura request variant, while source-path override exists only on Obsidian and audio variants. |
| `lib.rs` | Exports the completed sync, connect, and gate APIs; removes their six stub-table rows. It does not export a CLI parser or live HTTP constructor. |
| body-ingest `approval`/`lib.rs` | Widens and re-exports the existing `oura_approval` function unchanged. `BodyIngestError::kind()`/`stage()` remain the existing typed refusal contract. |

The live Plaud transport belongs to W10's adapter. Its construction consumes the
caller-provided credential, configures the documented retries/timeouts/streaming
posture, and passes separate narrow catalogue/download references into W9. W9
itself has no `std::env` read, no global clock, and no transport construction.

## State machines

### Shared sync-state

1. Read `imports/<backend>.json` through the backend-specific decoder.
2. A missing state yields `SyncStateRead::Absent`; an unreadable, malformed,
   overflowed, or non-finite state yields `SyncStateRead::Unreadable { class }`.
   Both are ordinary values that begin a fresh catalogue and never block sync.
3. A successful decode retains known fields plus all unknown root and entry
   fields.
4. Save serializes with the dedicated Python-compatible pretty writer and
   atomically replaces the state file. A write failure is a named write error;
   it is not converted into a preview success.

### Plaud

1. Obtain the caller-supplied credential. Missing credential returns a named
   configuration refusal; its value is never placed in state or an error string.
2. Ask the preview-safe catalogue seam for remote metadata, load prior state,
   then call `PlaudManifestLookup` on new/available remote IDs before assigning
   any new state.
3. Preserve existing entries while refreshing `filename` and `filesize`; promote
   an existing `available` entry to `imported` when it matches, otherwise place
   it back in the save queue. New ID-keyed entries record `fullname`, fractional
   numeric `start_time`, `duration`, and `is_trash`. Classify matched as
   `imported` with `import_timestamp`/`matched_at`, trash/short as `skipped`,
   otherwise `available`. Do not synthesize `removed`.
4. Preview returns/saves the catalogue state and never requests a temporary URL,
   bytes, or pipeline import.
5. Save orders available entries by descending numeric start time, preserving
   catalogue order for equal times, derives the reference timestamp, asks for a
   temporary URL, streams into its timestamped import destination, calls the
   pipeline with that timestamp, checkpoints after each successful import, and
   performs a final state write. Operation failures use only closed safe kinds
   in state and presentation.

### Obsidian

1. Select vault source in order: explicit path, previously retained `source_path`,
   then caller-supplied ordered candidates. Missing/non-directory source is a
   named refusal.
2. Scan caller-prepared eligible markdown notes. Each carries its relative path,
   title, numeric mtime, and content hash; content/frontmatter rendering remains
   behind the save writer rather than W9 state orchestration.
3. Preserve imported entries when content hash says unchanged while refreshing
   numeric mtime; otherwise
   make/update an `available` entry keyed by vault-relative path. Mark previous
   unseen paths `removed`.
4. Preview returns/saves state only. Save writes note segments and optional
   entity seeds, then transitions each successful note to `imported` with
   `imported_at`, `segments`, and incremented `edit_count`; failures remain
   available and are recorded in `errors`.

### Audio

1. Require an explicit source path and reject a scanner result with no audio
   candidates before state publication. Tool and filesystem-directory checks
   belong to the caller-owned scanner/probe adapters; W9 introduces no ambient
   process or filesystem policy seam for them.
2. Scan with the caller seam, excluding entries resolved under journal imports;
   key entries by POSIX relative path.
3. First promote previously available manifest matches to `imported`; then
   classify every discovered candidate as manifest-matched `imported`, probe-
   failed `unreadable`, short `skipped`, or `available`; finally mark every
   unseen prior entry `removed`, including an unseen entry promoted earlier.
4. Preview writes/returns the resulting catalogue only. Save invokes the
   pipeline once per available item and checkpoints after each attempted item.
   Success becomes `imported`; every pipeline non-success returns to
   `available` with `last_error` and an aggregate error record.

### Oura connect and consent route

1. Connect routes an explicit journal path to the native body owner and returns
   its typed authorization outcome. It neither reads nor stores a credential.
2. Preview sync never invokes consent and has no schedule effect.
3. Save sync asks the body-owned approval API before any W9 save-side operation;
   scheduled-consent expiry uses the body owner's own clock.
4. An approval error becomes `ConsentGateOutcome::Blocked`; it is formatted or
   encoded only by a caller, and W10 maps it to exit 2.
5. An approval success becomes `ConsentGateOutcome::Allowed`; standing consent
   and raw-retention handling remain inside the body owner. W9 emits neither
   `cron_hint` nor a crontab line.

## Test plan

| AC | Test location | Assertion and negative twin |
|---|---|---|
| 1 | `core/crates/solstone-core-import/tests/sync_state.rs` | Assert `SYNC_BACKEND_INVENTORY` is exactly `plaud`, `obsidian`, `audio`, `oura`, in the order formed by `import_reference_grammar.json`'s `syncable_backends_instantiated` followed by `native_sync_backends`; the vendored oracle `.sync` is corroborating data. A list missing `oura`, reordered, or containing a retired backend fails the literal fixture comparison. W10's later rendering is a dispatcher-only residual. |
| 2 | `core/crates/solstone-core-import/tests/sync_plaud.rs` and `sync_audio.rs` | Preview seam structs omit download/pipeline fields, while matching save seam structs require them; adding a preview download path therefore fails to compile. Save tests supply those seams and act. |
| 3 | `core/crates/solstone-core-import/tests/contract_fixture_shape.rs`, `sync_obsidian.rs`, and `sync_audio.rs` | `SyncBackendRequest` directly models one selected backend: an Obsidian or audio source-path override reaches only that variant, while the Oura variant alone carries `window_days`. A compile-fail request-shape check proves Plaud cannot receive `window_days`; construction assertions prove both native-window and local-source directions. W10 argv parsing is the sole dispatcher residual. |
| 4 | `core/crates/solstone-core-import/tests/consent_gate.rs` | Missing confirmation returns `Blocked`, `CONSENT_GATE_EXIT_CODE` is 2, and pure formatted text names the remedy. A real body approval fixture with `scheduled=true`, `confirmed=true`, and no valid `scheduled_sync` block returns the `scheduled_sync_consent_missing` reason family. The negative twin fails if either condition is treated as approval. |
| 5 | `core/crates/solstone-core-import/tests/consent_gate.rs` | Unsupported/unknown approval schema, absent artifact, and unreadable/malformed artifact each become `Blocked`; `check_oura_sync_save` has no save or pipeline authority in its signature, and the test also asserts no `imports` directory was created. |
| 6 | `core/crates/solstone-core-import/tests/contract_fixture_shape.rs` | A backend outcome carrying `SyncGuidance` renders its text through W9's pure formatter. The test enumerates the exact public fields of all six sync seam structs and rejects schedule-writer, schedule-path, and `cron_hint` identifiers, so adding schedule authority to an existing seam fails the field-shape assertion. This does not add a `cron_hint` producer: the retained Oura branch remains an early return before its dead consumer. W10 later chooses whether to print the already-rendered text. |
| 7 | `core/crates/solstone-core-import/tests/sync_state.rs` | The vendored oracle supplies the agreed sync path/backends while an inline, prep-table-derived test literal covers every Plaud, Obsidian, and audio union key plus unknown members; each round-trips with parsed field-for-field equality. Separately, library-authored state compares byte-for-byte for two-space indent, ASCII escaping, insertion order, and no trailing newline. Assert file mode `0o600` through `AtomicWriteOptions { mode: Some(0o600) }` and parent/import directories `0o700` through `create_directory_with_mode`. |
| 8 | `core/crates/solstone-core-import/tests/connect.rs` and existing `tests/resolution.rs` resolver-corpus coverage | `connect.rs` routes once to the native owner-present connection operation and returns its typed authorization result. The paired resolver-corpus assertion for `source=oura::plain.txt` remains the W1b refusal at `cli.py:582-588` (`OuraRequiresSync` / sync remedy), proving connect does not revive the retired file-import route. W10 later parses `--connect`, but has no ownership of either semantic. |
| 9 | `core/crates/solstone-core-import/tests/sync_audio.rs` and `sync_plaud.rs` | Script an already-imported match plus a later failure. Assert the matched record remains `imported`, the failed item remains `available` with `last_error`, `AudioItemOutcome` names the missing item, aggregate `errors` includes it, and the per-item checkpoint precedes the next item. A completion summary that reports success despite the failed item fails the test. |
| 10 | `core/crates/solstone-core-import/tests/sync_plaud.rs`, `connect.rs`, and `consent_gate.rs` | Recording state/error/render seams prove credentials never enter state, paths, or diagnostics; only caller-supplied reference paths are touched. No W9 operation has an outbound transport other than the injected Plaud API calls, and body sync/connect are direct native owner calls rather than a new body-data egress route. |
| 11 | Settled repository gate | Run `make ci` only in the implementation validation stage and report its actual outcome. Stub retirement has separate unit coverage in `core/crates/solstone-core-import/tests/stub_table.rs`; preview-type discipline has its own compile/runtime seam tests and is not substituted for any AC. |

No validation command runs in this design stage. Implementation validates only
the narrow W9 crate tests and the prescribed gate in its subsequent stage.

## Risks and fixed scope boundaries

- **State bytes:** byte-level Python compatibility needs a purpose-built writer;
  generic serde pretty serialization is insufficient. The fixture proves the
  settled subset, while a live Python comparison is unavailable in this tree.
- **Body API visibility:** the narrow public approval export is required before
  import can route to it. It must not broaden body write authority or expose
  mutable approval artifacts.
- **Audio source authority:** the scanner/probe/pipeline seams prevent W9 from
  silently acquiring process or filesystem policy. W10 decides production
  adapters.
- **No Article 8 gate is required by this plan.** It adds no outbound data path,
  no credential location, no consent relaxation, and no novel owner-data
  movement. Any later proposal that does one of those things must stop and use
  the Article 8 gate before design or implementation proceeds.
