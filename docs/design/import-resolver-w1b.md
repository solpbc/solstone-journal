# W1b Import Resolver Design

## Purpose and boundary

W1b ports the resolution half of `journal importer` into library crates only:

- `solstone-core-import`: detection orchestration, timestamp validation and
  resolution, generic-source hashing/deduplication, and import stream naming.
- `solstone-core-import-sources`: ordered registry selection plus the Apple
  pre-empt routing and Oura file-route refusal.

It does not parse argv, print, exit, start a process, create staging, write a
manifest, acquire a lock, write streams, or index. The caller owns all of
those later actions. The existing Python source of truth is
`solstone/think/importers/cli.py:493-690`; the resolver corpus's
`native_detector_answers_no` pass is the routing oracle.

The library has no global state. All potentially fallible external decisions
are caller-supplied closures. No new crate is required: `regex`, `chrono`, and
`sha2` are already workspace dependencies.

## Acceptance criteria (verbatim)

1. `[test]` A missing path raises before any other resolution step. *(corpus `missing_path`)*
2. `[test]` **The pre-empt gate is presence, not membership.** On an Apple export directory that also holds a
   top-level PDF: with **no** `--source` the Apple path is reached; with `--source recording`,
   `--source nosuch` and `--source plaud` the pre-empt is skipped and it resolves to `document`; with
   `--source ics` it resolves to `ics`. Must fail against an implementation that filters `--source` to
   registry membership before deciding whether to pre-empt.
2a. `[check]` The Apple detector is an **injectable seam** in the native crate, so AC 2 and AC 5 can drive
   both answers without depending on the shipped binary.
3. `[test]` A **non-registry** `--source` is silently ignored on files: `.ics`/`.pdf`/`.png` still sweep to
   `ics`/`document`/`image`; generic `.txt`/`.m4a` stay generic. It must not error.
   *(corpus `source=recording::*`, `source=nosuch::plain.txt`)*
4. `[test]` **Ordering decides**, from the corpus's two-claimant inputs: a Takeout carrying Calendar **and**
   Gemini → `ics`; a Claude export carrying an `.ics` → `ics`; a 3-markdown vault with a top-level PDF →
   `obsidian`. And **with the registry order reversed the selections change.** Every single-claimant case
   passes under any ordering, so single-claimant tests cannot detect a missing ordering guarantee.
   (`routing_order.rs` already does this for the two zips — extend, don't duplicate.)
5. `[test]` With the Apple detector answering *no*, an Apple export directory containing a top-level PDF
   resolves to **`document`**; with the pre-empt active it does not. *(corpus `bare::dir_apple_AND_pdf`)*
6. `[test]` **The real-predicate form of the swallow:** a generic binary archive no source claims resolves to
   the **generic path and does not raise**. *(corpus `bare::zip_generic`)* Do not assert "the sweep continues
   past a raising predicate" — the only raising predicate sits **last**, so continuation past it is vacuous,
   and manufacturing a fallible registry measures the fake.
7. `[test]` An Apple Health source routes to the native body path **and** leaves no staging directory, lock,
   manifest, stream record or index row. Assert the absences.
8. `[test]` An Oura file source is refused by name and the refusal names the sync remedy.
   *(corpus `source=oura::plain.txt`)*
9. `[test]` Generic dedup: hashes the **original**, not a staged copy; skips only when the recorded entry
   count is **> 0**; and given a manifest recording **0** entries, **no skip fires and resolution proceeds**.
   Without that negative twin a port locks the owner out of the file permanently.
10. `[test]` `--dry-run` suppresses generic dedup entirely.
11. `[test]` Both validators: the two shape refusals and the three calendar refusals, **each by its own
    class**. Must fail against a shape-only regex.
11a. `[test]` Both validators fire on the **file-importer** path too — a registry `--source` with an
    impossible day is refused by the calendar validator.
11b. `[test]` A non-ASCII-digit timestamp passes the shape validator (its character class is Unicode-aware)
    and is refused by the **calendar** validator, with the calendar message class.
12. `[test]` `--auto`'s four states against a **detectable** timestamp: absent → skip · bare → adopt ·
    text → adopt · **empty string → skip**. *(corpus `auto=*::dated.m4a`)*
12a. `[test]` **Guidance needs a different input.** On a detectable source the deterministic resolver answers
    and the model detector is never called, so guidance cannot reach it. Assert guidance reaching the
    detector on a **non-detectable** source with `--deterministic-only` **off** — and that `--auto ""` there
    passes an empty guidance the detector drops.
12b. `[test]` The model detector is **invoked exactly once** on the non-detectable, non-deterministic-only
    path. Without this positive twin, AC 11/12/13 are all satisfied by an implementation that never builds
    model detection at all.
12c. `[test]` A model-detection failure that is neither "no engine configured" nor a parse error surfaces as
    a refusal **naming the remedy**, not as a raw provider string reaching the owner.
13. `[test]` Deterministic-only with nothing found → successful skip, and the model detector is **never
    invoked**. Assert the non-invocation. *(corpus `bare::audio.m4a`, `auto=absent::audio.m4a_undetectable`)*
14. `[test]` Stream naming, **all seven oracle cases, literal strings**, from
    `core/fixtures/import_stream_name_oracle.json`. The registry-source rows are the positive twin — without
    them an implementation that hardcodes `import.audio`/`import.text` passes; the `--source recording` rows
    are the leak guard — must fail against a pass-through, and **pin the exact string** rather than "contains
    no trace", which also passes for an implementation that merely strips the value.
15. `[test]` Canonicalisation: separators folded; a name that would contain `..` **refused**, not sanitised;
    an otherwise-invalid name errors.
16. `[test]` Boundaries from the corpus: uppercase `.PDF` → `document` · PDF only in a **subdirectory** →
    claimed by nobody · three markdown files under a dot-directory → nobody · three **nested** → `obsidian` ·
    a `logseq/` marker → `obsidian` · a directory of only images → nobody.
17. `[check]` **Decide and record** whether a directory claimed by nobody keeps deriving `import.audio`.
18. `[check]` The only filesystem effect of this wave is the journal-root directory creation performed by the
    journal-path helper — and that is **conditional, not universal**: on the file-importer dry-run path the
    helper is never called at all. Name the **first real write** (`_setup_import`) and confirm it is outside
    this wave.
19. `[check]` `make ci` reported honestly.

Not acceptance criteria: "a named source fails to load" is a Python dynamic-import state that cannot occur
with a compiled-in registry.

## Deliberate decisions

1. The Apple pre-empt is driven by a generic `FnMut(&Path) -> Result<bool, E>`
   seam. It is called only when `source.is_none()` and the source is a directory
   or case-insensitive `.zip`; no registry-membership check precedes it. `Ok(true)`
   routes to Apple, `Ok(false)` continues normal resolution, and `Err` is a
   terminal named Apple-detector refusal. It neither launches a binary nor owns
   filesystem authority.
2. The model detector is a generic
   `FnMut(&Path, Option<&str>) -> Result<Option<DetectedTimestamp>, ModelDetectionError<E>>`
   seam. `Unavailable` represents the reference's no-engine, validation, and
   parse failures and becomes no detection; `Failed(E)` is retained as a typed
   source but never interpolated into owner-facing output. A no-detection result
   on the non-deterministic-only path is the reference's timestamp refusal, not
   a successful skip.
3. `AutoTimestamp` has four variants: `Absent`, `Bare`,
   `Guidance(NonEmptyGuidance)`, and `EmptyGuidance`. Its only raw constructor
   maps `None`, bare presence, non-empty text, and `""` respectively. `adopts()`
   is an exhaustive match: only `Bare` and `Guidance` return true. Thus an empty
   string cannot become adopting through a later dropped `is_empty()` check.
   `guidance()` returns `None`, `None`, `Some(nonempty)`, and `Some("")` in the
   same order. This preserves `cli.py:654` and `cli.py:658` structurally.
4. `stream_name.rs` belongs in `solstone-core-import`. It does not use
   `solstone-core-segment::projection::project_stream_name`, which projects an
   existing name to a filesystem location rather than deriving an import label.
   The three-extension detection-skip set (`.m4a`, `.txt`, `.md`) and the
   two-extension text-stream set (`.txt`, `.md`) remain separate named constants.
   Canonicalisation strips before validation. A comment cites
   `streams.py:98-116` and records the probe result: Rust `regex` rejects a raw
   trailing newline where Python's `$` accepts it; stripping keeps the public
   result compatible.
5. `ResolutionError` has separate `InvalidTimestampShape` and
   `InvalidTimestampCalendar` variants. Corpus-pinned owner messages are exact,
   including Python's `strptime` messages. `ModelDetectionFailed` says the
   remedy and carries its typed underlying cause as structured data.
6. Three behavior decisions are fixed:
   - An unclaimed directory is refused at stream derivation after deduplication
     and timestamp resolution. `cli.py:681-690` derives a label, but that branch
     is unreachable in practice because staging then raises `IsADirectoryError`.
     W1b deliberately replaces that unreachable failure with a named remedy;
     it applies regardless of extension. A `.pdf`-named directory remains the
     earlier PDF refusal.
   - A `.pdf`-named directory reaches the PDF refusal after no importer claims
     it. This is retained from `cli.py:557-561`; narrowing the check to files
     would silently route it as generic audio.
   - Model errors other than no-engine/parse errors become the named-remedy
     refusal above. This deliberately diverges from
     `detect_created.py:268-288`, which otherwise leaks such errors; the design
     and implementation comment must name the divergence.
7. `registry.rs` is the precedent for partial completion: it exposes
   `ORDERED_FILE_IMPORTER_NAMES` and `first_claimed` while retaining
   `reserved_seam()` (`core/crates/solstone-core-import-sources/src/registry.rs:12-36`).
   W1b retains the `registry` stub row and its convention. It removes no
   `MODULE_STUBS` rows: `detect` and `dedupe` are only partly implemented, and
   `apple_health`/`oura` retain future body-reader seams. Counts stay 19 and 12;
   both existing stub-table tests remain valid. This is not a claim that a
   module is wholly complete.

## Public library surface

All new source files receive the repository SPDX header.

| Module | Public surface |
|---|---|
| `detect.rs` | `resolve_import(options: &ResolutionOptions<'_>, seams: &mut ResolutionSeams<A, C, D, M, L, T>) -> Result<ResolutionOutcome, ResolutionError<AE, ME>>`, where `A: FnMut(&Path) -> Result<bool, AE>`, `C: FnMut(RegistrySource, &Path) -> Result<bool, CE>`, `D: FnMut(&Path, Option<&str>) -> Option<DetectedTimestamp>`, `M: FnMut(&Path, Option<&str>) -> Result<Option<DetectedTimestamp>, ModelDetectionError<ME>>`, `L: FnMut(&SourceHash) -> Option<ManifestSummary>`, and `T: FnMut() -> Timestamp`. `AE` and `ME` remain generic typed causes. |
| `detect.rs` seams | `ResolutionSeams<A, C, D, M, L, T> { apple_detector: A, claims: C, deterministic_detector: D, model_detector: M, manifest_lookup: L, generated_timestamp: T }`. Each field has a one-line doc contract: Apple runs only at the source-absent directory/ZIP pre-empt and its error is terminal; claims runs during ordered sweeps and an error is a swallowed non-answer; deterministic detection receives the path and Python-equivalent optional original filename; model detection runs once only after deterministic no-match and receives optional guidance; manifest lookup runs only for non-dry-run generic dedup and `None` is no prior manifest; generated timestamp runs only for a selected source with no explicit timestamp. Named fields prevent positional seam mix-ups and ambient-clock reads. Deterministic detection is injected because this wave ports resolution, not `resolve_created_deterministic`'s filename/Exif extraction implementation; its reference inputs are `path` and `original_filename` (`detect_created.py:205-227`, called at `cli.py:621-624`). |
| `detect.rs` types | `ResolutionOptions { media: &Path, source: Option<&str>, timestamp: Option<&str>, auto: AutoTimestamp, dry_run: bool, deterministic_only: bool, force: bool }`; `ResolutionOutcome::{RouteAppleHealth, Skipped { reason, detected_timestamp }, Resolved { source, timestamp, stream }}`; `ManifestSummary { entry_count: u64 }`; `SkipReason::{AlreadyImported, NoDeterministicMatch, TimestampRequired}`; `ResolvedSource::{Registry(RegistrySource), GenericAudio, GenericText}`. Every refusal is `Err(ResolutionError::...)`; no outcome variant represents a refusal. |
| `timestamp.rs` | `AutoTimestamp::from_raw(raw: Option<Option<&str>>) -> AutoTimestamp`; `validate_timestamp(raw: &str) -> Result<Timestamp, TimestampError>`; `DetectedTimestamp { timestamp: Timestamp }`; pure helpers for timestamp candidate handling. The deterministic seam supplies an already-resolved deterministic answer because metadata/Exif extraction is outside this wave. |
| `dedupe.rs` | `hash_source(path: &Path) -> Result<SourceHash, HashSourceError>`. It constructs the existing `SourceHash` with its current `new(String)` API; W1b does not redesign that type. |
| `stream_name.rs` | `import_stream_name(import_source: &str) -> Result<String, StreamNameError>` and `canonicalize_stream_name(base: &str, qualifier: Option<&str>) -> Result<String, StreamNameError>`. The resolver calls the former only after final source selection. |
| `registry.rs` | Retain `first_claimed`; the resolver-owned order and source type live in `solstone-core-import` because this crate depends on it. |
| `apple_health.rs` / `oura.rs` | Retain only future-body `reserved_seam()`s. The Apple routing verdict and Oura refusal live in `solstone-core-import`, where the resolver can use the single truth source. This is a scope deviation: putting them here would require inverting the crate dependency direction. |

`ResolutionOptions` intentionally carries the owner-facing fields requested by
this wave. Staging, journal path creation, and all mutation options do not
belong in it.

## Resolution state machine

0. **Existence:** reject a missing source immediately, before detector,
   lookup, timestamp, or journal callback. Reference: `cli.py:497-499`; corpus
   `missing_path`.
1. **Apple pre-empt:** if `source is None` and the source is a directory or
   `.zip`, call the injected Apple detector. Yes returns `RouteAppleHealth`; no
   continues; error is a terminal named refusal. Reference: `cli.py:507-520`;
   corpus `bare::dir_apple_AND_pdf` (detector-no pass). AC 2's non-registry
   directory inputs are constructed temporary trees.
2. **Named registry source:** exact compiled-in source names select immediately;
   unknown names are ignored. Presence still suppresses step 1. Reference:
   `cli.py:523-533`; corpus `source=ics::zip_apple`,
   `source=obsidian::dir_apple_AND_pdf`, and `source=recording::*`.
3. **Directory sweep:** with no selection, evaluate registered claim predicates
   in `ORDERED_FILE_IMPORTER_NAMES` order and take the first true claim.
   Predicate non-answers are ordinary non-matches. Reference: `cli.py:535-542`
   and `file_importer.py:128-136`; corpus `bare::dir_vault_3md_AND_pdf` and
   `bare::dir_apple_AND_pdf`.
4. **Unknown-extension file sweep:** with no selection, sweep only when suffix
   is not `.m4a`, `.txt`, or `.md`, retaining registry order. Reference:
   `cli.py:544-555`; corpus `source=recording::{cal.ics,doc.pdf,pic.png}` and
   `bare::zip_generic`.
5. **PDF boundary:** if still unselected and the lowercased suffix is `.pdf`,
   refuse with the document-importer remedy, even for a directory. Reference:
   `cli.py:557-561`; constructed `.pdf` directory.
6. **Early source exits:** Apple route stops before generic dedup/timestamps;
   selected Oura stops with the sync-remedy refusal. Reference:
   `cli.py:563-588`; corpus `source=oura::plain.txt`.
7. **Generic dedup:** only when unselected and not dry-run, hash the original
   path and look up its manifest. `force` still computes the source hash but
   suppresses only the duplicate skip. Skip only when `entry_count > 0`.
   Reference: `cli.py:590-614`.
8. **Timestamp and stream:** selected file importers receive a caller-supplied local import-time
   timestamp if absent; generic sources use deterministic then model detection.
   Validate every supplied/adopted timestamp, then derive registry source or
   `.txt`/`.md` text, otherwise audio, and form the stream. Reference:
   `cli.py:616-690`; corpus `timestamp=*`, `auto=*`, and stream oracle.

## Generic hash and dedup design

Generic hashing is in scope because resolution performs it before staging
(`cli.py:590-600`). File-importer dedup and manifest writing are later work;
the resolver accepts a lookup closure and never opens a journal directory
itself. `windowed_source_hash` is out of scope: it does not exist in the native
crate, and the existing `windowed_source_hash.rs` test only verifies that the
already-real `SourceHash::new(String)` preserves fixture-format strings.

Directory hashing reproduces the Python contract exactly for representable
paths:

- include files, hidden files, and descendants of dot-directories;
- include a symlink to a file under the link's relative path using target size;
  do not recurse a directory symlink; exclude a dangling link;
- encode newline-joined `relative-path:size` entries without a trailing newline;
- order entries by relative path components (the equivalent of Python
  `Path`-part tuple ordering), not flattened strings: `sub/a.txt` precedes
  `sub.txt`;

The component-order helper collects relative `OsString` components and sorts
their component vectors lexicographically; it must not call `to_str().unwrap()`
or sort rendered paths. Serialization unconditionally uses raw
`OsStrExt::as_bytes()` components separated by `/`, preserving non-UTF-8 bytes
rather than lossy replacement; every CI target is Unix (Linux host,
`aarch64-apple-ios`, or `aarch64-apple-darwin`; `Makefile:28-29`, `:359`,
`:372`, `:374-383`, `:953-973`), so no non-Unix fallback is added. This is the
chosen policy for the non-UTF-8 oracle; it is safer than a lossy string
conversion and keeps the byte-oriented hash input explicit. The fixture
currently gives only a digest, not the raw filename/tree used to produce it, so
the implementation test must construct and document its exact raw-byte case
before claiming parity.

The hash-tree discriminators are
`core/fixtures/import_reference_oracles.json:16-41`, `:65-130`, and `:176-205`.

## Fixture and test plan

First vendor the supplied file verbatim from
`~/import-fixtures-260811/w1b-stream-name-oracle.json` to
`core/fixtures/import_stream_name_oracle.json`. Tests use `include_str!` plus
`serde_json`; no case is transcribed into Rust source.

| AC | Test location and input |
|---|---|
| 1 | New `solstone-core-import/tests/resolution.rs`; corpus `missing_path`. |
| 2 | `resolution.rs`; constructed Apple-export-plus-top-level-PDF tree, no source / `recording` / `nosuch` / `plaud` / `ics`. |
| 2a | `resolution.rs`; counting injected Apple closure drives yes, no, and error. |
| 3 | `resolution.rs`; corpus `source=recording::{cal.ics,doc.pdf,pic.png,audio.m4a,plain.txt}` and `source=nosuch::plain.txt`. |
| 4 | Extend `solstone-core-import-sources/tests/routing_order.rs`; corpus `bare::zip_takeout_ics_AND_gemini`, `bare::zip_claude_AND_ics`, and `bare::dir_vault_3md_AND_pdf`, with reversed order. |
| 5 | `resolution.rs`; corpus `bare::dir_apple_AND_pdf`, injected Apple no versus yes. |
| 6 | `resolution.rs`; corpus `bare::zip_generic` with real no-claim predicate table. |
| 7 | `resolution.rs`; constructed Apple source tree, recursive byte snapshot before/after an injected-Apple-yes resolution, then assert `RouteAppleHealth` and identical trees. This directly proves no staging, lock, manifest, stream, or index artifact. |
| 8 | `resolution.rs`; corpus `source=oura::plain.txt`, exact remedy. |
| 9 | `resolution.rs`; constructed original and staged-lookalike files plus injected lookup records with positive and zero entry counts; verify original-path hashing and `force` suppression. |
| 10 | `resolution.rs`; constructed generic source with panic-on-call lookup and `dry_run=true`. |
| 11 | `timestamp.rs` unit tests; corpus timestamp rows, split shape and calendar variants. |
| 11a | `resolution.rs`; registry source plus impossible-day timestamp. |
| 11b | `timestamp.rs` unit test; constructed non-ASCII-digit timestamp. |
| 12 | `resolution.rs`; corpus `auto=*::dated.m4a`. |
| 12a | `resolution.rs`; constructed non-detectable source, deterministic closure returns none, model closure captures guidance for non-empty and empty variants. |
| 12b | `resolution.rs`; same non-detectable source and a call-counting model closure, asserted once. |
| 12c | `resolution.rs`; `Unavailable` model error reaches the reference no-detection refusal, while `Failed(cause)` reaches the named-remedy refusal with no raw cause text. |
| 13 | `resolution.rs`; corpus `bare::audio.m4a` and `auto=absent::audio.m4a_undetectable`; panic-on-call model closure. |
| 14 | New `solstone-core-import/tests/stream_name_oracle.rs`; vendored seven-case fixture. |
| 15 | `stream_name_oracle.rs`; constructed separator, double-dot, and invalid-name inputs. |
| 16 | `resolution.rs`; corpus boundary rows and constructed fixture trees using documented document/Obsidian predicate stand-ins. This certifies resolver dispatch and ordering given faithful predicates; source-body discrimination remains later work. |
| 17 | `resolution.rs`; constructed unclaimed directory, assert the named refusal after generic resolution; a `.pdf` directory asserts the earlier PDF refusal. |
| 18 | Implement-stage written finding, not a test: W1b libraries perform no writes. The caller-owned reference `get_journal()` auto-create is conditional; `_setup_import` is the first real write, reached at `cli.py:825` and `:865`, outside W1b. |
| 19 | After implementation only, run `make ci` through `hop check --allow-capture` and report its actual result. |

AC 7 is directly testable at the library boundary by recursive source-tree
snapshots. AC 18 is an implementation-report check: the W1b libraries create
no files at all, while caller-owned `get_journal()` auto-creation remains out of
scope.

## What a wrong port would have done

- It would have converted `--source` to a registry member before deciding Apple
  pre-emption, incorrectly pre-empting `recording`, `nosuch`, and `plaud`.
- It would have merged the three-extension detection-skip and two-extension
  text-stream sets, changing either routing or stream derivation.
- It would have treated any matched manifest as a duplicate, dropping the
  essential `entry_count > 0` condition and permanently blocking a zero-entry
  import.
- It would have allowed a registry claim-predicate error to escape instead of
  treating the real predicate's non-answer as no claim.
- It would have treated `--auto ""` as present-and-adopting rather than the
  structurally non-adopting `EmptyGuidance` state.
- It would have sorted directory hash entries by rendered relative-path string,
  producing `sub.txt` before `sub/a.txt` and the wrong digest.

## File sequence

1. Vendor the stream-name oracle unchanged and add its fixture-parsing test.
2. Add timestamp, stream-name, and source/hash value types plus their isolated
   tests; retain existing public stubs in `MODULE_STUBS`.
3. Extend the registry type/mapping while preserving ordered selection and
   extend the existing ordering test.
4. Add Apple/Oura route/refusal markers, then implement the resolver state
   machine with its injected Apple, deterministic, model, and manifest seams.
5. Add the constructed-tree resolution, dedup, error-class, and no-side-effect
   tests.
6. Run the requested Rust tests and `make ci` only during implementation's
   validation stage, not during this design stage.

## Source/corpus disagreement audit

No unrecorded contradiction was found between the supplied AC prose and the
source/corpus. The following qualifications must remain visible:

- AC 4/5 use the corpus's `native_detector_answers_no` pass. The live pass is
  not a routing oracle because the current native helper fails before the
  sweep; `routing_order.rs:30-31` records this.
- The `.pdf`-directory rule is derived from source, not measured by a corpus
  row (`cli.py:557-561`); it is deliberately retained above.
- AC 17's source-derived `import.audio` branch is not fixture-measured, and is
  deliberately replaced by the named refusal above because the Python branch
  subsequently fails in staging.
- AC 12c is the one intentional divergence: Python only swallows
  `NoBrainConfiguredError`, `ValueError`, and `JSONDecodeError`
  (`detect_created.py:282-288`); W1b turns all other model errors into a
  named-remedy refusal.
- AC 18 concerns writes, not reads. `hash_source` reads the source, and the
  Python Apple branch calls `get_journal()` before delegating
  (`cli.py:572-580`); neither is staging/lock/manifest/stream/index creation.
  The scope's parenthetical `cli.py:764` citation is stale in this checkout:
  `_setup_import` is the first real write, invoked at `cli.py:825` and `:865`.
