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
imports/<id>/body-touched.jsonl                                      what the import touched, per window
imports/<id>/body-progress.json                                      how far a run has got
imports/.<source>-zone.json                                          the zone fixed at first import
imports/.<source>-run/state.json                                     a run that hasn't finished
```

A **window** is one segment on a civil grid, floored to its length. A workout or a mindfulness session takes 300 seconds: its first row, its tiles, its events, and every reading from its own source inside its span, so deleting it takes its own series with it. Everything else with a time takes 3,600 seconds: a night and its stages, readings no session of their source covers, counts and logged entries. Only windows holding a row exist. Every row states its own exact times, so nothing reads meaning from the key: the length is used for placement and nothing else.

The zone is fixed at a source's first import, so importing again under a later owner zone writes the same coordinates. **Known limit:** a morning run on a trip can land on another civil day from the one lived. A row keeps its own offset, so its exact time stays true, and matching across streams must use instants, never window keys. A span read's order and range, and the times in `body.md`, go by this zone. An export can render every time at another offset, so a row's stated clock can differ from its window's by hours; an entry and a search result state the row's own time and offset.

A window whose directory holds older content, live or deleted (Apple's retired day-summary cards sit at `000000_300`), takes `<HHMMSS>_<len + 1>` beside it, and an existing one wins on later imports, so new rows never turn an older segment mixed.

`format_id: body-records` in [`body-records.schema.json`](../core/crates/solstone-core/src/contract/schemas/body-records.schema.json) describes one row. Rows are compact JSON with sorted keys, sorted by time and then id; dated rows come first.

## Rows

| kind | where it sits | holds |
|---|---|---|
| `experience` | the window of its start | a workout, a night's sleep or a mindfulness session, whole: `start`, `end`, `by`, `label`, every field the source gave in `fields`, `values`, `words` |
| `experience_tile` | every later window of its span | `of`, `index`, `count`, the whole span, `label`, `by`. No name, no totals, so nothing sums across tiles, and each day reads its own tiles |
| `stage`, `event` | the window of its start | a sleep stage or a workout's event, with `of` |
| `reading` | the window of `at`: its session's slot when a workout or mindfulness session from the same `by` covers it, its hour otherwise | one point reading: exact time, value, unit, source |
| `count` | the window of its start | an interval count, its source interval unsplit |
| `logged` | the window of its start | an entry or event the source recorded, by the shape it gave: `start` alone or `start` and `end`. `by` says who recorded it |
| `dated` | `000000_86400` of its date | a daily score or summary: `date`, no time at all. A whole-day segment reads `00:00-23:59`, because a midnight window would read as something that happened at midnight |

`value` is the source's exact string. `by` is the app or device's own name. Device strings, uuids and sync ids stay in `fields`, readable by an agent and never indexed. Every row also carries `run`, the import that first wrote it: set once, kept through edits, outside identity and never indexed.

## What is read

One table per source, in code: `read_kind` in `solstone-core-import-sources/src/apple_health.rs` for Apple. Any other type or element is counted and named in `imports/<id>/summary.md`, and its values are never held. A record that can't be read is counted by stage, with no value in the message. A row over 32 KiB, an experience over 14 days, an `end` before its `start` or one element with more than 10,000 children is refused the same way.

An export is read in bounded memory, so there's no count of records or rows. Its bounds are size, an `export.xml` over 64 GiB, and one XML tag over 1 MiB; either refuses the export, and the import says which. Its sorted staging is merged down in batches, so a long export can't run out of open files.

## Identity and re-import

A row's identity is who recorded what and when, as instants, plus the source's own sync id when it has one. An export rendering another offset can't double a row, and importing it changes nothing: a row stating the same instants keeps the bytes it has. Rows with the same identity and different content are numbered by content, so the same data in another order is the same bytes, and exact repeats collapse. An `experience_tile`'s id is `of` + `index`.

Importing again, under one lock per source:

- **The same data** changes nothing. No window is written or published.
- **An edited record** (same identity, changed value) replaces its row, and the window republishes to search.
- **A record an export omits** stays. An export isn't a deletion.
- **A window the owner deleted** stays deleted, counted in the summary. A tombstone's manifest says what it removed. One that shows older content isn't a window the owner deleted, so new rows land beside it and it stays deleted. One that can't say counts as a deleted window, so nothing comes back on a guess.
- **A window that can't be parsed**, bytes that aren't text included, is left as it is and counted.
- **A reading has two places on its day,** its hour and the slot of a session that covers it, and a later export can add, edit or drop that session. So a reading already saved in either place stays there, and is never in both. One the owner may have deleted doesn't come back: if it isn't saved in either place and its other place was deleted, it isn't added, because a tombstone names the files it removed, not the readings. That also holds back a reading new to the export, from any source, inside a deleted slot. When its other place can't be parsed, it isn't added either. The summary counts these beside "stayed deleted". A workout written by another app, such as Strava, has another `by`, so the watch's series during it stays on hours.
- **A run that didn't finish** leaves `imports/.<source>-run/state.json`. It records the last day done and, while a day is being written, that day's decisions, in one file replaced at once. The next run with the same export and facet carries on from there under the same import, and publishes the half-done day's windows even where their bytes already match. A run with another export first publishes that day, then starts fresh. Any run that isn't carrying on takes its own import, even for the same export under the same timestamp, so an import's record always says what its own run did.
- **A window that can't be published** is retried by the next run, with any export, and named in the summary until it is. Later days don't wait for it, and the import reads as partly failed: `imported.json` lists exactly the windows still failing. If 64 windows in a row fail with none published between them, the fault is taken to be the stream's and the run stops, to carry on when it's next started.
- **A run whose state can't be read**, or whose sorted staging is missing or short of the files the state records (as after a restore, since staging is left out of backups and the state isn't, or after power loss), publishes every window already in the stream, day by day, after bringing the stream's record level with its windows' markers.
- **What an import touched** is kept in `imports/<id>/body-touched.jsonl`, one line per day: each window it touched, with how many of its rows were new, updated and unchanged. With each row's `run`, this is what a later delete or preview can be built from.

**Known limits.** Apple gives no record id, so time is identity: a record moved in time is a new row, and the old one stays. Deleting one of two same-second records in Apple Health can leave the survivor stored twice.

## Apple's nights

Apple gives sleep stages, not nights. Per source name, ordered by start, stage intervals with a gap of 30 minutes or less form one night. The row's `assembled` says so in words, and it never reads as a session Apple reported. A night's id is its source and first stage's start. Re-importing tiles the union of the stored span and the new one, so an export with fewer stages changes nothing, and a later end rewrites every tile.

**Known limits,** each needing someone to edit stages in Apple Health: a new earlier first stage makes a second night beside the first; a stage bridging two nights, or a missing middle stage, leaves two overlapping nights; and a night whose first tile was deleted has no stored span to keep.

## What is indexed

- **One entry per experience**, from the `experience` row: label, name and owner's words, times, `by`, and the `values` its reader chose. `fields` never reaches this text, so a key nobody listed can't reach search.
- **One entry per day**, `chronicle/<day>/body.md`: experiences, each kind's count and first and last time, logged entries grouped under who recorded them, the words the source recorded on them, dated values. An entry reads as the owner's only where the source marks it as entered by the owner (Apple's `HKWasUserEntered`). Its times read on the source's fixed zone. It's deterministic and rebuilt from the windows that survive, on the `location.md` pattern. It holds no reading values.
- **One entry per import**, `imports/<id>/summary.md`: names and counts.
- **Nothing else.** Readings, counts, stages and tiles produce no search entry.

Everything generated restates what the source recorded. Nothing is averaged, trended or judged.

## Thinking

A segment is **body-only** by what it holds, never by its stream alone: it sits in `import.apple_health` or `import.oura`, it holds `body.jsonl` or a tombstone whose manifest shows it held `body.jsonl`, and everything else in it is incidental (`stream.json`, locks, a file manager's litter, an interrupted write's candidate, `talents/` holding only `facets.json`). Any other file, or an unreadable directory, fails the test, so older content in the same stream keeps its behaviour. `solstone_core_segment::is_body_only_dir`.

A body-only segment isn't processed by Sense and owes no daily work, at any age:

- Publication emits no observed, drain or enrichment event for it, and moves `body.updated` rather than `stream.updated`, whose completion would stay open with no daily work to close it. Other writers of the stream marker still move it, so a day whose body window was deleted reads as updated in the journal health report. A deleted body window stays body-only, so one delete doesn't make a day owed. Any other tombstone, or one that can't say what it removed, keeps its behaviour, so deleting older content owes its day's daily work as it always has.
- An import publishes its windows a day at a time and lists none of them in `imported.json` (it lists only those still failing), so an import asks no thinking or spending for them, and discovery never reads a record the size of a whole history.
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
4. `get_transcript` on a `span_reference` pages the span's entries, each attributed, in time order by each row's instant, across windows of every length, up to 100 a page. An entry states its time as its row does, offset included. A range reads on the source's fixed zone, as a window's `time` does. A call serves from at most 24 windows, so a page can be short; under a kind, a window without it doesn't count. Only what belongs to the span comes back: a timed entry inside it, an interval or experience overlapping it, and a dated entry always. Windows outside this connection's scope and deleted windows are skipped, and one that can't be read is counted in `unreadable`. The cursor is a place in that order, so a window that changes under it reads as it now is from there on.

`source` matches only `import.apple_health` and `import.oura` labels. Another label, or nothing readable that day, gets no `gaps` and a note saying so. A span crossing midnight is two lists.

`kind` (needs `source`) narrows both to one kind of entry, by the name its entry gives it: a reading's or count's name such as `resting heart rate`, or `workout`, `sleep` or `mindfulness`, case aside. The list holds only windows with that kind in the range, and its `span_reference` returns only those entries, so a sparse kind such as `resting heart rate` over a month is about one list and one read a day. `gaps` still describe the source, not the kind. When the source holds entries in the range but none of that kind, the empty list says so, apart from a range with nothing readable at all.

## How Strava relates

Strava keeps `workout.json` and its start-anchored keys. A Strava tile is an experience in everything but the file: it tiles its real span, holds every field and renders once, on its first piece. Its thinking stays as it is for now. `scan_day` doesn't count a tile, so its Sense comes only from publication's observed event, and its day still owes daily work. Aligning it is mainly a publication change, with no re-tiling, because Strava has no readings in its stream.

## Adding a source

1. A parser and read table, source-specific, producing these rows.
2. Its stream in `BODY_RECORD_STREAMS`, a family pattern in `content/mod.rs`, and a name in `source_words`.
3. Its zone file and its run, on `solstone-core-import-host/src/body_segments.rs`.
