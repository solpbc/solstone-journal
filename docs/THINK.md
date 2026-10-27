# Think

Home-side processing after capture. The process is `solstone journal think`. There is
no Python think package and `make install` does not install one.

## Commands

| Command | Purpose |
|---------|---------|
| `solstone journal think` | Run generators and talents for a day via Cortex |
| `solstone journal supervisor` | Start the local services that feed Convey, Cortex, and background work |
| `solstone journal cortex` | Talent orchestrator |
| `solstone journal talent list` / `solstone journal talent show <name>` | List or inspect talent configs; `--prompt` prints the composed prompt |
| `solstone journal talent passages <name>` | Copy exact passages from an activity's source units ([`SOURCE_PASSAGES.md`](SOURCE_PASSAGES.md)) |
| `solstone journal brain status` / `solstone journal brain refresh` | Active-brain status and one bounded check |
| `solstone journal indexer` | Rebuild the search index |
| `solstone call transcripts read` | Read audio and screen transcripts for a day |

```bash
solstone journal think [--day YYYYMMDD] [--segment HHMMSS_LEN] [--stream NAME] [--refresh] [--flush]
solstone journal supervisor [--no-daily] [--no-cortex] [--no-spl] [--no-convey] [--no-schedule]
solstone journal talent list [--schedule daily|segment] [--json]
solstone journal talent show <name> [--prompt] [--day YYYYMMDD] [--segment HHMMSS_LEN] [--full]
solstone journal talent passages <name> --day YYYYMMDD --facet NAME --activity ID [--json]
```

`--refresh` overwrites existing generator output.

## Talent inputs

Two principles decide what a talent reads. New talents and new segment sources
follow them.

**Segment talents sample to fit.** When a segment source can hold more than a
segment talent's context window, the journal samples it rather than clipping it:
it chooses what to read and how deeply, so the most important parts of the
segment are always indexed. Describe is the model. It categorizes every frame,
then gives detailed extraction to a bounded selection of frames in the
categories worth extracting ([SCREEN_CATEGORIES.md](SCREEN_CATEGORIES.md) § How
It Works, [OBSERVE.md](OBSERVE.md) § Vision input sizing). Sampling is enough
because an agent that finds a segment through what was indexed can still open
its original media where that is kept, and what matters in a segment usually
recurs often enough to be picked up. Speech transcripts are exempt: a segment's
transcript is small and spoken words always matter, so it is read whole.
Terminal, browsing and every new source need a sampling step, designed with the
source. Fitting that trims an input is a last-resort guard that records how
much it left out as `partial_input`. It is never the design.

**Later talents build on segment results.** Activity, daily and cadence talents
read what segment talents produced, plus excerpts and quotes that code copies
verbatim from the sources, rather than reading raw segment data again. Small
segment data can go in directly, such as segment metadata, locations and short
transcripts. Work's literal screen notes are the pattern for copied excerpts
([STORY_INPUTS.md](STORY_INPUTS.md)).

Inputs not yet aligned include [Terminal segment Sense](#terminal-segment-sense),
which trims its timeline rather than sampling it, and the Work, Participation
and Conversation talents, which read original activity input (Work only while
it fits, [STORY_INPUTS.md](STORY_INPUTS.md)). The Generate boundary
does the fitting ([GENERATE.md](GENERATE.md) § `overflow_text`).

## Daily date coordinates

`--day` selects the analysis day. Daily artifacts, including `morning_briefing.json`,
remain under that day. A morning briefing presents the following local calendar
morning: its sources use the analysis day, and it does not forecast the
presentation day or anything after it. Home reads exactly the preceding analysis day's briefing and links that generating run.
Late execution and retries do not change either coordinate.

Terminal work identity includes the source day from its event payload. The local
oplog partition records execution time; it does not replace an activity's source
day when completion crosses midnight.

### Daily completion and recovery

Daily completion records the evidence revision and effective talent contract that
each analysis consumed. New source text, derived evidence, or a relevant contract
change makes earlier results historical. A contract-only change re-owes today
and the seven preceding closed days; older accepted results stay kept unless
their evidence changes or the owner requests `--from-scratch`. A morning
briefing stays frozen from noon on the morning it is presented. Coverage and
execution use the same `accepted_reuse` decision. The raw-input marker still
guards whole-day publication, but cannot prove that a daily analysis consumed new
evidence.

The daily unit record in `chronicle/<day>/health/daily-units/` retains the prepared
input, model result, owner actions, and publication receipts. A retry resumes that
plan under the unit's publication lock. A replaced worker cannot publish with its
old attempt token. Completed domain actions remain historical receipts after an
owner edits or deletes their results. If an interrupted action cannot be distinguished
from an owner change, recovery reports a conflict and preserves the current files.
Missing or changed required output files make coverage outstanding without replaying
already committed domain actions.

On ios, a day rebuild can continue after process loss once its segment and
activity work has completed and its input and receipts are verified. The request
retains its applicable daily units and their reset identities. Continuation keeps
accepted results, resumes retained publication work, and starts untouched units
even if they were current before the rebuild. Work lost before that boundary,
uncertain model outcomes and older claims without this provenance remain held
for owner review. Explicit Stop persists a hold before acknowledgement.

Publication also checks the admitted facet identity and entity attachment. Daily
write admission assigns missing facet IDs through the facet owner before capturing
evidence; read-only coverage checks do not migrate declarations.

First adoption covers the latest seven closed local calendar days. New days,
known dirty older days, and explicitly requested days also become eligible. Bounded
reconciliation finds later evidence changes on adopted days; untouched older history
remains unverified. Coverage distinguishes current, current-degraded, outstanding,
historical-unverified, and unreadable results. Unsupported daily hook contracts stay
unverified. Current bounded analyses retain their selection and exclusion information;
they do not assert that every entity was processed.

`daily_schedule` has one global record at `health/maintenance/daily_schedule.json`,
shared across historical catchups for the current local window. Its status is reported
separately from historical daily coverage. Failure caps apply to the current evidence
and contract. Capped environmental failures receive one persisted, bounded revalidation
per local day; owner-write conflicts do not become successful capped results.

## Possible names

After Sense, every segment with a speech transcript (`audio.jsonl`, or its only
speech transcript) runs `entities:possible_names`. Its pre-step finds
facet-attached registry entities whose name, alias or close spelling matches a
span of the speech (first-name and sound matches are found but, as evaluated, not
offered; an alias only for an entity filed under one of the segment's facets),
keeps at most twelve, and asks the
owner's active brain at temperature 0, with the prompt as the system instruction
and the packet as the only text, to answer `yes` or `no` for each against a schema
built for that request. Accepted spans are written to
`talents/possible_names.json` by `solstone-core-possible-names` and queued for
search; readers show an entry only while it still holds. Retrieval thresholds and
route switches are constants in
`solstone-core-talent-runtime/src/possible_names/policy.rs`.

A sidecar with the same transcript digest, registry revision and talent version
is fresh, and Generate is skipped. The registry revision also covers the segment's
own facet list, so a re-filed segment regenerates. No candidates skips Generate too and replaces
an older list with an empty one. A malformed answer fails the talent and writes
nothing. The talent never gates publication (`SEGMENT_NONGATING_TALENTS`).
`solstone-core __possible-names-packet <segment-dir>` prints, for evaluation, the
exact prompt, packet, schema and candidates a run would send, without calling
Generate or writing anything.

## Terminal segment Sense

Sense encodes complete terminal-only segments as a reversible state timeline. Pane text, styles, links, identities, observation times and retained analysis remain reconstructible. Segments with other inputs keep the ordinary native request. Eligibility depends only on the segment's inputs, never on the provider, model, endpoint or context window.

Every terminal-only segment reads the timeline. Without typed input, the timeline goes to Sense's own task and schema with its reading guide as the system instruction, and the record publishes like any Sense output.

Typed input is read from the terminal capture's cursor, not from any application's screen layout. An active pane with a visible cursor, outside a tmux pane mode, takes input on the cursor's row; dim text after the cursor is a placeholder or suggestion and is left out. A row counts as typed once its text changes under the cursor, so an unchanged prompt does not, and when the cursor leaves a row, what that row now shows is taken as what was sent. Frames captured without cursor fields have no typed input. When a segment has typed input, the request is also anchored to the latest: the model chooses complete displayed passages for activity context. Rust copies those passages and the typed input into the existing Sense activity fields, then validates the canonical Sense object. Displayed text is framed as what was on screen; it does not establish authorship, approval or execution. Other fields remain generated assessments.

The timeline takes the ordinary input position, so fitting trims it like any talent's input, keeping the newest whole entries. Every pane and window roster restarts from complete text at the same observation at least every 60 seconds, and each restart begins an entry, so a trimmed timeline starts on complete screens. Within each displayed block, a line already shown since the latest entry is dropped, and each run of dropped lines becomes one `[N earlier lines]` marker; the lossless timeline is checked before that drop. The reading guide, the input cards, the passages and the Sense instructions are preserved, so every passage the model may choose stays visible. A trimmed result records its input budget like any other partial input. This trimming is the last-resort guard, not sampling: a segment longer than the window loses its earliest entries, whatever they held. Choosing entries for importance is the open direction ([Talent inputs](#talent-inputs)).

The talent runtime validates the Sense object (bound, when anchored) before its existing output writer runs. Think then publishes the usual segment projections through its existing writer. An invalid binding or record, or a context-window refusal because even the preserved part does not fit, permits one ordinary native fallback sequence. Cancellation, transport, authentication and attestation failures do not trigger that fallback. Attempt events and usage accounting retain the candidate and fallback calls.

## Architecture

```
solstone journal sense (observe) → chronicle/YYYYMMDD/{stream}/HHMMSS_LEN/
       ↓
solstone journal think
   ├── solstone-core-indexer
   ├── solstone-core-thinking (generators)
   └── solstone-core-cortex → solstone-core __talent-worker
```

Owners:

| Concern | Crate |
|---------|--------|
| `solstone journal think` CLI | `solstone-core-think-cli` |
| generator scheduling and `load` | `solstone-core-thinking` |
| Cortex service | `solstone-core-cortex` |
| talent worker | `solstone-core-talent-runtime` |
| talent configs | `core/payload/solstone/talent/*.md` |
| import | `solstone-core-import` / `solstone-core-import-sources` |
| indexer | `solstone-core-indexer` |
| providers | [PROVIDERS.md](PROVIDERS.md) |
| Cortex events | [CORTEX.md](CORTEX.md) |
| Callosum | [CALLOSUM.md](CALLOSUM.md) |

Agents invoke journal data operations through `solstone call <app> <verb>`. See
[SOLCLI.md](SOLCLI.md).

### Search publication

Saving or removing journal content adds a durable path hint through
`solstone_core_indexer_store::todo::enqueue_path(journal, path)`. Producers do not
open SQLite or acquire its writer lease. Entity changes enqueue `EntitySearch`;
facet declaration changes enqueue `Reclassify`. Existing corpus-wide import and
restore operations enqueue `ScanFull`.

`solstone journal indexer drain` consumes due hints. It reconciles each path with
the current filesystem, including deleted children of a queued directory. One
persistent drain lock protects claims and crash recovery, while the SQLite writer
lease is released between bounded batches. A failed present path is retried three
times, then left to the daily catch-all. Failed removals remain queued and retry
hourly after that limit. Unknown entry kinds and fields are preserved for newer
versions.

The supervisor checks due hints every 30 seconds without relying on Callosum.
Daily thinking first runs a forced drain with its own ten-minute budget, then
continues its talents even if the wait expires, recording pending paths. Its later
indexer phase drains hints too. The enabled `search-catch-all` schedule runs a
light `indexer --rescan` daily with a two-hour cap, separately from daily thinking.
Mobile hosts supply the same drain capability in process, after thinking and from
their scheduled loop. Health reports queued work, failed removals, and the age and
repair counts of the last finished light check. A segment publication receipt
means its saved outputs were queued successfully; search visibility follows later.

### Calendar listing

Ingest queues each kept calendar note file on the search work list, and the
single index writer indexes it like any other path. Each note becomes a hit on
the day it was stored. The writer also keeps a listing: for every event identity,
rows under each day the event happens, built from the latest note received for
it (latest `changed_at` as an instant, then `removed` over `changed` over
`added`, then by file name). A removal lists nothing. A timed event belongs to
each local day it overlaps in the journal's zone, end exclusive; an all-day event
to its own dates; an event lists at most 366 days. The rows are filed under the
latest note's file, and replacing them happens in the same index transaction
that indexes the newer note.

The listing is derived from stored notes alone, so a rebuild yields the same
listing, and nothing outside the index writer writes it. Removing a segment
drops its notes and falls each affected event back to its latest surviving note.
Timed events are listed in the journal's zone as it is when they are indexed: if
the owner changes the journal's time zone, timed events keep their old days until
the search index is rebuilt. The journal's usual fallback zone applies when none
is configured. While the settings file cannot be read, existing listing rows stay
as they are, the scan reports the failure once, and a note that arrives meanwhile
is listed once the file reads again.

A calendar file imported from an `.ics` export or a zip of them also joins the
listing. The importer writes calendar note files in the same format beside each
import window's transcript in the `import.ics` stream, one line per event or
expanded occurrence, and queues them for search alone: they ask for no
processing and mark no day as changed. The importer reads each event's `UID` as
its `event` (or derives a stable one from its content when there is none) and
its `RECURRENCE-ID` as its `occurrence`. It expands a repeating event's rule
up to a year past the import, keeping a series' most recent 5,000 dates, and
keeps each start's local wall time across daylight-saving changes, whatever the
machine's own zone; lines expanded from a rule carry `"expanded": true`. An
override stands for its date only when it writes a line of its own. Each line's
`changed_at` is the instant that placed its event's transcript, `LAST-MODIFIED`
or else `CREATED`, in the journal zone's offset, and it must fall inside that
segment's own span rather than the 300-second grid; an import window closes
where the zone's offset changes, so a daylight-saving change never splits one
file's times between two offsets. Each line also names its `import` and when
that import happened (`imported_at`, the upload time from the import's record,
or for a record with none the time its first save recorded there). An event
whose change time is missing or unreadable, or whose `TZID` matches no zone,
directly or through the Windows zone names Outlook exports, writes no line, and
the import's summary counts it. One import writes at most 200,000 lines; the
summary counts the events past that limit.

Imported lines are listed only: the import's transcript stays the record of the
day each event was stored, so they add no stored-day hit, and like phone notes
they carry no facet. An imported calendar file that fails the note contract or
its segment's span is skipped and reported once.
Among imported lines, each import of an event defines it across the dates it
covers, for every older import: from its earliest written occurrence to its
latest, or, when it expanded the event's rule, to a year past that import.
Imports are ordered by `imported_at`, with the import id breaking ties, and an
import's line counts only where no newer import covers its date. So dates a
newer export dropped stay out of the listing even after a still newer import
that holds a single changed date, and an older import's dates outside every
newer span still count. Phone notes are never set aside by this rule, and
between them and the imported lines that count the latest `changed_at` still
wins. Re-saving an import replaces only that import's calendar files in its
segments. Spans are read from surviving lines, so removing every segment of an
import that holds an event lets the older imports' dates it covered count
again. A repeating event that a later export holds as a single event keeps the
older imports' dates outside that one date, since the newer import covers only
the date it holds. Calendars imported before this change wrote no calendar note
files, and nothing backfills them.

### Interrupted activity work

`think-cli::activity_work` persists unfinished talent names and accepted Cortex use
IDs under `health/activity-work/` before waiting for results. A per-activity claim
serializes dispatch and completion. Successful siblings are removed from the pending
set; final success publishes the existing input provenance before removing the work
record. A retry reattaches a running use or folds its durable finish before considering
a new dispatch. A changed input fingerprint starts a new unit set after previous uses
have stopped.

The supervisor checks due work once per minute and submits at most eight identities
through its existing task queue. Connection failures retry indefinitely, with delays
from one minute to one hour; explicit non-retryable worker errors remain pending for
repair or an explicit refresh. Retry discovery includes older pending records,
independently of segment or daily completion. On startup and date rollover, existing
connection failures from the current and previous source days are adopted without
replaying historical segments. Remote and deferred-processing modes do not drain this
queue. Activity failure counts propagate through the segment replay outcome.

## Prompt Context Configuration

Generators and talents accept an optional `load` key in frontmatter:

```json
{
  "load": {"transcripts": true, "percepts": false, "talents": {"screen": true}}
}
```

- `false` — do not load this source
- `true` — load if available
- `"required"` — load, and skip generation if nothing is there
- For `agents` only: a dict of agent names, e.g. `{"entities": true, "meetings": "required"}`. `{}` means no agents.

Inline template variables in the `.md` body:

- `$facets` — focused facet context or all facets
- `$activity_context` — activity metadata, segment state, analysis focus

Priority is required on every scheduled prompt. See [CORTEX.md](CORTEX.md).

## Schedules

`config/schedules.json` stores scheduler metadata and named entries. Reserved
keys are `daily_time`, `weekly_day`, and `weekly_time`. Every other top-level
key is a named entry with `cmd` and `every`, plus optional `enabled` and
`max_runtime`. New schedule configurations set the daily scheduler time to
`00:15` and weekly scheduler time to Sunday at `03:15`, keeping both off the
hourly boundary. Existing configurations are not backfilled with those metadata
values. Writes go through `solstone-core-system`.

At every start, before the scheduler loads, the supervisor brings the file up
to date with what the journal ships: the built-in entries (`heartbeat`,
`weekly-agents`, `cadence`, `brain`, `facet-candidates`, `rebuild-edges`) are
added when absent, and the `maintenance:<routine>` entries generated from the
maintenance routine registry are reconciled, retired names removed and missing
routines added with their default cadence. Any entry an operator edited or
added is left as it is, and an entry first added at a start waits for its next
cadence mark rather than running in the boot catch-up. `solstone journal maintenance
sync` performs the maintenance half on demand.

## Related

- [PROVIDERS.md](PROVIDERS.md) — one active brain, no fallback
- [CORTEX.md](CORTEX.md) — talent spawn and events
- [CALLOSUM.md](CALLOSUM.md) — message bus
- [PROMPT_TEMPLATES.md](PROMPT_TEMPLATES.md) — `$name` and related variables
- [GENERATE.md](GENERATE.md) — generate contract
