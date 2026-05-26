# Captures and Extracts

## The Three-Layer Architecture

solstone transforms raw recordings into actionable understanding through a three-layer pipeline:

```
┌─────────────────────────────────────┐
│  LAYER 3: AGENT OUTPUTS             │  Narrative summaries
│  (Markdown files)                   │  "What it means"
│  - talents/*.md (daily outputs)     │
│  - *.md (segment outputs)           │
└─────────────────────────────────────┘
         ↑ synthesized from
┌─────────────────────────────────────┐
│  LAYER 2: EXTRACTS                  │  Structured data
│  (JSON/JSONL files)                 │  "What happened"
│  - audio.jsonl, *_audio.jsonl       │
│  - screen.jsonl, *_screen.jsonl     │
│  - events/*.jsonl (historical)      │
└─────────────────────────────────────┘
         ↑ derived from
┌─────────────────────────────────────┐
│  LAYER 1: CAPTURES                  │  Raw recordings
│  (Binary media files)               │  "What was recorded"
│  - *.flac, *.ogg, *.opus, *.wav (audio)    │
│  - *.webm (video)                   │
└─────────────────────────────────────┘
```

### Vocabulary Quick Reference

**Pipeline Layers**

| Term | Definition | Examples |
|------|------------|----------|
| **Capture** | Raw audio/video recording | `*.flac`, `*.ogg`, `*.opus`, `*.wav`, `*.webm` |
| **Extract** | Structured data from captures | `*.jsonl` |
| **Agent Output** | AI-generated narrative summary | `talents/*.md`, `HHMMSS_LEN/*.md` |

**Organization**

| Term | Definition | Examples |
|------|------------|----------|
| **Day** | 24-hour activity directory | `20250119/` |
| **Segment** | 5-minute time window | `143022_300/` (14:30:22, 5 min) |
| **Span** | Sequential segment group | Import creating 3 segments |
| **Facet** | Project/context scope | `#work`, `#personal` |

**Extracted Data**

| Term | Definition | Examples |
|------|------------|----------|
| **Entity** | Tracked person/project/concept | People, companies, tools |
| **Occurrence** | Historical term; occurrence hook retired 2026-04-18 Sprint 4. Produced `facets/{facet}/events/{day}.jsonl`, still searchable via `search_journal(agent="event")`. | Meetings, messages, files |

## Imported Audio

The `imports/` directory stores audio files imported via the import app, along with their processing artifacts. Each import is organized by detected timestamp:

```
imports/
  └── YYYYMMDD_HHMMSS/           # Import directory (detected or owner-specified timestamp)
      ├── import.json            # Import metadata and processing status
      ├── {original_filename}    # Original uploaded audio file
      ├── imported.json          # Processed transcript in standard format
      └── segments.json          # List of segment keys created for this import
```

### Import metadata

The `import.json` file tracks the import process:

```json
{
  "original_filename": "meeting_recording.m4a",
  "upload_timestamp": 1755034698276,
  "upload_datetime": "2025-08-12T15:38:18.276000",
  "detection_result": {
    "day": "20250630",
    "time": "143256",
    "confidence": "high",
    "source": "Date/Time Original"
  },
  "detected_timestamp": "20250630_143256",
  "user_timestamp": "20250630_143256",
  "file_size": 13950943,
  "mime_type": "audio/x-m4a",
  "facet": "work",
  "processing_completed": "2025-08-12T15:41:42.970189"
}
```

Once processed, imports are linked into the appropriate day's segment via `imported_audio.jsonl` files that reference the original import location.

## Day folder contents

Within each day, captured content is organized into **segments** (timestamped duration folders). The folder name is the **segment key**, which uniquely identifies the segment within the day and follows this format:

- `HHMMSS_LEN/` – Start time and duration in seconds (e.g., `143022_300/` for a 5-minute segment starting at 14:30:22)

Each segment progresses through the three-layer pipeline: captures are recorded, extracts are generated, and agent outputs are synthesized.

### Stream identity

Every segment belongs to a **stream** — a named series of segments from a single source. Streams provide navigable chains linking each segment to its predecessor.

- `stream.json` – Per-segment stream marker containing:
  - `stream` – stream name (e.g., `"archon"`, `"import.apple"`)
  - `prev_day` – day of the previous segment in this stream (null for first)
  - `prev_segment` – segment key of the predecessor (null for first)
  - `seq` – sequence number within the stream

Stream names follow the convention: `{hostname}` for local observers, `{observer_name}` for observers, `import.{type}` for imports (e.g., `import.apple`, `import.text`). Global stream state is tracked in the top-level `streams/` directory as `{name}.json` files.

Pre-stream segments (created before stream identity was added) have no `stream.json` and are handled gracefully as `None` throughout the pipeline.

## Layer 1: Captures

Captures are the original binary media files recorded by observation tools.

`journal grab` walks observed screens from day to stream to segment to screen to frame.
Without `--out` it lists what is available or shows one frame's details.
With `--out` it writes one or more frame images using the suffix you choose.
Use bare `screen` for single-screen segments.
Use stems like `center_DP-3_screen` for per-monitor segments.

### Audio captures

Audio files are initially written to the day root with the segment key prefix (Linux) or directly to segment folders (macOS):

- **Linux**: `HHMMSS_LEN_*.flac` – audio files in day root (e.g., `143022_300_audio.flac`)
- **macOS**: `HHMMSS_LEN/audio.m4a` – audio files written directly to segment folder

After transcription, audio files are moved into their segment folder:

- `HHMMSS_LEN/*.flac`, `*.m4a`, `*.ogg`, `*.opus`, or `*.wav` – audio files moved here after processing, preserving descriptive suffix (e.g., `audio.flac`, `audio.m4a`, `imported_audio.opus`)

Note: The descriptive portion after the segment key (e.g., `_audio`, `_recording`) is preserved when files are moved into segment directories. Processing tools match files by extension only, ignoring the descriptive suffix.

### Screen captures

Screen recordings use per-monitor files with position and connector/displayID in the filename:

- **Linux**: `HHMMSS_LEN_<position>_<connector>_screen.webm` – screencast video files in day root (e.g., `143022_300_center_DP-3_screen.webm`)
- **macOS**: `HHMMSS_LEN/<position>_<displayID>_screen.mov` – video files written directly to segment folder (e.g., `center_1_screen.mov`)

After analysis, files are in their segment folder:

- `HHMMSS_LEN/<position>_<connector>_screen.webm` or `*.mov` – video files (e.g., `center_DP-3_screen.webm`, `center_1_screen.mov`)

For multi-monitor setups, each monitor produces a separate file. Position labels include: `center`, `left`, `right`, `top`, `bottom`, and combinations like `left-top`.

## Layer 2: Extracts

Extracts are structured data files (JSON/JSONL) derived from captures through AI analysis.

### Audio transcript extracts

The transcript file (`audio.jsonl`) contains a metadata line followed by one JSON object per transcript segment.

Example transcript file:

```jsonl
{"raw": "audio.flac"}
{"start": "00:00:01", "source": "mic", "text": "So we need to finalize the authentication module today."}
{"start": "00:00:15", "source": "sys", "text": "I agree. Let's make sure we have proper unit tests."}
```

**Metadata line (first line):**
- `raw` – path to processed audio file (required)
- `backend` – STT backend used (e.g., "whisper", "revai")
- `model` – model used for transcription (e.g., "medium.en", "revai-fusion")
- `device` – device used for inference (e.g., "cuda", "cpu", "cloud")
- `compute_type` – compute precision used (e.g., "float16", "int8", "api")
- `observer` – observer name if transcribed from an observer source (optional)
- `imported` – object with import metadata for external files (optional):
  - `id` – unique import identifier
  - `facet` – facet name for entity extraction
  - `setting` – contextual setting description

**Transcript statements (subsequent lines):**
- `start` – timestamp in HH:MM:SS format (required)
- `text` – transcribed text (required)
- `source` – audio source: "mic" or "sys" (optional)
- `speaker` – speaker identifier, numeric or string (optional, not currently populated)
- `corrected` – LLM-corrected version of text (optional, added during enrichment)
- `description` – tone or delivery description, e.g., "enthusiastic", "questioning" (optional, added during enrichment)

### Screen frame extracts

Screen analysis files use per-monitor naming: `<position>_<connector>_screen.jsonl` (e.g., `center_DP-3_screen.jsonl`, `left_HDMI-1_screen.jsonl`). For single-monitor setups, the file is simply `screen.jsonl`. Each file contains one JSON object per qualified frame. Frames qualify when they show significant visual change (≥5% RMS difference) compared to the previous qualified frame.

Example frame record:

```json
{
  "frame_id": 123,
  "timestamp": 45.67,
  "requests": [
    {"type": "describe", "model": "gemini-2.5-flash-lite", "duration": 0.5},
    {"type": "category", "category": "reading", "model": "gemini-3-flash", "duration": 1.2}
  ],
  "analysis": {
    "visual_description": "Documentation page showing API reference.",
    "primary": "reading",
    "secondary": "none",
    "overlap": true
  },
  "content": {
    "reading": "# API Reference\n\n## Authentication\n\nUse Bearer tokens..."
  }
}
```

**Common fields:**
- `frame_id` – sequential frame number in the video
- `timestamp` – time in seconds from video start
- `requests` – list of vision API requests made for this frame (type: "describe" for initial, "category" for follow-ups)
- `analysis` – categorization result with `primary`, `secondary`, `overlap`, and `visual_description`
- `content` – object containing category-specific extracted content (see below)
- `error` – present when processing failed after retries

**Category-specific content (inside `content` object):**
- `messaging` – markdown content when frame contains chat/email apps
- `browsing` – markdown content when frame contains web browsing
- `reading` – markdown content when frame contains documents/articles
- `productivity` – markdown content when frame contains spreadsheets/slides/calendars
- `meeting` – JSON object when frame contains video conferencing, includes participant detection and bounding boxes

The vision analysis uses multi-stage conditional processing:
1. Initial categorization determines content type (e.g., `code`, `meeting`, `browsing`, `reading`). See `solstone/observe/categories/` for the full list of categories.
2. Category-specific follow-up prompts are discovered from `solstone/observe/categories/*.md` files
3. Follow-ups are triggered for categories that have extraction content in their `.md` file (currently: messaging, browsing, reading, productivity output markdown; meeting outputs JSON)

### Historical event extracts

The retired occurrence hook previously extracted time-based events from the day's transcripts—meetings, messages, follow-ups, file activity, and more. Those historical event rows were stored per-facet in JSONL files at `facets/{facet}/events/{day}.jsonl`. These files persist for historical search via `search_journal(agent="event")`. Live future scheduled items are stored separately as anticipated activity records under `facets/{facet}/activities/{target_day}.jsonl` with `source: "anticipated"`.

```jsonl
{"type": "meeting", "start": "09:00:00", "end": "09:30:00", "title": "Team stand-up", "summary": "Status update with the engineering team", "work": true, "participants": ["Jeremie Miller", "Alice", "Bob"], "facet": "work", "agent": "meetings", "occurred": true, "source": "20250101/talents/meetings.md", "details": "Sprint planning discussion"}
```

**Common historical fields:**
- **type** – event kind such as `meeting`, `message`, `file`, `followup`, `documentation`, `research`, `media`, `deadline`, or `appointment`
- **start** and **end** – HH:MM:SS timestamps (or `null` when a time was not known)
- **title** and **summary** – short text for display and search
- **facet** – facet name the event belonged to
- **agent** – source generator type for the historical event row (for example `"meetings"`)
- **occurred** – `true` for historical event rows in `facets/*/events/*.jsonl`
- **source** – path to the output file that generated the event row
- **work** – boolean, work vs. personal classification
- **participants** – optional list of people or entities involved
- **details** – free-form string with additional context

These persisted historical files still allow the indexer to collect and search event rows across all facets and days.

## Layer 3: Agent Outputs

Agent outputs are AI-generated markdown files that provide human-readable narratives synthesized from captures and extracts.

### Segment outputs

After captures are processed, segment-level outputs are generated within each segment folder as `HHMMSS_LEN/*.md` files. Available segment output types are defined by templates in `solstone/talent/` with `"schedule": "segment"` in their metadata JSON.

### Daily outputs

Post-processing generates day-level outputs in the `talents/` directory that synthesize all segments.

**Generator discovery:** Available generator types are discovered at runtime from:
- `solstone/talent/*.md` – system generator templates (files with `schedule` field but no `tools` field)
- `solstone/apps/{app}/talent/*.md` – app-specific generator templates

Each template is a `.md` file with JSON frontmatter containing metadata (title, description, schedule, output format). The `schedule` field is required and must be `"segment"` or `"daily"` - generators with missing or invalid schedule are skipped. Use `get_talent_configs(has_tools=False)` from `solstone/think/talent.py` to retrieve all available generators, or `get_talent_configs(has_tools=False, schedule="daily")` to get generators filtered by schedule.

**Output naming:**
- System outputs: `talents/{agent}.md` (e.g., `talents/briefing.md`, `talents/default.md`)
- App outputs: `talents/_{app}_{agent}.md` (e.g., `talents/_entities_observer.md`)
- JSON output: `talents/{agent}.json` when metadata specifies `"output": "json"`
- Story fields (`story`, `commitments`, `closures`, `decisions`) live on the activity record in `facets/{facet}/activities/{day}.jsonl`

Each generator type has a corresponding template file (`{name}.md`) that defines how the AI synthesizes extracts into narrative form.
