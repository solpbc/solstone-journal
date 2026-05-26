# Observe Module

Multimodal capture and AI-powered analysis of desktop activity.

## Observer Architecture

Observers are independent capture agents that upload segments to solstone via the HTTP ingest API (`/app/observer/ingest/<key>`). Each observer runs as its own process with its own lifecycle — solstone core is the journal + processing engine.

| Observer | What it captures | Repo | Runs as |
|----------|-----------------|------|---------|
| **solstone-linux** | Screen + audio on Linux | `solstone-linux` | systemd user service / standalone |
| **solstone-macos** | Screen + audio on macOS | `solstone-macos` | Native menu bar app |
| **solstone-tmux** | Tmux terminal sessions | `solstone-tmux` | systemd user service / standalone |

### Managing observers

```bash
# List all registered observers
sol observer list

# Register a new observer
sol observer create <name>

# Check observer status
sol observer status <name>

# Rename an observer
sol observer rename <old> <new>

# Revoke an observer's key
sol observer revoke <name>
```

## Commands

| Command | Purpose |
|---------|---------|
| `sol observer` | Screen and audio capture (auto-detects platform) |
| `sol observe-linux` | Screen and audio capture on Linux (direct) |
| `journal transcribe` | Audio transcription with faster-whisper |
| `journal describe` | Visual analysis of screen recordings |
| `journal grab` | Walk available screen frames and optionally write frame images |
| `journal sense` | Unified observation coordination |

## Architecture

```
Observers (standalone or built-in)
       ↓ HTTP multipart upload
Observer Ingest API (/app/observer/ingest/<key>)
       ↓
   Raw media files (*.flac, *.webm, tmux_*.jsonl)
       ↓
journal sense (coordination)
   ├── journal transcribe → audio.jsonl
   └── journal describe → screen.jsonl
```

## Linux Observer State Machine

The Linux observer operates in two modes based on desktop activity:

```
SCREENCAST  ←→  IDLE
```

| Mode | Trigger | Captures |
|------|---------|----------|
| SCREENCAST | Screen active (not idle/locked/power-save) | Video + Audio |
| IDLE | Screen idle, locked, or power-save | Audio only (if threshold met) |

**Segment boundaries** are triggered by:
- Transitions between SCREENCAST and IDLE modes
- Mute state changes
- 5-minute window elapsed

## Key Components

- **observer.py** — Unified entry point with platform detection
- **linux/observer.py** — Linux capture: audio + screencast + activity detection
- **linux/screencast.py** — XDG Portal screencast with PipeWire + GStreamer
- **gnome/activity.py** — GNOME-specific activity detection (idle, lock, power save)
- **observer_client.py** — HTTP upload client for observer → server communication
- **sense.py** — File watcher that dispatches transcription and description jobs
- **transcribe.py** — Audio transcription with faster-whisper and sentence-level embeddings
- **describe.py** — Vision analysis with Gemini, category-based prompts
- **categories/** — Category-specific prompts for screen content (see [SCREEN_CATEGORIES.md](SCREEN_CATEGORIES.md))

## Standalone Observers

**Tmux capture** is handled by the `solstone-tmux` package, which runs as its own systemd user service. See `solstone-tmux` repo for setup instructions.

**macOS capture** is handled by the `solstone-macos` native Swift app. See `solstone-macos` repo.

Both upload segments via the same HTTP ingest API used by the built-in Linux observer.

## Output Formats

See [captures.md](../talent/journal/references/captures.md) for detailed extract schemas:
- Audio transcripts: `audio.jsonl` with timestamps (speaker detection not included)
- Screen analysis: `screen.jsonl` with frame-by-frame categorization

## Configuration

Requires the journal directory at project root. API keys for transcription/vision services configured in `.env`.
