# Facets

The `facets/` directory provides a way to organize journal content by scope or focus area. Each facet represents a cohesive grouping of related activities, projects, or areas of interest.

## Facet structure

Each facet is organized as `facets/<facet>/` where `<facet>` is a descriptive short unique name. When referencing facets in the system, use hashtags (e.g., `#personal` for the "Personal Life" facet, `#ml_research` for "Machine Learning Research"). Each facet folder contains:

- `facet.json` – metadata file with facet title and description.
- `activities/` – configured activities and completed activity records (see [activity records](#activity-records)).
- `entities/` – entity relationships and detected entities (see [facet entities](#facet-entities)).
- `events/` – historical extracted events per day (see [historical event extracts](captures.md#historical-event-extracts)).
- `news/` – owner markdown (the product no longer writes, shows, or indexes it; do not create files there).
- `logs/` – action audit logs for tool calls (optional, see [action logs](logs.md#action-logs)).

## Facet metadata

The `facet.json` file contains basic information about the facet:

```json
{
  "title": "Machine Learning Research",
  "description": "AI/ML research projects, experiments, and related activities",
  "color": "#4f46e5",
  "emoji": "🧠"
}
```

Optional fields:
- `color` – hex color code for the facet card background in the web UI
- `emoji` – emoji icon displayed in the top-left of the facet card
- `muted` – boolean flag to mute/hide the facet from views (default: false)
  - Muted facets are excluded from enabled-facet lists (such as `sense`, `morning_briefing`, and `weekly_reflection`), so new segments aren't routed to them, and pending activity enrichment pauses while muted.

## Facet Entities

Entities in the journal use a two-tier architecture with **journal-level entities** (canonical identity) and **facet relationships** (per-facet context). There are also **detected entities** (daily discoveries) that can be promoted to attached status.

### Entity Storage Structure

```
entities/
  └── {entity_id}/
      ├── entity.json              # Journal-level entity (canonical identity)
      └── voiceprints.npz          # Voice recognition data (optional)

facets/{facet}/
  └── entities/
      ├── YYYYMMDD.jsonl           # Daily detected entities
      └── {entity_id}/
          ├── entity.json          # Facet relationship
          └── observations.jsonl   # Durable notes (optional)
```

**Journal-level entities** (`entities/<id>/entity.json`) store the canonical identity: name, type, aliases (aka), and principal flag. These are shared across all facets.

**Facet relationships** (`facets/<facet>/entities/<id>/entity.json`) store per-facet context: description, timestamps, and custom fields specific to that facet.

**Entity memory:** notes (`observations.jsonl`) are stored alongside facet relationships; `voiceprints.npz` is stored alongside the journal-level entity.

### Journal-Level Entities

Journal entities represent the canonical identity record:

```json
{
  "id": "alice_johnson",
  "name": "Alice Johnson",
  "type": "Person",
  "aka": ["Ali", "AJ"],
  "is_principal": false,
  "created_at": 1704067200000
}
```

**Standard fields:**
- `id` (string) – Stable slug identifier derived from name via `entity_slug()` (lowercase, underscores, e.g., "Alice Johnson" → "alice_johnson"). Used for folder paths, URLs, and tool references.
- `name` (string) – Display name for the entity.
- `type` (string) – Entity type (e.g., "Person", "Company", "Project", "Tool"). Types are flexible and owner-defined; must be alphanumeric with spaces, minimum 3 characters.
- `aka` (array of strings) – Alternative names, nicknames, or acronyms. Used in speaker resolution and fuzzy matching.
- `is_principal` (boolean) – When `true`, identifies this entity as the journal owner. Auto-flagged when name/aka matches identity config.
- `blocked` (boolean) – When `true`, the owner has hidden the entity. It is detached from all facets and excluded from normal entity context. Explicit reads can still return it with this status.
- `created_at` (string or integer) – UTC RFC 3339 timestamp or Unix timestamp in milliseconds when the entity was created.

### Facet Relationships

Facet relationships link journal entities to specific facets with context:

```json
{
  "entity_id": "alice_johnson",
  "description": "Lead engineer on the API project",
  "attached_at": "2026-01-15T12:00:00.000000Z",
  "updated_at": "2026-01-16T12:00:00.000000Z",
  "last_seen": "20260115"
}
```

**Relationship fields:**
- `entity_id` (string) – Links to the journal entity.
- `description` (string) – Facet-specific description.
- `attached_at` (string; integer epoch ms on older records) – UTC RFC 3339 timestamp when attached to this facet.
- `updated_at` (string; integer epoch ms on older records) – UTC RFC 3339 timestamp of last modification.
- `last_seen` (string) – Day (YYYYMMDD) when last mentioned in journal content.
- `detached` (boolean) – When `true`, detached from this facet with its history preserved.
- Custom fields (any) – Additional facet-specific metadata (e.g., `tier`, `status`, `priority`).

### Detected Entities

Daily detection files (`facets/<facet>/entities/YYYYMMDD.jsonl`) contain entities automatically discovered from journal content:

```jsonl
{"type": "Person", "name": "Charlie Brown", "description": "Mentioned in standup meeting"}
{"type": "Tool", "name": "React", "description": "Used in UI development work"}
```

### Entity Lifecycle

1. **Detection**: Per-segment `entities:detection` checks each entity `sense` extracted and writes the ones that took part in that moment to `facets/<facet>/entities/YYYYMMDD.jsonl` for the segment's active facets
2. **Aggregation**: Daily `entities:entities_review` scans the prior 7 days of detections per facet and counts distinct days seen per entity with a dominant type (2 days for `Person`, 3 for `Company` or `Project`, 5 for `Tool` or other types)
3. **Promotion**: `entities:entities_review` reviews eligible candidates and promotes recurring, specific ones to attached status in that facet (declining generic words, placeholders, or contradictions); owners can also promote manually in the UI
4. **Persistence**: Creates journal entity + facet relationship; remains active until detached
5. **Detachment**: Sets `detached: true` on facet relationship, preserving all data
6. **Re-attachment**: Clears detached flag, restoring the entity with preserved history
7. **Hiding**: Sets `blocked: true` on journal entity and detaches from all facets

### Cross-Facet Behavior

The same entity can be attached to multiple facets with independent descriptions and timestamps. When loading entities across all facets, the alphabetically-first facet wins for duplicates during aggregation.

## What processing reads from a facet

Each segment's `sense` step reads a short summary of each facet that isn't muted. Run `solstone journal talent show sense --prompt` to see it.

**The description** is how the model tells the facets apart when it decides which ones a segment belongs to. A facet with no description is known by its title alone. A useful description is one plain sentence about what goes in the facet and what doesn't, such as "Client work for Acme: meetings, code review, invoices; not personal email". Set it with `solstone call journal facet update <name> --description "..."`.

**The attached entities.** Under each facet, the summary lists up to 20 of that facet's attached entities, by name only. They are ranked by how many notes the journal has saved about each one (what `solstone call entities observations` lists), then by how recently it last saved one. The rest are counted as "and N more". Detached and hidden entities are never listed. The list gives the model each facet's known names. An entity from the list is kept for a segment only when that segment itself shows or says the name.

**The owner is not in this list.** Prompts name the owner directly and tell the model never to list them as an entity.

A step that runs for one facet reads that facet alone. It sees the facet's description, the owner's role in that facet when the owner's entry there has a description, and up to 20 entities with each one's description for that facet. An owner entry with no description stays among those entities.

**Curating it:**
- **Detach** an entity from a facet on its entities page when it doesn't belong there. It leaves that facet's list and keeps its history, and re-attaching restores it.
- **Hide** an entity that shouldn't be offered anywhere. It leaves every facet's list. You can't hide yourself.
- **Describe** an entity in a facet with `solstone call entities update <entity> "<description>" -f <facet>`. `sense` doesn't read it. A step that runs for that one facet does, and so does the step that notes each name `sense` found in a segment of that facet. It doesn't change the ranking.
- Which entities make the top 20 follows how many notes are saved for each one. To make room for one that ranks lower, detach the ones above it that don't belong in the facet.

## Facet News

The `news/` directory may hold owner markdown files. The product no longer writes, shows, or indexes files here; do not create files there.

## Activity Records

The `activities/` directory within each facet stores both the configured activity types (`activities.jsonl`) and completed activity records organized by day (`{day}.jsonl`). Activity records represent completed spans of activity — periods where a specific activity type continued across one or more segments.

**File path pattern:**
```
facets/personal/activities/activities.jsonl                        # Configured activity types
facets/personal/activities/20260209.jsonl                          # Completed records for the day
facets/work/activities/20260209.jsonl
```

Each day file contains one JSON object per line, where each record represents a completed activity span:

```jsonl
{"id": "coding_095809_303", "activity": "coding", "segments": ["095809_303", "100313_303", "100816_303", "101320_302"], "level_avg": 1.0, "title": "Developed extraction prompts using Claude Code and VS Code", "description": "Developed extraction prompts using Claude Code and VS Code", "details": "", "active_entities": ["Claude Code", "VS Code", "Acme API"], "hidden": false, "story": {"talent": "work", "body": "Iterated on the extraction flow and validated generated output paths.", "topics": ["extraction prompts"], "confidence": 0.8}, "relations": [], "participation": [], "edits": [{"timestamp": "2026-02-09T18:19:02.114200Z", "actor": "participation", "fields": ["participation"], "note": "updated participation"}, {"timestamp": "2026-02-09T18:20:19+00:00", "actor": "story", "fields": ["story", "relations"], "note": ""}], "created_at": 1770435619415}
{"id": "meeting_090953_303", "activity": "meeting", "segments": ["090953_303", "091457_303", "092001_304", "092506_304", "093010_304"], "level_avg": 1.0, "title": "Sprint Planning", "description": "Sprint planning meeting with the engineering team", "details": "", "active_entities": ["Alice", "Bob"], "hidden": false, "source": "user", "edits": [], "created_at": 1770435619420}
```

### Record ID scheme

Activity record IDs follow the format `{activity_type}_{segment_key}` where `segment_key` is the segment in which the activity started. Within one stream this is unique within a facet+day, because only one activity of a given type can start in a given segment for one facet. Two streams can hold segments with the same key, and clients that align segments to the clock often do. When another stream's activity already holds that ID, the record takes `{activity_type}_{stream}_{segment_key}` instead. Which of the two keeps the plain ID depends on which was written first.

### Record fields

- `id` (string) – Unique identifier: `{activity}_{start_segment_key}` (e.g., `coding_095809_303`)
- `activity` (string) – Activity type ID from the facet's configured activities
- `segments` (array of strings) – Ordered list of segment keys where this activity was active
- `stream` (string) – The stream whose segments the activity covers, on records written by segment thinking for a named stream. Another stream can hold a segment with the same key, so whatever reads the activity's segments reads them from this stream. Absent on older records and for segments filed directly under the day.
- `level_avg` (float) – Engagement level of the last segment that listed this facet above low. Newly tracked automatic records contain 0.5 (medium) or 1.0 (high); low facets are excluded before tracking. Older records may differ. The stored key is historical: this is not an average. Records you create may omit it.
- `title` (string) – Human title for the activity span; falls back to `description` unless set explicitly (for example by a CLI edit). When the activity's story is written, its opening sentence becomes the title, unless the owner wrote the activity or edited the title
- `description` (string) – Description of the activity span, carried from the per-segment activity state when the record is written, which is the line of its latest segment. When the activity's story is written, its opening sentence replaces it, unless the owner wrote the activity or edited the description
- `details` (string) – Optional longer-form narrative detail for the span
- `active_entities` (array of strings) – Merged and deduplicated entity names from all segments
- `hidden` (boolean) – When `true`, the record is muted from default list views
- `source` (string) – Origin of a record that did not come from segment thinking: `user` (owner-created) or `body` (imported body records); historical records can carry `cogitate` or `anticipated`. Absent on records written by segment thinking.
- `edits` (array of objects) – Append-only edit history with `timestamp`, `actor`, `fields`, and `note`
- `created_at` (integer) – Unix timestamp in milliseconds when the record was created

### Lifecycle

Activity records are written by segment thinking when an activity ends:

1. After a segment's `talents/sense.json` is saved, the stream's live activity state machine applies its segments in chronological order, never in arrival order: the segment and any later ones of the stream that already have `sense`, stopping before an earlier segment that has arrived but is still being thought: for 15 minutes after it arrived or after another of the stream's waiting segments got its `sense`, and never more than two hours. A new day first applies whatever of the previous day was still waiting. A segment that is not newer than the stream's last applied one never ends, extends or rewinds live activities: it joins the open activity it falls inside, and otherwise the stream's day is rebuilt in chronological order without saving live state, which writes only a finished activity holding that segment, if its ID is new. A published activity's segments stay as written. Each entry has an `id` (`{activity}_{since}`) that identifies the activity span. Each stream has its own state, kept in `awareness/activity_state/<stream>.json`, so a segment from one stream never ends or extends an activity from another. Segment thinking and the flush take turns on a stream's state, so neither writes over what the other started or ended.
2. An activity ends at once on an idle segment from its stream, or when its stream's next segment starts more than 10 minutes later or on another day. It also ends when its facet drops out of the sense output (or falls to low), or the sense output shows a different activity type, for two segments in a row.
3. When an activity ends, a record covering its whole segment span is written to the facet's day file, and the step work it needs is queued. Every activity that ended is written before any step runs and before the stream's state is saved, so a run that stops partway loses no record, and step work it left unfinished resumes on its own.
4. Activity-scheduled steps whose `activities` list matches the record's type then run for it. A story step's output is merged onto the record as its `story` (`body`, `topics`, `confidence`) and its `relations`; the participation step adds `participation`. When the activity was too large for the provider's context window and its oldest input was left out, `story` also carries `partial_input` (`dropped_entries`, `dropped_chars`), so the story is not read as covering the whole activity.
5. Later CLI edits append to the record's `edits` log and may hide/unhide the record without changing its ID

**Segment flush:** If a stream sends no new segments for an hour, the supervisor runs `solstone journal think --flush` on that stream's last segment, unless processing is deferred or no model is chosen. Flush first applies any of the stream's segments still waiting on an earlier one that never got `sense`. If that segment is then the last one the stream's activity state has seen, flush ends the activities still open there, writes their records and runs their activity steps. This way the last activity before intake stops (a locked screen, for example) gets its record about an hour later, while the journal keeps running, instead of waiting for intake to resume. At the day rollover the supervisor runs the same flush for each stream's last segment of the previous day. Flush also runs any segment step that declares `hook.flush: true`.

Records are written idempotently — duplicate IDs are skipped on re-runs.

### Generated output

The stock activity steps (`conversation`, `work` and `participation`) write no file of their own: their output is merged onto the activity record. An activity-scheduled step without a post hook writes its output alongside the records, organized by day and record ID:

```
facets/{facet}/activities/{day}/{activity_id}/{agent}.{ext}
```

The think process builds that path and passes it as `output_path` in the step request. Search indexes only Markdown outputs there, through the `facets/*/activities/*/*/*.md` formatter pattern.
