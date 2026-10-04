# Facets

The `facets/` directory provides a way to organize journal content by scope or focus area. Each facet represents a cohesive grouping of related activities, projects, or areas of interest.

## Facet structure

Each facet is organized as `facets/<facet>/` where `<facet>` is a descriptive short unique name. When referencing facets in the system, use hashtags (e.g., `#personal` for the "Personal Life" facet, `#ml_research` for "Machine Learning Research"). Each facet folder contains:

- `facet.json` – metadata file with facet title and description.
- `activities/` – configured activities and completed activity records (see [activity records](#activity-records)).
- `entities/` – entity relationships and detected entities (see [facet entities](#facet-entities)).
- `events/` – historical extracted events per day (see [historical event extracts](captures.md#historical-event-extracts)).
- `news/` – daily news and updates relevant to the facet (optional).
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
  - Muted facets are filtered out by `get_enabled_facets()`, so agents that iterate enabled facets, such as `entity_observer`, skip them silently.

## Facet Entities

Entities in solstone use a two-tier architecture with **journal-level entities** (canonical identity) and **facet relationships** (per-facet context). There are also **detected entities** (daily discoveries) that can be promoted to attached status.

### Entity Storage Structure

```
entities/
  └── {entity_id}/
      └── entity.json              # Journal-level entity (canonical identity)

facets/{facet}/
  └── entities/
      ├── YYYYMMDD.jsonl           # Daily detected entities
      └── {entity_id}/
          ├── entity.json          # Facet relationship
          ├── observations.jsonl   # Durable facts (optional)
          └── voiceprints.npz      # Voice recognition data (optional)
```

**Journal-level entities** (`entities/<id>/entity.json`) store the canonical identity: name, type, aliases (aka), and principal flag. These are shared across all facets.

**Facet relationships** (`facets/<facet>/entities/<id>/entity.json`) store per-facet context: description, timestamps, and custom fields specific to that facet.

**Entity memory** (observations, voiceprints) is stored alongside facet relationships.

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
- `id` (string) – Stable slug identifier derived from name via `entity_slug()` in `solstone/think/entities/` (lowercase, underscores, e.g., "Alice Johnson" → "alice_johnson"). Used for folder paths, URLs, and tool references.
- `name` (string) – Display name for the entity.
- `type` (string) – Entity type (e.g., "Person", "Company", "Project", "Tool"). Types are flexible and owner-defined; must be alphanumeric with spaces, minimum 3 characters.
- `aka` (array of strings) – Alternative names, nicknames, or acronyms. Used in audio transcription and fuzzy matching.
- `is_principal` (boolean) – When `true`, identifies this entity as the journal owner. Auto-flagged when name/aka matches identity config.
- `blocked` (boolean) – When `true`, entity is hidden from all facets and excluded from agent context.
- `created_at` (integer) – Unix timestamp in milliseconds when entity was created.

### Facet Relationships

Facet relationships link journal entities to specific facets with context:

```json
{
  "entity_id": "alice_johnson",
  "description": "Lead engineer on the API project",
  "attached_at": 1704067200000,
  "updated_at": 1704153600000,
  "last_seen": "20260115"
}
```

**Relationship fields:**
- `entity_id` (string) – Links to the journal entity.
- `description` (string) – Facet-specific description.
- `attached_at` (integer) – Unix timestamp when attached to this facet.
- `updated_at` (integer) – Unix timestamp of last modification.
- `last_seen` (string) – Day (YYYYMMDD) when last mentioned in journal content.
- `detached` (boolean) – When `true`, soft-deleted from this facet but data preserved.
- Custom fields (any) – Additional facet-specific metadata (e.g., `tier`, `status`, `priority`).

### Detected Entities

Daily detection files (`facets/<facet>/entities/YYYYMMDD.jsonl`) contain entities automatically discovered by agents from journal content:

```jsonl
{"type": "Person", "name": "Charlie Brown", "description": "Mentioned in standup meeting"}
{"type": "Tool", "name": "React", "description": "Used in UI development work"}
```

### Entity Lifecycle

1. **Detection**: Daily agents scan journal content and record entities in `facets/<facet>/entities/YYYYMMDD.jsonl`
2. **Aggregation**: Review agent tracks detection frequency across recent days
3. **Promotion**: Entities with 3+ detections are auto-promoted to attached, or owners manually promote via UI
4. **Persistence**: Creates journal entity + facet relationship; remains active until detached
5. **Detachment**: Sets `detached: true` on facet relationship, preserving all data
6. **Re-attachment**: Clears detached flag, restoring the entity with preserved history
7. **Blocking**: Sets `blocked: true` on journal entity and detaches from all facets

### Cross-Facet Behavior

The same entity can be attached to multiple facets with independent descriptions and timestamps. When loading entities across all facets, the alphabetically-first facet wins for duplicates during aggregation.

## Facet News

The `news/` directory provides a chronological record of news, updates, and external developments relevant to the facet. This allows tracking of industry news, research updates, regulatory changes, or any external information that impacts the facet's focus area.

### News organization

News files are organized by date as `news/YYYYMMDD.md` where each file contains the day's relevant news items. Only create files for days that have news to record—sparse population is expected.

### News file format

Each `YYYYMMDD.md` file is a markdown document with a consistent structure:

```markdown
# 2025-01-18 News - Machine Learning Research

## OpenAI Announces New Model Architecture
**Source:** techcrunch.com | **Time:** 09:15
Summary of the announcement and its relevance to current research projects...

## Paper: "Efficient Attention Mechanisms in Transformers"
**Source:** arxiv.org | **Time:** 14:30
Key findings from the paper and potential applications...

## Google Research Updates Dataset License Terms
**Source:** blog.google | **Time:** 16:45
Changes to dataset licensing that may affect ongoing experiments...
```

### News entry structure

Each news entry should include:
- **Title** – concise headline as a level 2 heading
- **Source** – origin of the news (website, journal, etc.)
- **Time** – optional time of publication or discovery (HH:MM format)
- **Summary** – brief description focusing on relevance to the facet
- **Impact** – optional notes on how this affects facet work

### News metadata

Optionally, a `news.json` file can be maintained at the root of the news directory to track metadata:

```json
{
  "last_updated": "2025-01-18",
  "sources": ["arxiv.org", "techcrunch.com", "nature.com"],
  "auto_fetch": false,
  "keywords": ["transformer", "attention", "llm", "research"]
}
```

This allows for future automation of news gathering while maintaining manual curation quality.

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
{"id": "coding_095809_303", "activity": "coding", "segments": ["095809_303", "100313_303", "100816_303", "101320_302"], "level_avg": 1.0, "title": "Developed extraction prompts using Claude Code and VS Code", "description": "Developed extraction prompts using Claude Code and VS Code", "details": "", "active_entities": ["Claude Code", "VS Code", "sunstone"], "hidden": false, "story": {"talent": "work", "body": "Iterated on the extraction flow and validated generated output paths.", "topics": ["extraction prompts"], "confidence": 0.8}, "commitments": [], "closures": [], "decisions": [], "relations": [], "participation": [], "edits": [{"timestamp": "2026-02-09T18:19:02.114200Z", "actor": "participation", "fields": ["participation"], "note": "updated participation"}, {"timestamp": "2026-02-09T18:20:19+00:00", "actor": "story", "fields": ["story", "commitments", "closures", "decisions", "relations"], "note": ""}], "created_at": 1770435619415}
{"id": "meeting_090953_303", "activity": "meeting", "segments": ["090953_303", "091457_303", "092001_304", "092506_304", "093010_304"], "level_avg": 1.0, "title": "Sprint Planning", "description": "Sprint planning meeting with the engineering team", "details": "", "active_entities": ["Alice", "Bob"], "hidden": false, "source": "user", "edits": [], "created_at": 1770435619420}
```

### Record ID scheme

Activity record IDs follow the format `{activity_type}_{segment_key}` where `segment_key` is the segment in which the activity started. This is unique within a facet+day because only one activity of a given type can start in a given segment for one facet.

### Record fields

- `id` (string) – Unique identifier: `{activity}_{start_segment_key}` (e.g., `coding_095809_303`)
- `activity` (string) – Activity type ID from the facet's configured activities
- `segments` (array of strings) – Ordered list of segment keys where this activity was active
- `stream` (string) – The stream whose segments the activity covers, on records written by segment thinking for a named stream. Another stream can hold a segment with the same key, so whatever reads the activity's segments reads them from this stream. Absent on older records and for segments filed directly under the day.
- `level_avg` (float) – Engagement level of the last segment that listed this facet above low. Newly tracked automatic records contain 0.5 (medium) or 1.0 (high); low facets are excluded before tracking. Older records may differ. The stored key is historical: this is not an average. Records you create may omit it.
- `title` (string) – Human title for the activity span; falls back to `description` unless set explicitly (for example by a CLI edit)
- `description` (string) – Description of the activity span, carried from the per-segment activity state when the record is written
- `details` (string) – Optional longer-form narrative detail for the span
- `active_entities` (array of strings) – Merged and deduplicated entity names from all segments
- `hidden` (boolean) – When `true`, the record is muted from default list views
- `source` (string) – Origin of a record that did not come from segment thinking: `anticipated` (the schedule talent) or `user`; historical inferred records can carry `cogitate`. Absent on records written by segment thinking.
- `edits` (array of objects) – Append-only edit history with `timestamp`, `actor`, `fields`, and `note`
- `created_at` (integer) – Unix timestamp in milliseconds when the record was created

### Lifecycle

Activity records are written by segment thinking when an activity ends:

1. After a segment's `talents/sense.json` is saved, the stream's live activity state machine applies its segments in capture order, never in arrival order: the segment and any later ones of the stream that already have Sense, stopping before an earlier segment that has arrived but is still being thought: for 15 minutes after it arrived or after another of the stream's waiting segments got its Sense, and never more than two hours. A new day first applies whatever of the previous day was still waiting. A segment that is not newer than the stream's last applied one never ends, extends or rewinds live activities: it joins the open activity it falls inside, and otherwise the stream's day is rebuilt in capture order without saving live state, which writes only a finished activity holding that segment, if its ID is new. A published activity's segments stay as written. Each entry has an `id` (`{activity}_{since}`) that identifies the activity span. Each stream has its own state, kept in `awareness/activity_state/<stream>.json`, so a segment from one stream never ends or extends an activity from another. Segment thinking and the flush take turns on a stream's state, so neither writes over what the other started or ended.
2. An activity ends at once on an idle segment from its stream, or when its stream's next segment starts more than 10 minutes later or on another day. It also ends when its facet drops out of the sense output (or falls to low), or the sense output shows a different activity type, for two segments in a row.
3. When an activity ends, a record covering its whole segment span is written to the facet's day file, and the agent work it needs is recorded. Every activity that ended is written before any agent runs and before the stream's state is saved, so a run that stops partway loses no record, and agent work it left unfinished resumes on its own.
4. Activity-scheduled agents whose `activities` list matches the record's type then run for it. A story agent's output is merged onto the record as its `story` (body, topics, confidence) and its `commitments`, `closures`, `decisions` and `relations`; the participation agent adds `participation`. When the activity was too large for the provider's context window and its oldest input was left out, `story` also carries `partial_input` (`dropped_entries`, `dropped_chars`), so the story is not read as covering the whole activity.
5. Later CLI edits append to the record's `edits` log and may hide/unhide the record without changing its ID

**Segment flush:** If a stream sends no new segments for an hour, the supervisor runs `solstone journal think --flush` on that stream's last segment, unless processing is deferred or no model is chosen. Flush first applies any of the stream's segments still waiting on an earlier one that never got Sense. If that segment is then the last one the stream's activity state has seen, flush ends the activities still open there, writes their records and runs their activity agents. This way the last activity before intake stops (a locked screen, for example) gets its record about an hour later, while the journal keeps running, instead of waiting for intake to resume. At the day rollover the supervisor runs the same flush for each stream's last segment of the previous day. Flush also runs any segment agent that declares `hook.flush: true`.

Records are written idempotently — duplicate IDs are skipped on re-runs.

### Generated output

The stock activity agents (`conversation`, `work` and `participation`) write no file of their own: their output is merged onto the activity record. An activity-scheduled agent without a post hook writes its output alongside the records, organized by day and record ID:

```
facets/{facet}/activities/{day}/{activity_id}/{agent}.{ext}
```

The think process builds that path and passes it as `output_path` in the agent request. Search indexes only Markdown outputs there, through the `facets/*/activities/*/*/*.md` formatter pattern.
