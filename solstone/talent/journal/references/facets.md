# Facets

The `facets/` directory provides a way to organize journal content by scope or focus area. Each facet represents a cohesive grouping of related activities, projects, or areas of interest.

## Facet structure

Each facet is organized as `facets/<facet>/` where `<facet>` is a descriptive short unique name. When referencing facets in the system, use hashtags (e.g., `#personal` for the "Personal Life" facet, `#ml_research` for "Machine Learning Research"). Each facet folder contains:

- `facet.json` – metadata file with facet title and description.
- `activities/` – configured activities and completed activity records (see [activity records](#activity-records)).
- `entities/` – entity relationships and detected entities (see [facet entities](#facet-entities)).
- `todos/` – daily todo lists (see [facet-scoped todos](#facet-scoped-todos)).
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

The `activities/` directory within each facet stores both the configured activity types (`activities.jsonl`) and completed activity records organized by day (`{day}.jsonl`). Activity records represent completed spans of activity — periods where a specific activity type was continuously tracked across one or more recording segments.

**File path pattern:**
```
facets/personal/activities/activities.jsonl                        # Configured activity types
facets/personal/activities/20260209.jsonl                          # Completed records for the day
facets/work/activities/20260209.jsonl
facets/work/activities/20260209/coding_095809_303/session_review.md  # Generated output
```

Each day file contains one JSON object per line, where each record represents a completed activity span:

```jsonl
{"id": "coding_095809_303", "activity": "coding", "segments": ["095809_303", "100313_303", "100816_303", "101320_302"], "level_avg": 0.88, "title": "Prompt Refactor Session", "description": "Developed extraction prompts using Claude Code and VS Code", "details": "Iterated on the extraction flow and validated generated output paths.", "active_entities": ["Claude Code", "VS Code", "sunstone"], "hidden": false, "source": "cogitate", "edits": [{"timestamp": "2026-02-09T18:20:19Z", "actor": "cogitate:activities", "fields": ["title", "description", "details"], "note": "synthesized activity summary"}], "created_at": 1770435619415}
{"id": "meeting_090953_303", "activity": "meeting", "segments": ["090953_303", "091457_303", "092001_304", "092506_304", "093010_304"], "level_avg": 1.0, "title": "Sprint Planning", "description": "Sprint planning meeting with the engineering team", "details": "", "active_entities": ["Alice", "Bob"], "hidden": false, "source": "user", "edits": [], "created_at": 1770435619420}
```

### Record ID scheme

Activity record IDs follow the format `{activity_type}_{segment_key}` where `segment_key` is the segment in which the activity started. This is unique within a facet+day because only one activity of a given type can start in a given segment for one facet.

### Record fields

- `id` (string) – Unique identifier: `{activity}_{start_segment_key}` (e.g., `coding_095809_303`)
- `activity` (string) – Activity type ID from the facet's configured activities
- `segments` (array of strings) – Ordered list of segment keys where this activity was active
- `level_avg` (float) – Average engagement level across all segments (high=1.0, medium=0.5, low=0.25)
- `title` (string) – Human title for the activity span; newer records set this explicitly, older records may fall back to `description`
- `description` (string) – AI-synthesized description of the full activity span
- `details` (string) – Optional longer-form narrative detail for the span
- `active_entities` (array of strings) – Merged and deduplicated entity names from all segments
- `hidden` (boolean) – When `true`, the record is muted from default list views
- `source` (string) – Origin of the record, currently `cogitate` or `user`
- `edits` (array of objects) – Append-only edit history with `timestamp`, `actor`, `fields`, and `note`
- `created_at` (integer) – Unix timestamp in milliseconds when the record was created

### Lifecycle

Activity records are created by the `activities` segment agent when it detects that an activity has ended:

1. The `activity_state` agent tracks per-segment, per-facet activity states with continuity via `since` fields. Each entry includes an `id` field (`{activity}_{since}`) that uniquely identifies the activity span, and `activity.live` events are emitted for active entries.
2. The `activities` agent runs after `activity_state` and compares previous vs. current segment states
3. When an activity ends (explicitly, implicitly, or via timeout), the agent walks the segment chain to collect all data
4. A record is written to the facet's day file with preliminary description
5. An LLM synthesizes all per-segment descriptions into a unified narrative
6. The synthesized summary updates the record's `description`, and may also fill `title` and `details`
7. Later CLI edits append to the record's `edits` log and may hide/unhide the record without changing its ID

**Segment flush:** If no new segments arrive for an extended period (1 hour), the supervisor triggers `journal think --flush` on the last segment. Agents that declare `hook.flush: true` (like `activities`) run with `flush=True` in their context, treating all remaining active activities as ended. This ensures activities are recorded promptly even when the owner stops working, and prevents cross-day data loss.

Records are written idempotently — duplicate IDs are skipped on re-runs.

### Generated output

Activity-scheduled agents (`schedule: "activity"`) produce output that is stored alongside the activity records, organized by day and record ID:

```
facets/{facet}/activities/{day}/{activity_id}/{agent}.{ext}
```

For example, a `session_review` agent processing a coding activity would write to:
```
facets/work/activities/20260209/coding_095809_303/session_review.md
```

These output directories are only created when activity-scheduled agents run. The path is computed by `get_activity_output_path()` in `solstone/think/activities.py` and passed as `output_path` in the agent request. Output files are indexed for search via the `facets/*/activities/*/*/*.md` formatter pattern.

## Facet-Scoped Todos

Todos are organized by facet in `facets/{facet}/todos/{day}.jsonl` where each file stores todo items as JSON Lines. Todos belong to a specific facet (e.g., "personal", "work", "research") and are completely separated by scope.

**File path pattern:**
```
facets/personal/todos/20250110.jsonl
facets/work/todos/20250110.jsonl
facets/research/todos/20250112.jsonl
```

Each file contains one JSON object per line, with the line number (1-indexed) serving as the stable todo ID.

```jsonl
{"text": "Draft standup update"}
{"text": "Review PR #1234 for indexing tweaks", "time": "14:30"}
{"text": "Morning planning session notes", "completed": true}
{"text": "Cancel meeting with vendor", "cancelled": true}
```

## Format Specification

**JSONL structure:**

Each line is a JSON object with the following fields:
- `text` (required) – Task description
- `time` (optional) – Scheduled time in `HH:MM` format (e.g., `"14:30"`)
- `completed` (optional) – Set to `true` when task is done
- `cancelled` (optional) – Set to `true` for soft-deleted tasks
- `created_at` (optional) – Unix timestamp in milliseconds when todo was created
- `updated_at` (optional) – Unix timestamp in milliseconds of last modification

**Facet context:**
- Facet is determined by the file location, not inline tags
- Each facet has its own independent todo list for each day
- Work todos (`facets/work/todos/`) are completely separate from personal todos (`facets/personal/todos/`)

**Rules:**
- Line number is the stable todo ID (1-indexed); todos are never removed, only cancelled
- Append new todos at the end of the file to maintain stable line numbering
- Mark completed items with `"completed": true`
- Cancel items with `"cancelled": true` (soft delete preserves line numbers)

**Tool Access:**
All todo operations require both `day` and `facet` parameters:
- `todo_list(day, facet)` – view numbered checklist for a specific facet
- `todo_add(day, facet, text)` – append new todo
- `todo_done(day, facet, line_number)` – mark complete
- `todo_cancel(day, facet, line_number)` – cancel entry (soft delete)
- `todo_upcoming(limit, facet=None)` – view upcoming todos (optionally filtered by facet)

This facet-scoped structure provides true separation of concerns while enabling automated tools to manage tasks deterministically.
