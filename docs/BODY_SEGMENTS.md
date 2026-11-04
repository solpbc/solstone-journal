# Body segments

How body data lands in the journal as ordinary segments. Apple Health proves this contract first, Oura follows, and Strava relates to it as described below. Parsers stay source-specific: there is no generic body framework, and every source on this contract writes these rows.

**Status:** the Apple Health writer exists and no shipped command reaches it (`core/crates/solstone-core-import-host/examples/apple_body_segments.rs` runs it). Every Apple Health and Oura owner command still saves a bundle, as [`health_imports.md`](health_imports.md) describes. The readers below are live and hold nothing until a window exists.

A body import isn't an authoritative copy of the details, which live in the source's own system. It tells the story of what the owner did and experienced.

## On disk

```text
chronicle/<day>/import.<source>/<HHMMSS>_<len>/body.jsonl
chronicle/<day>/import.<source>/<HHMMSS>_<len>/talents/facets.json   only with a chosen facet
chronicle/<day>/body.md                                              the day's document
chronicle/<day>/health/body.json                                     what body.md was built from
chronicle/<day>/health/body.updated                                  moves when body records change
imports/<id>/summary.md                                              the import's summary
imports/.<source>-zone.json                                          the zone fixed at first import
```

A **window** is one segment on a civil grid, floored to 300 seconds. Only windows holding a row exist. Every row states its own exact times, so nothing reads meaning from the key: the window length is one constant used for placement, and a source or kind could use longer windows beside 300-second tiles without changing a reader.

The zone is fixed at a source's first import, so importing again under a later owner zone writes the same coordinates. **Known limit:** a morning run on a trip can land on another civil day from the one lived. A row keeps its own offset, so its exact time stays true, and matching across streams must use instants, never window keys.

A window whose directory holds older content, live or deleted (Apple's retired day-summary cards sit at `000000_300`), takes `<HHMMSS>_<len + 1>` beside it, and an existing one wins on later imports, so new rows never turn an older segment mixed.

`format_id: body-records` in [`body-records.schema.json`](../core/crates/solstone-core/src/contract/schemas/body-records.schema.json) describes one row. Rows are compact JSON with sorted keys, sorted by time and then id; dated rows come first.

## Rows

| kind | where it sits | holds |
|---|---|---|
| `experience` | the window of its start | a workout, a night's sleep or a mindfulness session, whole: `start`, `end`, `by`, `label`, every field the source gave in `fields`, `values`, `words` |
| `experience_tile` | every later window of its span | `of`, `index`, `count`, the whole span, `label`, `by`. No name, no totals, so nothing sums across tiles, and each day reads its own tiles |
| `stage`, `event` | the window of its start | a series inside an experience, with `of` |
| `reading` | the window of `at` | one point reading: exact time, value, unit, source |
| `count` | the window of its start | an interval count, its source interval unsplit |
| `logged` | the window of its start | something the owner logged, by the shape the source gave: `start` alone or `start` and `end` |
| `dated` | `000000_86400` of its date | a daily score or summary: `date`, no time at all. A whole-day segment reads `00:00-23:59`, because a midnight window would read as something that happened at midnight |

`value` is the source's exact string. `by` is the app or device's own name. Device strings, uuids and sync ids stay in `fields`, readable by an agent and never indexed.

## What is read

One table per source, in code: `read_kind` in `solstone-core-import-sources/src/apple_health.rs` for Apple. Any other type or element is counted and named in `imports/<id>/summary.md`, and its values are never held. A record that can't be read is counted by stage, with no value in the message. A row over 32 KiB, an experience over 14 days or an `end` before its `start` is refused the same way.

## Identity and re-import

A row's identity is who recorded what and when, as instants, plus the source's own sync id when it has one. An export rendering another offset can't double a row. Rows with the same identity and different content are numbered by content, so the same data in another order is the same bytes, and exact repeats collapse. An `experience_tile`'s id is `of` + `index`.

Importing again, under one lock per source:

- **The same data** changes nothing. No window is written or published.
- **An edited record** (same identity, changed value) replaces its row, and the window republishes to search.
- **A record an export omits** stays. An export isn't a deletion.
- **A window the owner deleted** stays deleted, counted in the summary. A tombstone's manifest says what it removed. One that shows older content isn't a window the owner deleted, so new rows land beside it and it stays deleted. One that can't say counts as a deleted window, so nothing comes back on a guess.
- **A window that can't be parsed** is left as it is and counted.
- **A run that didn't finish** leaves `imports/.<source>-publishing`, and the next run publishes every window in the stream, in its export or not. The marker goes only when the attempt completed and `imports/<id>/imported.json` says `success`.

**Known limits.** Apple gives no record id, so time is identity: a record moved in time is a new row, and the old one stays. Deleting one of two same-second records in Apple Health can leave the survivor stored twice.

## Apple's nights

Apple gives sleep stages, not nights. Per source name, ordered by start, stage intervals with a gap of 30 minutes or less form one night. The row's `assembled` says so in words, and it never reads as a session Apple reported. A night's id is its source and first stage's start. Re-importing tiles the union of the stored span and the new one, so an export with fewer stages changes nothing, and a later end rewrites every tile.

**Known limits,** each needing someone to edit stages in Apple Health: a new earlier first stage makes a second night beside the first; a stage bridging two nights, or a missing middle stage, leaves two overlapping nights; and a night whose first tile was deleted has no stored span to keep.

## What is indexed

- **One entry per experience**, from the `experience` row: label, name and owner's words, times, `by`, and the `values` its reader chose. `fields` never reaches this text, so a key nobody listed can't reach search.
- **One entry per day**, `chronicle/<day>/body.md`: experiences, each kind's count and first and last time, logged kinds, the owner's words on logged rows, dated values. It's deterministic and rebuilt from the windows that survive, on the `location.md` pattern. It holds no reading values.
- **One entry per import**, `imports/<id>/summary.md`: names and counts.
- **Nothing else.** Readings, counts, stages and tiles produce no search entry.

Everything generated restates what the source recorded. Nothing is averaged, trended or judged.

## Thinking

A segment is **body-only** by what it holds, never by its stream alone: it sits in `import.apple_health` or `import.oura`, it holds `body.jsonl` or a tombstone whose manifest shows it held `body.jsonl`, and everything else in it is incidental (`stream.json`, locks, a file manager's litter, an interrupted write's candidate, `talents/` holding only `facets.json`). Any other file, or an unreadable directory, fails the test, so older content in the same stream keeps its behaviour. `solstone_core_segment::is_body_only_dir`.

A body-only segment isn't processed by Sense and owes no daily work, at any age:

- Publication emits no observed, drain or enrichment event for it, and moves `body.updated` rather than `stream.updated`, whose completion would stay open with no daily work to close it. Other writers of the stream marker still move it, so a day whose body window was deleted reads as updated in the journal health report. A deleted body window stays body-only, so one delete doesn't make a day owed. Any other tombstone, or one that can't say what it removed, keeps its behaviour, so deleting older content owes its day's daily work as it always has.
- Its coordinates carry `body_only` in `imported.json`, so an import asks no thinking or spending for them.
- `scan_day` finds no modality and skips it, as with a location-only segment.
- A day holding only such segments, or calendar notes, owes no work. Daily evidence, the schedule's active times, weekly reflection, reprocessing and both active-facet walks leave them out, so they neither hold nor change a day's daily work.

Later work opts in explicitly: code-written activity records, and a day's body document as a daily input.

## Facets

Code writes `talents/facets.json` as `[{"facet":"<name>"}]` when the owner chose a facet for the import. A muted or missing facet is refused before anything is written. With none chosen there's no file: a connection that sees the whole journal reads the window and one limited to chosen facets doesn't, and the summary says so. Importing again with no facet leaves the file alone. Another facet replaces it and queues the window, so search reclassifies it. **A window carries one facet, so its rows share it:** rows from two imports in one window are readable under whichever facet it has, and the import's summary says when that happened. `body.md` is journal-wide, so a limited connection never sees it.

## Agents

Through the MCP surface, with no new tool and no stream name in any response:

1. `search` finds an experience. `fetch` on its reference, and `get_transcript` on the segment reference that returns, read the experience with every field.
2. `list_transcripts` leaves body windows out, so hundreds of them can't push what the owner said off a page, and on a day names the sources it left out in `other_sources`.
3. `list_transcripts` with `source` (needs `day`, refuses `facet`) lists that source's windows and adds, over the whole range and on every page:
   - **`gaps`:** stretches with no window this connection can read. A gap means no window, not no reading. A whole-day segment and a tombstoned window aren't coverage. A clock-change day can show a false hour.
   - **`span_reference`:** pass it to `get_transcript`.
   - **`gaps_note`:** present when this connection can also read other content from the source that day, which these gaps don't describe.
4. `get_transcript` on a `span_reference` pages entries in time order, each attributed, across windows. A page is whole segment pages, up to 100 entries, and a call visits at most 24 windows, so a page can be short. Windows outside this connection's scope and deleted windows are skipped, and one that can't be read is counted in `unreadable`. Only the window being paged is bound to its version: one that changes before the walk reaches it reads as it now is.

`source` matches only `import.apple_health` and `import.oura` labels. Another label, or nothing readable that day, gets no `gaps` and a note saying so. A span crossing midnight is two lists. There's no kind filter yet, so one kind over a month walks every entry of that source; the walk and its cursor can take a filter and skip windows without changing the format.

## How Strava relates

Strava keeps `workout.json` and its start-anchored keys. A Strava tile is an experience in everything but the file: it tiles its real span, holds every field and renders once, on its first piece. Its thinking stays as it is for now. `scan_day` doesn't count a tile, so its Sense comes only from publication's observed event, and its day still owes daily work. Aligning it is mainly a publication change, with no re-tiling, because Strava has no readings in its stream.

## Adding a source

1. A parser and read table, source-specific, producing these rows.
2. Its stream in `BODY_RECORD_STREAMS`, a family pattern in `content/mod.rs`, and a name in `source_words`.
3. Its zone file and its run, on `solstone-core-import-host/src/body_segments.rs`.
