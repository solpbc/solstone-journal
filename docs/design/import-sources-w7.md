# W7 Native Import Sources Design

## Purpose and boundary

W7 ports the read-only source behavior for Claude, ChatGPT, Gemini, and Kindle
from their retained Python importers into `solstone-core-import-sources`:

- content-aware `detect` predicates;
- owner-facing `preview` results;
- pure parsing and import planning; and
- source-specific malformed-entry accounting.

The wave does not parse argv, stage a source, acquire a lock, write a segment
or content manifest, seed an entity, publish/index a result, emit progress, or
otherwise mutate either an owner source or journal state. The retained Python
verb is the boundary reference: the dry-run branch calls `preview` and returns
at `solstone/think/importers/cli.py:698-730`; the real file-importer call
shape is `solstone/think/importers/cli.py:465-490`.

The one vendored oracle proposed by this design is
`core/fixtures/import_sources_w7_oracle.json`. Its per-source wrapper holds a
fixture-owned `status` and a `captured` payload containing the original source
row verbatim; the fixture also carries provenance. This is the only way to
honor both requirements at once: adding a status field directly to a captured
row means that row is no longer byte-identical. The `captured` payload is the
unchanged oracle data, while status is fixture metadata. It is the sole truth
source for resulting oracle assertions; test code and this document do not keep
a parallel hard-coded list of superseded or non-expectation rows.

## Acceptance criteria (verbatim)

1. `[test]` Each source's `detect` reproduces the oracle, **including** that a chat-shaped archive is claimed by exactly one of the two chat sources — assert both directions (claude's predicate says no to a ChatGPT archive and vice versa), so a filename-only predicate reds.
2. `[test]` Each preview reproduces the oracle's counts and ranges under the §5.1 unit decision, with the unit named in the summary string.
3. `[check]` The `item_count` unit decision is applied to **all four** sources and recorded in the design doc / outcome, including which oracle rows it supersedes and why.
4. `[test]` A clippings file resolves to generic text without `--source` and to `kindle` with `--source kindle`, driven through `resolve_import`. ⛔ Must fail against an implementation that removed `txt` from `DETECTION_SKIP_EXTENSIONS`.
5. `[test]` A richer Takeout fixture pins gemini's parse; the design doc says what it pins. ⛔ The oracle's zero-item row is not used as an expectation.
6. `[test]` Conversation entries carry their timestamps and roles, and are grouped into the days the oracle records.
7. `[test]` Affected days are supplied to publication as data, not derived from path position.
8. `[test]` The owner's export archives are byte-identical after import — assert through `observe_source_immutability` with the real source behavior running inside, not a stub.
9. `[test]` A malformed entry inside an otherwise-valid export is **skipped and reported** — not fatal, and not silently dropped. Assert the report is non-empty and names the skipped entry; ⛔ a test that only asserts "did not panic" reproduces the silent-drop shape.
10. `[check]` `make ci` reported honestly — fail-fast means targets after a failure did not run and must not be claimed as passing; `--locked` and the host-scoped `BINDGEN_EXTRA_CLANG_ARGS` notes apply.

### How each criterion is discharged

| AC | Concrete assertion and test file |
|---|---|
| 1 | New source detector tests under `core/crates/solstone-core-import-sources/tests/` construct the two minimal `conversations.json` archives and assert both Claude/ChatGPT directions, extensions, and source-specific Gemini/Kindle predicates. |
| 2 | `preview_oracle.rs` reads `core/fixtures/import_sources_w7_oracle.json`, honors the wrapper status, and compares expectation-row preview counts/ranges plus exact unit-naming summaries. |
| 3 | Design doc and implementation outcome record atomic units for all four sources and identify the superseded ChatGPT and non-expectation Gemini rows; AC 2 supplies the corresponding summary-string assertion. |
| 4 | `core/crates/solstone-core-import/tests/resolution.rs` uses a genuine clippings input and drives the assertion through `resolve_import`: no source is `GenericText`, explicit Kindle selects Kindle. Because it uses the resolver's real skip-extension gate, deleting `txt` from `DETECTION_SKIP_EXTENSIONS` runs the claims sweep and changes the no-source result, so the test reds. |
| 5 | `plan_contract.rs` uses the richer `Takeout/My Activity/Gemini Apps/MyActivity.json` fixture to assert prompt/response, timestamp, and HTML branches. `preview_oracle.rs` reads the row status and never treats the zero-item Gemini capture as an expectation. |
| 6 | `plan_utc.rs` uses midday UTC conversation fixtures and asserts timestamped Human/Assistant entries, their UTC day grouping, and `HHMMSS_300` keys against the recorded days. |
| 7 | `plan_contract.rs` asserts sorted, deduplicated explicit `affected_days` on `ImportPlan`. Publication is deferred to the staging wave; `ImportPlan.affected_days` is the named hand-off, and W7 discharges only the data-carrying half because it writes no output paths. |
| 8 | `source_immutability.rs` runs actual detect, preview, and plan for all four constructed inputs *inside* `observe_source_immutability`, then asserts the report is not violated. |
| 9 | `plan_contract.rs` embeds malformed candidates in otherwise-valid inputs and asserts a non-empty skipped report containing the source locator and closed reason; it inspects report contents rather than merely asserting no panic. Clean fixtures separately assert `skipped.is_empty()`. |
| 10 | Implementation-stage outcome reports the actual `make ci` execution honestly: with fail-fast, unrun later targets are recorded as unrun, not passing; it also records the applicable `--locked` and host-scoped `BINDGEN_EXTRA_CLANG_ARGS` conditions. |

The stub-table bookkeeping is a deliberate-decision guard rather than a
separate acceptance criterion: `stub_table.rs` asserts eight remaining unique
stubs and explicit exclusion of the four completed modules.

Not acceptance criteria:

- Staging, segment/manifest publication, entity seeding, import-directory
  locks, index publication, and any `ImportResult.files_created` value. Those
  operations belong to later write-owning waves.
- A `dry_run` parameter. The audited verb never passes it to a file importer
  (`cli.py:465-490`), and dry-run has already returned from preview
  (`cli.py:698-730`); adding it would preserve an unreachable branch.
- A progress callback. It is deferred, not discarded. The in-memory plan is
  not the long-running operation; the later write loop in
  `solstone-core-import/src/staging.rs` is its landing point. The reference
  calls Claude and ChatGPT once at `(len(conversations), len(conversations))`
  (`claude_chat.py:205-212`, `chatgpt.py:243-250`), and Gemini and Kindle every
  100 original items (`gemini.py:282-289`, `kindle.py:294-301`). The latter two
  omit a guaranteed final call, a reference defect that the staging wave must
  not reproduce.
- Unifying `entity_count`. It remains source-specific: zero for the three chat
  sources, and Kindle's distinct books plus distinct authors
  (`kindle.py:208-219,242-247`). Therefore one book by one author legitimately
  previews as one book and `entity_count == 2`. Normalizing entity semantics
  would require a broader entity/import contract decision.
- Retiring `registry::reserved_seam` at
  `core/crates/solstone-core-import-sources/src/registry.rs:20-22`. It has the
  same stale-seam shape retired elsewhere, but registry completion is not W7.

## Deliberate decisions

1. **`item_count` means an atomic written entry.** For Claude, ChatGPT, and
   Gemini it is a chat message; for Kindle it is a parsed clipping/highlight.
   It predicts `ImportResult.entries_written`, matches an owner's sense of how
   much will be imported, and avoids reporting a 5,000-highlight Kindle import
   as 30 items because it came from 30 books.

   The exact preview summaries for the current synthetic fixture are:

   | Source | W7 summary |
   |---|---|
   | Claude | `2 messages from Claude chat export` |
   | ChatGPT | `2 messages from ChatGPT export` |
   | Gemini | `4 messages from Gemini export` |
   | Kindle | `1 highlights from 1 books` |

   In general the first three sources use `N messages ...`; Gemini's atomic
   unit is a prompt or response message an activity yields (one or two), never
   the activity container. Kindle retains its type-count wording (for example,
   `N highlights ...` or `N notes ...`), which names its atomic clipping kind.
   `core/fixtures/import_sources_w7_oracle.json` owns capture status and W7
   expectations; this document does not duplicate those per-row classifications.

2. **The public source surface is fallible and read-only.**
   `detect(path) -> Result<bool, SourceError>` names source I/O and corrupt
   archive failures rather than concealing them in a boolean. The
   resolver-facing adapter maps `Err` to a non-claim: Python's registry sweep
   treats a raising predicate as a non-answer
   (`solstone/think/importers/file_importer.py:128-136`), and native resolution
   already does so with `unwrap_or(false)`
   (`core/crates/solstone-core-import/src/detect.rs:243-250`).

   `preview(path) -> Result<ImportPreview, SourceError>` is exactly the
   oracle seam because `_preview_file_importer` calls `importer.preview(path)`
   and nothing else for these sources (`cli.py:451-462`). Each source also
   exposes `plan(path) -> Result<ImportPlan, SourceError>`. No public operation
   includes `dry_run` or a progress callback.

3. **UTC is the sole timestamp rule.** Every raw timestamp is converted to UTC
   before deriving preview days, affected days, 300-second window boundaries,
   and `HHMMSS_300` keys. There is no timezone parameter because this crate has
   no owner-identity access. `docs/PORTING.md:241-247` identifies UTC as the
   sanctioned fallback and warns that reproducing host-local behavior in Rust
   diverges from the Python owner-timezone resolution.

   This intentionally diverges from the references:

   - Claude preview formats the parsed ISO value as written
     (`claude_chat.py:130-149`), whereas process later turns it through epoch
     time (`claude_chat.py:49-74`) and UTC date generation
     (`claude_chat.py:198-203`).
   - ChatGPT preview uses naive-local `fromtimestamp`
     (`chatgpt.py:163-182`); process date range uses explicit UTC
     (`chatgpt.py:236-241`), while shared message windows use local time
     (`shared.py:234-270`).
   - Gemini parses ISO timestamps to epoch (`gemini.py:110-119`), then preview
     uses naive-local `fromtimestamp` (`gemini.py:194-212`); process date range
     is UTC (`gemini.py:271-306`) and its shared message windows are local.
   - Kindle parses a naive local wall-clock date (`kindle.py:49-57`), makes a
     host-local epoch (`kindle.py:282-284`), and windows with `tz=None`
     (`kindle.py:322`); preview only formats the naive value
     (`kindle.py:229-246`).

   ChatGPT and Gemini oracle captures were host-timezone-sensitive; Claude and
   Kindle preview captures were not. Because the oracle does not retain raw
   timestamps, W7 fixtures must use UTC/no-offset timestamps at a midday value
   far from any plausible day boundary. This is mandatory fixture construction,
   not a test-host environment assumption. If later owner-timezone windowing is
   required, it belongs at the staging/publication boundary after
   `solstone-core-import/src/staging.rs` has explicit owner identity/config
   access; it is not a speculative parameter on this parser surface.

4. **Malformed entries produce one closed, per-item report.** `ImportPlan`
   contains `skipped: Vec<SkippedEntry>`, where `SkippedEntry` has a source
   locator (`conversation_index`, `message_index`, `activity_index`, or
   `clipping_block_index`) and `SkipReason`. The shared closed enum covers all
   reference drop conditions:

   | Reason | Reference condition |
   |---|---|
   | `EmptyConversation` | Claude has no `chat_messages` (`claude_chat.py:43-47`). |
   | `EmptyMessageText` | Claude text is falsy (`claude_chat.py:58-62`). |
   | `NoUsableTimestamp` | Claude has neither a valid message timestamp nor valid conversation fallback (`claude_chat.py:49-74`). |
   | `NoImportableConversationContent` | Claude conversation yielded no accepted message (`claude_chat.py:88-91`). |
   | `MissingConversationMapping` | ChatGPT mapping is falsy (`chatgpt.py:67-71`). |
   | `InvalidConversationPath` | ChatGPT lacks a usable `current_node` path (`chatgpt.py:34-56`). |
   | `UnsupportedMessageRole` | ChatGPT role is neither user nor assistant (`chatgpt.py:76-80`). |
   | `EmptyMessageContent` | ChatGPT has no joined string content (`chatgpt.py:82-87`). |
   | `InvalidMessageTimestamp` | ChatGPT timestamp is absent or nonnumeric (`chatgpt.py:89-91`). |
   | `NoImportableConversationContent` | ChatGPT conversation yielded no accepted message (`chatgpt.py:113-115`). |
   | `NoActivityContent` | Gemini has neither usable prompt nor usable response (`gemini.py:87-108`). |
   | `MissingActivityTimestamp` | Gemini `time` is absent/falsy (`gemini.py:110-113`). |
   | `InvalidActivityTimestamp` | Gemini ISO time parsing fails (`gemini.py:114-119`). |
   | `InsufficientClippingLines` | Kindle block has fewer than two lines (`kindle.py:65-67`). |
   | `EmptyClippingTitle` | Kindle title is empty after BOM removal (`kindle.py:69-74`). |
   | `InvalidClippingDate` | Kindle date is missing or does not match supported formats (`kindle.py:99-108`). |
   | `EmptyBookmark` | Kindle bookmark lacks content (`kindle.py:117-119`). |

   A blank or whitespace-only Kindle block is not a candidate entry and produces
   no `SkippedEntry`, matching the reference's early `continue`
   (`kindle.py:272-274`). Only a non-blank block that fails to parse is reported;
   this keeps ordinary delimiters and inter-block whitespace out of diagnostics.
   A clean fixture with no malformed candidate entries must return
   `skipped.is_empty()`.

   `NoImportableConversationContent` is emitted once for a container only when
   it has no accepted atomic entry *and* no more-specific container reason
   applies; it does not replace or duplicate entry-level reasons. This gives a
   malformed conversation one causal report rather than an invalid-path plus
   generic-empty pair.
   This is a deliberate divergence: Claude, ChatGPT, and Gemini merely count
   skips for logging (`claude_chat.py:239-240`, `chatgpt.py:277-278`,
   `gemini.py:369-370`), whereas Kindle alone puts selected block failures in
   `ImportResult.errors` (`kindle.py:275-280`). The report surfaces on the plan,
   not `ImportPreview`. That is acceptable because preview is deliberately the
   small owner-facing count/date/summary contract (`file_importer.py:14-21`),
   while detailed diagnostics require parsing every candidate item and belong to
   the later plan/result presentation.

5. **Completed modules leave the stub inventory.** Following `df22182b6`, W7
   deletes the four source `reserved_seam` functions and removes their four rows
   from `MODULE_STUBS` (12 to 8), then widens the companion
   `implemented_*_modules_have_no_unimplemented_seam` assertion in
   `tests/stub_table.rs`. Decreasing the count alone would allow later drift;
   the implemented list keeps an accidental reintroduction red.

6. **One vendored fixture owns status as data.**
   `core/fixtures/import_sources_w7_oracle.json` wraps each captured row as
   `{ "status": ..., "captured": ... }`, with the original payload preserved
   under `captured` and provenance beside the case table. The status vocabulary
   distinguishes at least `expectation`, `superseded`, and `non_expectation`:
   `expectation` is asserted byte-for-byte at the preview seam; `superseded`
   documents a deliberately replaced result; and `non_expectation` supplies
   detection/provenance evidence but cannot satisfy a preview assertion. This
   avoids two truths between a fixture and a Rust exclusion list. The wrapper is
   a necessary qualification to the otherwise incompatible “byte-identical row”
   and “per-row status field” requirements; the captured payload, not the
   wrapper, is byte-identical. The vendoring posture follows
   `core/fixtures/import_stream_name_oracle.json` (introduced by `61f8a3469`).

## Public library surface

All source and test files receive the repository SPDX header. `ImportPreview`
and `ImportResult` remain owned by `solstone-core-import`'s existing contract
(`core/crates/solstone-core-import/src/contract.rs:10-64`).

| Module | Public surface |
|---|---|
| `claude.rs`, `chatgpt.rs`, `gemini.rs`, `kindle.rs` | `detect(path: &Path) -> Result<bool, SourceError>`, `preview(path: &Path) -> Result<ImportPreview, SourceError>`, and `plan(path: &Path) -> Result<ImportPlan, SourceError>`. Each module implements source parsing but returns the shared types below. |
| shared source module (new) | `ImportPlan`, `PlannedSegment`, `PlannedEntry`, `SkippedEntry`, `SkipLocator`, `SkipReason`, and `SourceError`. This prevents four near-identical plan models. |
| resolver adapter | A source-to-`ResolutionSeams.claims` adapter that maps `detect` success to its boolean and every `SourceError` to false. It does not alter `resolve_import`'s ordered sweep. |
| `ImportPlan` | `segments: Vec<PlannedSegment>`, `affected_days: Vec<String>`, `item_count: u64`, `date_range: (String, String)`, and `skipped: Vec<SkippedEntry>`. Empty plans use `("", "")`, matching preview convention. |
| `PlannedSegment` | UTC `day`, UTC `segment_key`, optional first model slug, and `entries`. |
| `PlannedEntry` | Exactly `{ start, speaker, text }`. `_window_messages` builds this shape directly (`shared.py:220-272`). `window_items` is the second reference window producer (`shared.py:275-335`): it groups Kindle's parsed clipping records unchanged. W7 retains that two-view split and projects the same three-field public entry from each windowed clipping; it adds no payload enum, source discriminant, or field. |
| `SourceError` | Named source-boundary failures: `Io`, `UnsupportedPathKind`, `UnsupportedExtension`, `ArchiveOpen`, `ArchiveMemberMissing`, `ArchiveMemberRead`, `InvalidJson`, `InvalidJsonShape`, and `TextDecode`, each carrying the affected path and applicable member/operation context. Malformed individual entries are `SkippedEntry`, not operation-fatal errors. |

The source crate gains `zip = { version = "=2.4.2", default-features = false,
features = ["deflate"] }`, matching
`solstone-core-body-ingest/Cargo.toml:29` and
`solstone-core-convey-shell/Cargo.toml:51` exactly. It uses workspace
`serde_json` for archive/JSON values and `chrono` for ISO parsing and UTC
derivation (`core/Cargo.toml:172,193`). `regex` is not required by the chat
sources; Kindle may use it only if its Python parsing expressions are ported as
regexes rather than equivalent explicit parsing.

## Parse and planning model

Plans reproduce the reference's 300-second grouping rule without writes. Input
entries are sorted by timestamp; a new window starts on first entry, a UTC day
change, or elapsed time `>= 300`; its key is UTC `HHMMSS_300`; entry offsets
are formatted `HH:MM:SS`; and the first non-null model slug becomes the window
model (`solstone/think/importers/shared.py:220-272`). Affected days are the
deduplicated, sorted segment days, not an implicit consequence a later writer
must recalculate.

`ImportPlan` stops at its five declared fields. It does not carry
conversation/activity-to-segment bindings, book-to-segment grouping, content
manifest rows, Kindle entity definitions, or any other manifest-ready data:
`affected_days` is the complete W7 day-impact surface. Content-manifest binding
and all publication decisions belong to staging rather than this speculative
parser contract. Actual `write_segment`, `write_markdown_segments`, and
`write_content_manifest` calls remain later work
(`solstone/think/importers/shared.py:187-217,338-375,834-854`), as does Kindle
entity seeding (`kindle.py:393-413`).

Where a source needs parsed records beyond the shared plan projection, it may
use a module-local typed parser consumed by that module's `plan` function. Kindle
is the present case: one clipping parser supplies both the public window-entry
projection and its own rendering-ready clipping records, avoiding a second parse
path. No module-local parser is added merely for symmetry.

## Fixture and test plan

The fixtures are constructed, vendored evidence; no test invokes retained
Python. The richer Gemini Takeout archive uses the exact member path
`Takeout/My Activity/Gemini Apps/MyActivity.json`. Its records cover: prompt
only; response only; prompt and response; subtitle `value`; subtitle `name`
fallback; HTML that strips to empty; no content; missing time; invalid time;
and Bard/Gemini-era `products` and `header` fields. This pins `_parse_activity`'s
prompt/response, timestamp, and HTML branches (`solstone/think/importers/gemini.py:85-141`).
The era fields remain for Takeout fixture fidelity only; W7 does not classify
or surface era. The vendored zero-item Gemini row is never a preview expectation.

The Claude/ChatGPT discrimination fixtures contain only the required ZIP member
`conversations.json`: `[ {"chat_messages": []} ]` must claim Claude and reject
ChatGPT, while `[ {"mapping": {}} ]` must claim ChatGPT and reject Claude. The
extension variants prove that a `.dms` uses Claude's ZIP-content predicate and
that ChatGPT does not accept it. This remains a content predicate test, never a
test of the display-only `file_patterns` metadata.

## Implementation sequence

1. Vendor the status-bearing oracle and construct the focused archive/text
   fixtures, including the midday UTC timestamp rule and rich Gemini records.
2. Add the direct `zip`, `serde_json`, and `chrono` dependencies and shared
   source error/plan types; no staging dependency is introduced.
3. Implement Claude and ChatGPT archive readers/detectors together, using their
   mutual-discrimination fixtures before their previews/plans.
4. Implement Gemini's ZIP/JSON/directory predicates and activity parsing, then
   Kindle's clipping detection/parser and shared UTC planner projection.
5. Add the fallible detector adapter in `solstone-core-import-sources`, then add
   the genuine Kindle `plain.txt` resolver test in
   `core/crates/solstone-core-import/tests/resolution.rs`. This does **not**
   modify `core/crates/solstone-core-import/src/detect.rs`:
   `ORDERED_FILE_IMPORTER_NAMES`, `DETECTION_SKIP_EXTENSIONS`, and
   `resolve_import`'s body remain unchanged.
6. Remove completed source stubs and strengthen the stub-table guard.
7. Add the real, all-operation source-immutability test and complete the
   focused conformance suite before the requested settled gate.

## Follow-up outcomes

- The staging wave owns publication from `ImportPlan`: segment and manifest
  writes, Kindle markdown rendering, entity-seeding authorization, and the
  explicit progress callback. Its callback must report a terminal final status
  even when fewer than 100 items were processed. Kindle markdown is rendered
  from the module's clipping records, not from `PlannedEntry`, matching the
  reference's parsed-record/windowed-entry two-view split.

## Risks and source/corpus disagreement audit

- The raw oracle fixture timestamps are unavailable. ChatGPT and Gemini were
  host-timezone-sensitive at capture, so their recorded dates are unsafe to
  reproduce with arbitrary epochs. The required midday UTC fixture discipline
  makes W7's replacement pin deterministic, but it cannot prove the original
  capture's exact timestamp.
- The Gemini zero row proves archive-path detection only. It does not exercise
  `_parse_activity`, and treating it as a successful parse oracle would lock in
  absence of behavior rather than source parity.
- Atomic `item_count` intentionally changes the ChatGPT preview row. It must
  not be described as byte-for-byte parity with the old oracle; its status must
  be data-marked `superseded`.
- Python's shared message windows are host-local, while process date ranges are
  partly UTC. The one-rule UTC decision is therefore an intentional behavior
  change, not a mechanical port.
- Several Python detectors/parser helpers can throw on unexpected JSON element
  types rather than return false or skip. W7 must classify source-level invalid
  JSON shape as `SourceError::InvalidJsonShape`; silently broadening claims
  would hide corrupt inputs, while panicking violates the error-mapping rule in
  `docs/PORTING.md:262-279`.
- Python Kindle's `journal_root` parameter is ignored by its segment writer,
  which instead reaches global journal resolution (`kindle.py:322-328` and
  `shared.py:367-375`). The pure W7 plan avoids inheriting that write-path
  defect.
- Kindle has two legitimate views: its clipping records carry the fields needed
  by `_render_highlight_markdown` (`kindle.py:137-175`), while the public plan
  exposes only the three-field segment-entry projection. This is the reference
  pattern, not a W7 payload expansion: `window_items` groups parsed records
  unchanged (`shared.py:275-335`) and the renderer consumes those records rather
  than window entries (`kindle.py:322-328`).
