# Configuration

The optional `config/journal.json` file allows customization of journal processing and presentation based on owner preferences. This file should be created at the journal root and contains personal settings that affect how the system processes and interprets journal data.

## Identity configuration

The `identity` block contains information about the journal owner that helps tools correctly identify the owner in transcripts, meetings, and other captured content:

```json
{
  "identity": {
    "name": "Jeremie Miller",
    "preferred": "Jer",
    "pronouns": {
      "subject": "he",
      "object": "him",
      "possessive": "his",
      "reflexive": "himself"
    },
    "aliases": ["Jer", "jeremie"],
    "email_addresses": ["jer@example.com"],
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

The `convey` block contains settings for the web application:

```json
{
  "convey": {
    "password_hash": "<set via Settings > Security or journal password set>"
  }
}
```

Fields:
- `password_hash` (string) – Hashed password for accessing the convey web application. Set via Settings → Security or `journal password set`.

**UI Preferences:** The separate `config/convey.json` file stores UI/UX personalization (facet/app ordering, selected facet). All fields optional:

```json
{
  "facets": {"order": ["work", "personal"], "selected": "work"},
  "apps": {"order": ["home", "activities", "todos"], "starred": ["home", "todos"]}
}
```

- `facets.order` – Custom facet ordering. `facets.selected` – Currently selected facet (auto-synced with browser).
- `apps.order` – Custom app ordering in menu bar.
- `apps.starred` – Apps to show in the quick-access starred section.

## Retention configuration

The `retention` block controls automatic cleanup of layer 1 raw media (audio recordings, video captures, screen diffs) while preserving all layer 2 extracts and layer 3 agent outputs. Three modes control when raw media is deleted:

- `"keep"` – retain raw media indefinitely
- `"days"` – delete raw media after `raw_media_days` days, once the segment has finished processing (default: 7 days)
- `"processed"` – delete raw media as soon as the segment has finished processing

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
- `raw_media` (string) – Retention mode: `"keep"`, `"days"`, or `"processed"`. Default: `"days"`.
- `raw_media_days` (integer or null) – Number of days to retain raw media when mode is `"days"`. Default: `7`. Required when `raw_media` is `"days"`, ignored otherwise.
- `per_stream` (object) – Per-stream overrides keyed by stream name. Each entry supports `raw_media` and `raw_media_days`. Omitted fields inherit from the global retention settings.

"Raw media" means layer 1 capture files only: audio files (`.flac`, `.opus`, `.ogg`, `.m4a`, `.wav`), video files (`.webm`, `.mov`, `.mp4`), and screen diffs (`monitor_*_diff.png`).

All layer 2 and layer 3 content is always preserved regardless of retention policy: transcripts (`audio.jsonl`, `screen.jsonl`), talent outputs (`talents/*.md`), speaker labels (`talents/speaker_labels.json`), historical facet events (`events/*.jsonl`), entity data, segment metadata (`stream.json`), and search index entries.

Raw media is never deleted from segments that haven't finished processing. A segment is considered complete only when all four checks pass:

- No `_active.jsonl` files in `talents/` (no running talents)
- `audio.jsonl` (or `*_audio.jsonl`) exists if audio raw media was captured
- `screen.jsonl` (or `*_screen.jsonl`) exists if video raw media was captured
- `talents/speaker_labels.json` exists if voice embeddings (`.npz`) are present

Purged segments remain fully navigable in convey. Transcripts, entities, speaker labels, and summaries are all intact. The only difference is that audio/video playback is unavailable.

## Environment variables

The `env` block provides fallback values for environment variables. These are loaded at CLI startup and used when the corresponding variable is not set in the shell or `.env` file:

```json
{
  "env": {
    "GOOGLE_API_KEY": "your-google-api-key",
    "ANTHROPIC_API_KEY": "your-anthropic-api-key",
    "OPENAI_API_KEY": "your-openai-api-key",
    "REVAI_ACCESS_TOKEN": "your-revai-token",
    "PLAUD_ACCESS_TOKEN": "your-plaud-token"
  }
}
```

**Precedence order** (highest to lowest):
1. Shell environment variables
2. `.env` file in project root
3. Journal config `env` section

This allows storing API keys in the journal config as an alternative to `.env`, which can be useful when the journal is synced across machines or when you want to keep all configuration in one place.

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
    "enrich": true,
    "preserve_all": false,
    "noise_upgrade_min_speech_ratio": 0.3,
    "parakeet": {
      "model_version": "v3",
      "device": "auto",
      "timeout_sec": 120.0
    },
    "whisper": {
      "device": "auto",
      "model": "medium.en",
      "compute_type": "default"
    },
    "revai": {
      "model": "fusion"
    }
  }
}
```

**Top-level fields:**
- `backend` (string) – STT backend to use: `"parakeet"` (default local processing), `"whisper"` (local rollback path), `"revai"` (cloud with speaker diarization), or `"gemini"` (cloud with speaker diarization). Default: `"parakeet"`.
- `enrich` (boolean) – Enable LLM enrichment for topic extraction and transcript correction. Default: `true`.
- `preserve_all` (boolean) – Keep audio files even when no speech is detected. When `false`, silent recordings are deleted to save disk space. Default: `false`.
- `noise_upgrade_min_speech_ratio` (number) – Min speech/loud ratio required for noisy upgrade (default: `0.3`). Filters out music and other non-speech noise.

**Parakeet backend settings** (`transcribe.parakeet`):
- `model_version` (string) – Parakeet model version: `"v3"`. Default: `"v3"`.
- `device` (string) – Runtime preference for Parakeet: `"auto"`, `"cpu"`, or `"cuda"`. Default: `"auto"`.
- `timeout_sec` (number) – Helper/runtime timeout in seconds. Default: `120.0`.

**Whisper backend settings** (`transcribe.whisper`):
- `device` (string) – Device for inference: `"auto"` (detect GPU, fall back to CPU), `"cpu"`, or `"cuda"`. Default: `"auto"`.
- `model` (string) – Whisper model to use (e.g., `"tiny.en"`, `"base.en"`, `"small.en"`, `"medium.en"`, `"large-v3-turbo"`, `"distil-large-v3"`). Default: `"medium.en"`.
- `compute_type` (string) – Compute precision: `"default"` (auto-select optimal for platform), `"float32"` (most compatible), `"float16"` (faster on CUDA GPUs), `"int8"` (fastest on CPU). Default: `"default"`.

**Rev.ai backend settings** (`transcribe.revai`):
- `model` (string) – Rev.ai transcriber model: `"fusion"` (best quality), `"machine"` (fast automated), or `"low_cost"`. Default: `"fusion"`.

**Platform auto-detection** (Whisper): When `compute_type` is `"default"`, optimal settings are automatically selected:
- **CUDA GPU**: Uses `float16` for GPU-optimized inference
- **CPU (including Apple Silicon)**: Uses `int8` for ~2x faster inference and significantly faster model loading

Voice embeddings (wespeaker-resnet34) use CoreML with CPU fallback on Darwin and CPU-only elsewhere.

CLI flags can override settings: `--backend` selects the backend, `--cpu` forces CPU mode with int8 (Whisper only), `--model MODEL` overrides the Whisper model.

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

### Category overrides

Each category (e.g., `code`, `meeting`, `browsing`) can have:

| Field | Values | Description |
|-------|--------|-------------|
| `importance` | `high`, `normal`, `low`, `ignore` | Advisory priority hint for AI frame selection. `high` prioritizes these frames, `low` deprioritizes unless unique, `ignore` suggests skipping unless categorization seems wrong. Default: `normal`. |
| `extraction` | string | Custom guidance for when to extract content from this category. Overrides the default from the category's `.json` file. |

Importance levels are advisory hints passed to the AI selection process, not hard filters. The AI may still select frames from `ignore` categories if it determines the content is valuable or the categorization may be incorrect.

## Providers configuration

The `providers` block enables fine-grained control over which LLM provider and model is used for different contexts. This supports a tier-based system where you can specify capability levels (pro/flash/lite) rather than specific model names.

```json
{
  "providers": {
    "default": {
      "provider": "google",
      "tier": 2
    },
    "contexts": {
      "observe.*": {"provider": "google", "tier": 3},
      "talent.system.*": {"tier": 1},
      "talent.system.conversation": {"provider": "anthropic", "disabled": true},
      "talent.entities.observer": {"tier": 2}
    },
    "models": {
      "google": {
        "1": "gemini-3-pro-preview",
        "2": "gemini-3-flash-preview",
        "3": "gemini-2.5-flash-lite"
      }
    }
  }
}
```

### Tier system

Tiers provide a provider-agnostic way to specify model capability levels:

| Tier | Name  | Description |
|------|-------|-------------|
| 1    | pro   | Highest capability, best for complex reasoning |
| 2    | flash | Balanced performance and cost (default) |
| 3    | lite  | Fastest and cheapest, for simple tasks |

System defaults map tiers to models for each provider. See `solstone/think/models.py` for current tier-to-model mappings (`PROVIDER_DEFAULTS` constant).

If a requested tier is unavailable for a provider, the system falls back to more capable tiers (e.g., tier 3 → tier 2 → tier 1).

### Context matching

Contexts are matched in order of specificity:
1. **Exact match** – `"talent.system.conversation"` matches only that exact context
2. **Glob pattern** – `"observe.*"` matches any context starting with `observe.`
3. **Default** – Falls back to the `default` configuration

### Context naming convention

Talent configs (agents and generators) use the pattern `talent.{source}.{name}`:
- System configs: `talent.system.{name}` (e.g., `talent.system.conversation`, `talent.system.default`)
- App configs: `talent.{app}.{name}` (e.g., `talent.entities.observer`, `talent.support.support`)

Other contexts follow the pattern `{module}.{feature}[.{operation}]`:
- Observe pipeline: `observe.describe.frame`, `observe.enrich`, `observe.transcribe.gemini`

### Configuration options

**default** – Global defaults applied when no context matches:
- `provider` (string) – Provider name: `"google"`, `"openai"`, or `"anthropic"`. Default: `"google"`.
- `tier` (integer) – Tier number (1-3). Default: `2` (flash).
- `model` (string) – Explicit model name (overrides tier if specified).

**contexts** – Context-specific overrides. Each key is a context pattern, value is:
- `provider` (string) – Override provider (optional, inherits from default).
- `tier` (integer) – Tier number (optional).
- `model` (string) – Explicit model name (optional, overrides tier).
- `disabled` (boolean) – Disable this talent config (optional, talent contexts only).

**models** – Per-provider tier overrides. Maps provider name to tier-model mappings:
```json
{
  "google": {"1": "gemini-3-pro-preview", "2": "gemini-3-flash-preview"},
  "openai": {"2": "gpt-5-mini-custom"}
}
```

Note: Tier keys in JSON must be strings (`"1"`, `"2"`, `"3"`) since JSON doesn't support integer keys.
