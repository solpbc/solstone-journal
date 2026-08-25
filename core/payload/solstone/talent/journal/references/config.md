# Configuration

The optional `config/journal.json` file allows customization of journal processing and presentation based on owner preferences. This file should be created at the journal root and contains personal settings that affect how the system processes and interprets journal data.

## Identity configuration

The `identity` block contains information about the journal owner that helps tools correctly identify the owner in transcripts, meetings, and other captured content:

```json
{
  "identity": {
    "name": "Raelyn Brooks",
    "preferred": "Rae",
    "pronouns": {
      "subject": "he",
      "object": "him",
      "possessive": "his",
      "reflexive": "himself"
    },
    "aliases": ["Rae", "raylyn"],
    "email_addresses": ["rae@example.com"],
    "timezone": "America/Los_Angeles"
  }
}
```

Fields:
- `name` (string) – Full legal or formal name of the journal owner
- `preferred` (string) – Preferred name or nickname to be used when addressing the owner
- `pronouns` (object) – Structured pronoun set for template usage with fields:
  - `subject` – Subject pronoun (e.g., "he", "she", "they")
  - `object` – Object pronoun (e.g., "him", "her", "them")
  - `possessive` – Possessive adjective (e.g., "his", "her", "their")
  - `reflexive` – Reflexive pronoun (e.g., "himself", "herself", "themselves")
- `aliases` (array of strings) – Alternative names, nicknames, or usernames that may appear in transcripts
- `email_addresses` (array of strings) – Email addresses associated with the owner for participant detection
- `timezone` (string) – IANA timezone identifier (e.g., "America/New_York", "Europe/London") for timestamp interpretation

This configuration helps meeting extraction identify the owner as a participant, enables personalized agent interactions, and ensures timestamps are interpreted correctly across the journal.

## Convey configuration

The separate `config/convey.json` file stores UI/UX personalization (facet/app ordering, selected facet). All fields optional:

```json
{
  "facets": {"order": ["work", "personal"], "selected": "work"},
  "apps": {"order": ["home", "activities", "entities"]}
}
```

- `facets.order` – Custom facet ordering. `facets.selected` – Currently selected facet (auto-synced with browser).
- `apps.order` – Custom app ordering in menu bar.

## Retention configuration

The `retention` block controls when layer 1 raw media (audio recordings, video captures, screen diffs) becomes eligible for an owner-approved removal proposal, while preserving all layer 2 extracts and layer 3 agent outputs. A mark is a durable, non-destructive proposal; actual removal still requires the owner's approval. Three modes control eligibility for marking:

- `"keep"` – retain raw media indefinitely (the default)
- `"days"` – make raw media eligible for marking after `raw_media_days` days, once the segment has finished processing
- `"processed"` – make raw media eligible for marking as soon as the segment has finished processing

```json
{
  "retention": {
    "raw_media": "days",
    "raw_media_days": 30,
    "per_stream": {
      "plaud": {
        "raw_media": "days",
        "raw_media_days": 7
      },
      "archon": {
        "raw_media": "processed"
      }
    }
  }
}
```

Fields:
- `raw_media` (string) – Retention mode: `"keep"`, `"days"`, or `"processed"`. Default: `"keep"`.
- `raw_media_days` (integer or null) – Number of days before raw media is eligible for marking when mode is `"days"`. Default: `null`; a `days` rule needs a positive value to make media eligible, ignored otherwise.
- `per_stream` (object) – Per-stream overrides keyed by stream name. Each entry supports `raw_media` and `raw_media_days`. Omitted fields inherit from the global retention settings.

"Raw media" means layer 1 capture files only: audio files (`.flac`, `.opus`, `.ogg`, `.m4a`, `.wav`), video files (`.webm`, `.mov`, `.mp4`), and screen diffs (`monitor_*_diff.png`).

All layer 2 and layer 3 content is always preserved regardless of retention policy: transcripts (`audio.jsonl`, `screen.jsonl`), talent outputs (`talents/<name>.md` or `talents/<name>.json`, depending on the declared `output` format; JSON outputs are rendered to text through the formatter registry), speaker labels (`talents/speaker_labels.json`), historical facet events (`events/*.jsonl`), entity data, segment metadata (`stream.json`), and search index entries.

Raw media is not eligible for policy marking until its segment has finished processing. A segment is considered complete only when all four checks pass:

- No `_active.jsonl` files in `talents/` (no running talents)
- `audio.jsonl` (or `*_audio.jsonl`) exists if audio raw media was captured
- `screen.jsonl` (or `*_screen.jsonl`) exists if video raw media was captured
- `talents/speaker_labels.json` exists if voice embeddings (`.npz`) are present

Marking does not change segment navigability or audio/video playback. Transcripts, entities, speaker labels, and summaries remain intact.

## Environment variables

The `env` block stores configuration as environment variables that solstone loads into the process environment at CLI startup. This is where managed provider API keys live:

```json
{
  "env": {
    "GOOGLE_API_KEY": "your-google-api-key",
    "ANTHROPIC_API_KEY": "your-anthropic-api-key",
    "OPENAI_API_KEY": "your-openai-api-key",
    "PLAUD_ACCESS_TOKEN": "your-plaud-token"
  }
}
```

**Managed provider keys are journal-config-exclusive.** For the managed provider API keys — `GOOGLE_API_KEY`, `OPENAI_API_KEY`, and `ANTHROPIC_API_KEY` — the journal config `env` section is the authoritative and exclusive source. At CLI startup, solstone loads the `env` block into the environment and then strips any of these managed keys that is *not* set in journal config, so a value set only in the shell is never used. This keeps the journal config the single, predictable place that decides which provider keys are in effect (useful when the journal is synced across machines).

Other variables declared in the `env` block (for example `PLAUD_ACCESS_TOKEN`) are loaded into the environment at startup as well.

### Template usage examples

The structured pronoun format enables proper pronoun usage in generated text and agent responses:

```python
# In templates or generated text:
f"{identity.pronouns.subject} joined the meeting"  # "he joined the meeting"
f"I spoke with {identity.pronouns.object}"         # "I spoke with him"
f"That is {identity.pronouns.possessive} desk"     # "That is his desk"
f"{identity.pronouns.subject} did it {identity.pronouns.reflexive}"  # "he did it himself"
```

For complete documentation of the prompt template system including all variable categories, composition patterns, and how to add new variables, see [PROMPT_TEMPLATES.md](../../../docs/PROMPT_TEMPLATES.md).

## Transcribe configuration

The `transcribe` block configures audio transcription settings for `journal transcribe`:

```json
{
  "transcribe": {
    "backend": "parakeet",
    "preserve_all": false,
    "confidential_audio": true,
    "parakeet": {
      "model_version": "v3",
      "device": "auto",
      "timeout_sec": 120.0
    }
  }
}
```

**Top-level fields:**
- `backend` (string) – STT backend to use: `"parakeet"` (default local processing), `"parakeet-cpp"` (Linux-only local processing via a supervised parakeet.cpp server), or `"confidential"` (operated attested STT when the confidential lane is active). Default: `"parakeet"`.
- `preserve_all` (boolean) – Keep audio files even when no speech is detected. When `false`, silent recordings are deleted to save disk space. Default: `false`.
- `confidential_audio` (boolean) – Allow confidential hosted STT when the confidential lane is active. Absent means `true`; set to `false` to keep STT on local placement.

**Parakeet backend settings** (`transcribe.parakeet`):
- `model_version` (string) – Parakeet model version: `"v3"`. Default: `"v3"`.
- `device` (string) – Runtime preference for Parakeet: `"auto"`, `"cpu"`, or `"cuda"`. Default: `"auto"`.
- `timeout_sec` (number) – Helper/runtime timeout in seconds. Default: `120.0`.

**Parakeet.cpp backend settings** (`transcribe.parakeet-cpp`):
- `device` (string) – Runtime preference for the parakeet.cpp server: `"auto"` (use GPU if available, else CPU) or `"cpu"`. Default: `"auto"`.

Voice embeddings (wespeaker-resnet34) use CoreML with CPU fallback on Darwin and CPU-only elsewhere.

CLI flags can override settings: `--backend` selects the backend.

## Describe configuration

The `describe` block configures screen analysis settings for `journal describe`:

```json
{
  "describe": {
    "max_extractions": 20,
    "categories": {
      "code": {
        "importance": "high",
        "extraction": "Extract when viewing different repositories or files"
      },
      "gaming": {
        "importance": "ignore"
      }
    }
  }
}
```

**Fields:**
- `max_extractions` (integer) – Maximum number of frames to run detailed content extraction on per video. The first qualified frame is always extracted regardless of this limit. When more frames are eligible, selection uses AI-based prioritization (falling back to random selection). Default: `20`.
- `categories` (object) – Per-category overrides for importance and extraction guidance.
- `scene_cut_threshold` (integer) – Scene-change sensitivity for screen frame analysis. Frames at or above this perceptual-distance threshold bypass the stride floor. Default: `25`.
- `min_stride_seconds` (number) – Minimum seconds between analyzed frames when the frame is not a scene cut. Default: `5.0`.
- `max_concurrent` (integer) – Maximum concurrent screen describe jobs. Default: `1`.
- `max_runtime` (integer or duration string) – Wall-clock cap for each screen describe job, in seconds or a duration like `"30m"`. Default: `1800`.

`depict` (still-image description) is a sense handler with the same `max_concurrent` / `max_runtime` config shape. Its `max_runtime` default is `600` seconds.

### Category overrides

Each category (e.g., `code`, `meeting`, `browsing`, `social`) can have:

| Field | Values | Description |
|-------|--------|-------------|
| `importance` | `high`, `normal`, `low`, `ignore` | Advisory priority hint for AI frame selection. `high` prioritizes these frames, `low` deprioritizes unless unique, `ignore` suggests skipping unless categorization seems wrong. Default: `normal`. |
| `extraction` | string | Custom guidance for when to extract content from this category. Overrides the default from the category's `.md` frontmatter. |

Importance levels are advisory hints passed to the AI selection process, not hard filters. The AI may still select frames from `ignore` categories if it determines the content is valuable or the categorization may be incorrect.

## Providers configuration

Brain choice is managed in the Thinking app. Your journal stores that choice in
`journal/config/journal.json` under `providers`.

```json
{
  "env": {
    "GOOGLE_API_KEY": "stored-by-settings"
  },
  "providers": {
    "active": {
      "provider": "google",
      "model": "gemini-3.5-flash"
    },
    "local": {
      "endpoint_url": "http://127.0.0.1:8080",
      "served_model_id": "local-model-name",
      "parallel_slots": 2
    }
  }
}
```

### Live provider keys

**`providers.active`** controls the single brain used by both generate and
cogitate requests. It contains:
- `provider` – `"google"`, `"anthropic"`, `"openai"`, `"local"`, or omitted.
- `model` – explicit model id for that provider.

There is no implicit key-based selection or fallback. A missing or invalid
active profile is the no-brain state.

**`env`** stores managed cloud API keys:
- `GOOGLE_API_KEY`
- `ANTHROPIC_API_KEY`
- `OPENAI_API_KEY`

**`providers.local`** configures a local OpenAI-compatible endpoint. The endpoint
is active only when both fields are present:
- `endpoint_url`
- `served_model_id`

Optional local endpoint fields:
- `credential`
- `parallel_slots`

### Talent overrides

`talent_overrides.<talent>.disabled` and `talent_overrides.<talent>.extract`
are optional talent metadata. Provider and model overrides are rejected.
