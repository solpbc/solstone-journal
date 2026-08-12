# W6 Native ICS and Obsidian Source Design

## Purpose and boundary

W6 ports the read/compute half of Python's `ICSImporter` and
`ObsidianImporter` into `solstone-core-import-sources`, specifically
`src/ics.rs` and `src/obsidian.rs`.

It adds native, source-specific `detect`, in-memory extraction, entity
projection, and `ImportPreview` construction. It reads only owner-supplied
paths. It does not parse CLI arguments, select a source in the registry, stage
or copy imports, create a journal directory, write chronicle files or content
manifests, seed entities, publish segments, index, or invoke
`ObsidianSyncBackend`. Those remain later-waves/caller responsibilities.

The source of truth is `solstone/think/importers/ics.py:24-418` and
`solstone/think/importers/obsidian.py:26-343`. The save paths at
`ics.py:420-596` and `obsidian.py:345-523` are read only to preserve their
placement semantics, not ported here.

The implementation adds `icalendar = "0.17.13"` to
`core/crates/solstone-core-import-sources/Cargo.toml`, with its default
`parser` feature and `chrono-tz` feature. The latter uses the existing workspace
`chrono-tz` dependency to resolve TZID-qualified event times. `icalendar` has
MSRV 1.88 (below the workspace's 1.95), is
MIT/Apache-2.0, and is actively maintained. Its public API supplies
`Calendar::events()` for VEVENT iteration and the public `Component` trait's
`properties()`, `multi_properties()`, `property_value()`, and `Property`
parameter access. That is sufficient for the required
`LAST-MODIFIED`/`CREATED`/`DTSTART`, `ORGANIZER`, multi-valued `ATTENDEE`, and
other VEVENT properties without parsing lines locally. This is preferred over
archived `ical` and `ical_rust`, which has a weaker maintenance/MSRV signal.

ICS ZIP support is source behavior, not routing behavior. The crate will also
take the existing locked `zip = "=2.4.2"` dependency directly with the same
`default-features = false, features = ["deflate"]` policy used by the existing
core ZIP consumers. `chrono` and `regex` are existing workspace dependencies
needed for dates and Obsidian extraction. No dependency is needed merely to
set fixture mtimes: Rust's `std::fs::FileTimes` is sufficient and already has
an in-repo test precedent.

## Acceptance criteria (verbatim)

1. `[test]` ICS `detect` and `preview` match the vendored source oracle for
   `cal.ics`: detected, two events, UTC creation-date range 20260311 through
   20260312, one attendee entity, and the owner-facing summary.
2. `[test]` Obsidian `detect` and `preview` match the vendored vault facts:
   marker/three-note detection, three items, one wikilink entity, and daily
   versus knowledge counts. Its date range is proven from constructed fixture
   mtimes, never the test clock.
3. `[test]` ICS extraction preserves the reference creation timestamp
   priority, duration, organizer/attendee email normalization and dedupe; the
   attendee projection yields Person entities only for attendees having both a
   name and an email.
4. `[test]` Obsidian extraction and entity projection preserve daily-note
   recognition, wikilinks, `@`-prefix Person precedence, folder-derived type,
   and Topic fallback, including `@` filenames not linked elsewhere.
5. `[test]` W6 does not change resolution semantics: the existing four
   no-match corpus rows remain unclaimed, and the three two-claimant rows keep
   their existing ordered results. W6 does not derive predicates from display
   `file_patterns`.
6. `[test]` Each extracted entry carries the day the existing writer would
   place it in: calendar entries use the UTC day of `create_ts`; every
   Obsidian entry, daily or knowledge, uses its file mtime's local calendar
   day.
7. `[test]` Every W6 read path leaves the supplied source tree byte-for-byte
   unchanged, including an ICS archive and a vault tree.
8. `[check]` A Google Takeout ZIP holding Calendar and Gemini data remains an
   unresolved product-routing question; W6 neither changes first-claim
   selection nor implements multi-claim dispatch.
9. `[test]` Once both source modules are implemented, neither remains in the
   reserved-module inventory and neither exposes `reserved_seam()`.

| AC | Test location and concrete test function(s) |
|---|---|
| 1 | New `solstone-core-import-sources/tests/ics.rs`: `ics_oracle_detect_and_preview_match_fixture`, `ics_preview_uses_utc_creation_days`; `tests/obsidian.rs::source_detect_rejects_corpus_no_match_directories` directly checks both source detectors against the four corpus no-match directory shapes. |
| 2 | New `solstone-core-import-sources/tests/obsidian.rs`: `obsidian_oracle_detect_and_preview_match_fixture`, `obsidian_preview_uses_constructed_mtimes_not_the_clock`. |
| 3 | `ics.rs`: `parse_events_uses_creation_timestamp_priority_and_computes_duration`, `calendar_attendee_entities_require_name_and_email`. |
| 4 | `obsidian.rs`: `collect_notes_and_wikilink_entities_preserve_type_precedence`. |
| 5 | Existing `registry_fixture_contract.rs::registry_and_auxiliary_grammar_match_the_frozen_fixture_contract`, `routing_order.rs::first_claimed_uses_fixture_order_and_fixture_zip_claimants`, `solstone-core-import/tests/resolution.rs::ac16_corpus_directory_and_extension_boundaries` for routing-level predicate stand-ins, and `tests/obsidian.rs::source_detect_rejects_corpus_no_match_directories` for direct ICS/Obsidian no-match detector certification. |
| 6 | `ics.rs::calendar_entries_expose_writer_day` and `obsidian.rs::note_entries_use_mtime_day_for_daily_and_knowledge_notes`. |
| 7 | Extend `source_immutability.rs` with `implemented_source_reads_leave_the_owner_source_unchanged`. |
| 8 | Existing `routing_order.rs::first_claimed_uses_fixture_order_and_fixture_zip_claimants` remains the first-claim regression guard; the design's open-question record is the acceptance evidence, not a new product-routing test. |
| 9 | Extend `stub_table.rs` with `implemented_source_modules_have_no_unimplemented_seam`. |

## Deliberate decisions

1. **Use `icalendar` 0.17.13.** Parse an owned UTF-8 ICS string into an
   `icalendar::Calendar`, enumerate `Calendar::events()`, access singleton
   properties through `Component`, and access repeated `ATTENDEE` properties
   through `Component::multi_properties()`. The dependency is intentionally
   local to `solstone-core-import-sources`; no new cross-crate parsing facade
   is introduced.
2. **Expose read values, not a save-shaped importer object.** Each module has
   `detect`, an extraction function returning in-memory entries, `preview`,
   and a small entity-projection helper. This permits direct tests of the
   entity/day facts which a later writer will need, without prematurely
   coupling this source crate to staging or publication.
3. **Calendar placement stays creation-moment based.** `LAST-MODIFIED`, then
   `CREATED`, then `DTSTART` supplies `create_ts`; date-only values become
   midnight and naive values are treated as UTC. The entry's `day` is the UTC
   calendar day of that timestamp. Events without a usable value are skipped,
   exactly as Python `_parse_events` does.
4. **Obsidian placement stays mtime based for every note.** Python
   `ObsidianImporter.process()` assigns `mtime` to all notes at
   `obsidian.py:377-400`, then calls `window_items(notes, "mtime", tz=None)`
   at line 470. Daily-name classification does not alter placement. Therefore
   `NoteEntry.day` derives from its mtime in the local calendar timezone for
   daily and knowledge notes alike. W6 must not invent a frontmatter date key
   or assign knowledge notes a content date.
5. **Disclose the mtime range.** The Obsidian preview continues to compute
   `date_range` from mtime, matching the writer, but for a non-empty vault its
   summary becomes `<Python count summary>; date range reflects file
   modification time`. This makes the existing caveat visible to the owner.
   It is an intentional owner-text divergence from the captured Python oracle;
   the fixture test obtains the original count text from the oracle and
   appends the documented suffix rather than hand-transcribing it.
6. **Keep Python's intentionally different detect and walk predicates.**
   Obsidian `detect` counts non-hidden Markdown descendants and accepts an
   `.obsidian` or `logseq` marker. Collection excludes hidden directories,
   `templates`/`_templates` case-insensitively, and `logseq/.recycle`. Do not
   make detection use the stricter collection walk or turn `file_patterns`
   into predicates.
7. **Entity projection is pure and deterministic.** Calendar projection
   normalizes/deduplicates organizer then attendees by lowercase email and
   emits only named email addresses as Person entities. Obsidian projection
   derives title folder types across all collected notes, then applies
   `@` Person > folder type > Topic and adds `@` filenames. Outputs are sorted
   by entity name where the Python helper does so; no entity write occurs.
8. **Fixture ownership is local to the source crate.** Duplicate the compact
   `Tree`/`Drop` fixture helper pattern from
   `solstone-core-import/tests/resolution.rs` into each new source-crate test
   file. Do not move it into a cross-crate test utility: no such shared helper
   exists, and coupling test crates for a small filesystem builder is broader
   than W6.
9. **Retire completed seams mechanically.** Follow `df22182b6`: delete each
   implemented `reserved_seam()`, remove both table rows, update the count,
   and add an explicit absence test. Do not keep an obsolete seam for
   compatibility; it has no callers.

## Public library surface

All W6 source files keep repository SPDX headers. Return types use source-local
error enums for filesystem, archive, and source-decoding failures; malformed
individual VEVENT payloads follow Python and contribute no entry rather than
turning a readable source into a writer operation.

| Module | Public surface |
|---|---|
| `ics.rs` | `pub fn detect(path: &Path) -> bool`; `pub fn parse_events(path: &Path) -> Result<Vec<CalendarEntry>, IcsError>`; `pub fn preview(path: &Path) -> Result<ImportPreview, IcsError>`; `pub fn attendee_entities(entries: &[CalendarEntry]) -> Vec<CalendarEntity>`. |
| `ics.rs` values | `CalendarEntry { title: String, content: String, create_ts: DateTime<Utc>, day: String, ts: Option<String>, end_ts: Option<String>, duration_minutes: Option<i64>, location: Option<String>, attendees: Vec<CalendarAttendee>, recurrence: Option<String> }`; `CalendarAttendee { name: String, email: String }`; `CalendarEntity { day: String, name: String, email: String, entity_type: String }`, where `entity_type` is `"Person"`. |
| `obsidian.rs` | `pub fn detect(path: &Path) -> bool`; `pub fn collect_notes(path: &Path) -> Result<Vec<NoteEntry>, ObsidianError>`; `pub fn preview(path: &Path) -> Result<ImportPreview, ObsidianError>`; `pub fn wikilink_entities(notes: &[NoteEntry]) -> Vec<ObsidianEntity>`. |
| `obsidian.rs` values | `NoteEntry { title: String, source_path: PathBuf, content: String, tags: Vec<String>, wikilinks: Vec<String>, is_daily: bool, daily_note_day: Option<String>, day: String, inferred_entity_type: Option<String> }`; `ObsidianEntity { name: String, entity_type: String }`. `source_path` is relative to the vault; `daily_note_day` preserves a parsed filename date while `day` is the writer-placement day. |

`preview` calls its module's extraction/collection function, then aggregates the
fixed `solstone_core_import::ImportPreview` fields. No source module returns
`ImportResult`, `CreatedSegment`, or a publication input in W6.

## Source read and computation pipeline

### ICS

1. `detect` rejects non-files, accepts case-insensitive `.ics`, and for a ZIP
   returns true only when an archive member has an `.ics` suffix; a bad ZIP is
   a false detector result.
2. `parse_events` reads one `.ics` file or every case-insensitive `.ics` ZIP
   member. It parses each readable calendar and iterates its VEVENTs.
3. For each event, compute `create_ts`, `day`, start/end strings, duration,
   title/content/location, organizer/attendee list, and recurrence description.
   Preserve organizer-first and first-email-wins ordering.
4. `attendee_entities` consumes only these entries, filters missing name/email,
   and produces the later-writer-ready Person facts. `preview` counts unique
   attendee emails directly from the entries and takes its date range from
   UTC `create_ts` days.

### Obsidian

1. `detect` uses the reference directory marker/visible-Markdown heuristic.
2. `collect_notes` uses the reference collection walk, reads UTF-8 with BOM
   handling, and records relative path, title, tags, links, daily-name
   information, folder type, and mtime-derived `day`. Empty or unreadable notes
   remain entries with empty content-derived facts so preview counts match Python.
3. `wikilink_entities` first builds the title-to-folder-type map, then applies
   Python's `@` and Topic precedence to all links and `@` filenames.
4. `preview` aggregates daily/knowledge count and unique wikilinks from the
   collected entries and derives the range from entry `day` values. It adds the
   explicit mtime caveat described above.

## Fixture and test plan

Vendor `/home/jer/import-fixtures-260811/w6-w8-source-oracles.json` unchanged
in content as `core/fixtures/import_source_preview_oracle.json`. The
snake_case destination follows the existing vendored
`import_resolver_corpus.json`, `import_detection_corpus.json`, and
`import_reference_grammar.json` convention. Tests consume it with
`include_str!` and `serde_json`; no expected source-oracle case is
hand-transcribed into Rust.

All source trees below are constructed under the local `Tree` helper, not
checked-in fixture directories:

- `cal.ics`: two valid VEVENTs for the oracle, one with a named attendee and
  one without. The same file drives detection/preview and proves the
  no-attendee branch. A focused timestamp-priority event set adds
  LAST-MODIFIED, CREATED-only, DTSTART-fallback, all-day, and mismatched
  timezone-duration cases.
- `vault-oracle/`: three visible Markdown files—one daily and two
  knowledge—with a single wikilink. Its fixture-derived preview assertion
  consumes the source oracle.
- `vault-mtime-range/`: daily and knowledge files receive fixed historical
  mtimes (for example 2024-01-02 and 2024-03-04 at midday UTC) using
  `File::set_times`. The expected day is derived from the fixed file metadata
  in the same local-timezone convention as the production port, never from
  `SystemTime::now`; this is the AC2/AC6 anti-clock twin. No `filetime`
  dependency is needed.
- `vault-entities/`: a daily note; `People/Aster Placeholder.md`; a typed-folder
  note whose target is also linked with `@`; an `@Offline Placeholder.md` filename not
  linked elsewhere; and a reference note linking each plus `[[Loose Topic]]`.
  It proves daily/knowledge classification, folder inference, `@` precedence,
  unlinked `@` filename inclusion, and Topic fallback.
- `source-immutable/`: an ICS archive plus a vault created by the same helper
  for the before/action/after immutable-tree observation.

The four no-match rows—`bare::dir_vault_1md`,
`bare::dir_vault_3md_hidden`, `bare::dir_pdf_in_subdir`, and
`bare::dir_only_images`—and the two-claimant rows are already established by
the vendored resolver corpus and its source-crate registry tests. W6 reuses
those tests and adds no duplicate directory corpus or competing source-routing
test.

## What a wrong port would have done

- It would have selected the date encoded in a daily filename or invented a
  frontmatter date for a knowledge note, despite Python placing both from
  mtime.
- It would have made the Obsidian detection heuristic share the stricter
  content walk and silently changed what claims a vault.
- It would have built source predicates from `file_patterns`, so `*.md` files
  or arbitrary ZIPs would be misclassified.
- It would have deduplicated people by display name, omitted organizer-first
  ordering, or emitted unnamed calendar attendees.
- It would have let folder inference override `@`, dropped an unlinked `@`
  filename, or assigned Topic before checking a typed folder.
- It would have derive a publication day from an output path rather than
  exposing the precomputed source-entry day required by the existing writer.
- It would have retained `reserved_seam()` and claimed an implemented module
  remained unfinished.

## File sequence

1. **Vendor fixture commit:** add
   `core/fixtures/import_source_preview_oracle.json` unchanged in content and
   add fixture-parsing coverage using `include_str!`/`serde_json`.
2. **ICS implementation commit:** add the direct dependencies, implement
   `ics.rs` detect/extraction/preview/entity projection, and add
   `tests/ics.rs` using local constructed trees.
3. **Obsidian implementation commit:** implement `obsidian.rs`
   detect/collection/preview/entity projection and add `tests/obsidian.rs`,
   including the explicit mtime fixture and summary caveat.
4. **Retire-stubs commit:** delete both `reserved_seam()` functions, remove
   `ics` and `obsidian` from `MODULE_STUBS` in `lib.rs` (12 to 10), update the
   stub count, extend immutability coverage, and add the explicit implemented
   modules absence test.
5. **Design-doc commit:** add this design document. The implementation commits
   may be rebased into this order, but their boundaries must remain reviewable.

## Source/corpus disagreement audit

The vendored W6 oracle records Python's current preview strings and its
Obsidian date range from captured file mtimes. W6 deliberately keeps that mtime
range and all count/entity facts, because `ObsidianImporter.process()` uses
mtime uniformly for actual segment placement. The only intended difference is
the documented Obsidian summary suffix identifying mtime as the range source;
the test derives its base text from the oracle and asserts the suffix.

The source oracle's Obsidian date happens to be 20260811, which can coincide
with a run date. The separate historical-mtime fixture is therefore mandatory;
it proves that native code reads fixture metadata rather than the clock.

`import_resolver_corpus.json` records that a Takeout ZIP containing Calendar
and Gemini resolves to ICS under today's ordered, first-claim registry. Whether
an owner expects that one archive to enter both Calendar and a Gemini
multi-claim import path is an open product question. W6 names it but makes no
decision: it neither changes `detect()` nor extends registry routing.

The Python `ObsidianSyncBackend` is outside this port. Its persisted sync state,
incremental comparison, writes, and entity seeding must not leak into these
read-only source APIs.
