# Observe Module

Multimodal capture and AI-powered analysis of desktop activity.

## Observer Architecture

Observers are independent capture agents that upload segments to solstone via `POST /app/observer/ingest`; the observer key/handle rides in the `X-Solstone-Observer` header (falling back to `Authorization: Bearer <handle>`), not the URL path. Each observer runs as its own process with its own lifecycle — solstone core is the journal + processing engine.

| Observer | What it captures | Repo | Runs as |
|----------|-----------------|------|---------|
| **solstone-linux** | Screen + audio on Linux | `solstone-linux` | systemd user service / standalone |
| **solstone-macos** | Screen + audio on macOS | `solstone-macos` | Native menu bar app |
| **solstone-tmux** | Tmux terminal sessions | `solstone-tmux` | systemd user service / standalone |

### Managing observers

```bash
# List all registered observers
journal observer list

# Check observer status
journal observer status <name>

# Rename an observer
journal observer rename <old> <new>

# Revoke an observer's key
journal observer revoke <name>
```

## Commands

| Command | Purpose |
|---------|---------|
| `journal observer` | Manage observer registrations (see "Managing observers" above) |
| `journal observer prune` | Dry-run or execute safe cleanup of duplicate observer segments |
| `journal transcribe` | Audio transcription with faster-whisper |
| `journal describe` | Visual analysis of screen recordings |
| `journal grab` | Walk available screen frames and optionally write frame images |
| `journal sense` | Unified observation coordination |

## Architecture

```
Observers (standalone, per-platform repos)
       ↓ HTTP multipart upload
Observer Ingest API (POST /app/observer/ingest — key via X-Solstone-Observer header)
       ↓
   Raw media files (*.flac, *.webm, tmux_*.jsonl)
       ↓
journal sense (coordination)
   ├── journal transcribe → audio.jsonl
   └── journal describe → screen.jsonl
```

## Key Components

Capture components (screen/audio grab, platform activity detection, the upload
client) live in the per-platform observer repos (`solstone-linux`,
`solstone-macos`, `solstone-tmux`) — see the Observer Architecture table above.
What remains in this package is the home-side ingest-and-processing pipeline:

- **sense.py** — File watcher that dispatches transcription and description jobs
- **transcribe/** — Audio transcription with native speaker-analysis embeddings. Exit-code contract, retry/deferral semantics, and the `observe.transcribed` field table: [transcribe-failure-and-telemetry.md](transcribe-failure-and-telemetry.md)
- **solstone-core-describe** — Native vision analysis helper reached via `journal describe`; the Python reference is retired
- **categories/** — Category-specific prompts for screen content (see [SCREEN_CATEGORIES.md](SCREEN_CATEGORIES.md))

### Vision input sizing

Image sizing is phase- and runtime-specific. The application never enlarges an
input image.

| Path | Bundled Qwen sizing | Other providers/platforms |
|---|---|---|
| Frame categorization (`observe.describe.frame`) | 1024 image-token area ceiling, with the standing 1920px longest-side ceiling | standing 1920px ceiling |
| Category extraction (`observe.describe.<category>`) | standing 1920px ceiling | standing 1920px ceiling |
| Still depiction (`observe.depict`) | standing 1920px ceiling | standing 1920px ceiling |
| Image/document import vision | model preprocessor defaults | model preprocessor defaults |

The [native describe pipeline](../core/crates/solstone-core-describe/src/pipeline.rs)
applies the 1024 categorization ceiling only to the bundled Qwen/llama.cpp
path. Configured BYO OpenAI-compatible endpoints retain their existing
preprocessing. Detailed extraction retains the 1920px longest-side ceiling.

## Standalone Observers

Each observer is a standalone package in its own repo (see the Observer Architecture table above), with its own capture internals and lifecycle:

- **`solstone-linux`** — screen + audio capture on Linux; runs as a systemd user service.
- **`solstone-macos`** — screen + audio capture on macOS; native Swift menu-bar app.
- **`solstone-tmux`** — tmux terminal-session capture; runs as a systemd user service.

All upload segments via the same HTTP ingest API (`POST /app/observer/ingest`), with the observer key/handle carried in `X-Solstone-Observer` (or `Authorization: Bearer <handle>` fallback), not the URL.

Observer ingest derives duplicate identity from the journal segment directory on
disk, not from an append-only history index. For an upload, the server looks
under `chronicle/<day>/<stream>/` for segment directories sharing the requested
`HHMMSS` start, checks the exact requested key first, then checks the remaining
candidates lexicographically. The content set is the uploaded audio/video files
when the bundle has any; otherwise it is the uploaded non-reserved files, so
tmux-style JSONL-only bundles never match on an empty media set.

Reserved segment markers, including `stream.json` and `ingest.json`, are
journal-authored. If a client includes those names in a bundle, the bytes are
validated when covered by the journal contract, but they are not written from the
client payload and are recorded in observer history as received-not-written.
Segment listings filter those audit-only records so clients never treat
journal-authored marker files as proof that their own marker bytes are held.

Every resolution into an existing candidate appends observer history, including
`duplicate`. That audit record is what lets `/app/observer/ingest/segments/<day>`
corroborate the duplicate for clients that confirm before deleting local files,
including segments that were originally created by import or transfer rather
than observer ingest.

Segment listings report each uploaded file as `present`, `processed`, or
`missing`. `present` means the recorded file still exists at its exact path.
`processed` applies only to raw audio/video media whose recorded path is absent
but whose same-stem JSONL sidecar at that segment path carries a terminal
`solstone.processing.v1` proof for the original input size. Legacy segments
without `ingest.json` use that proof to dedupe absent raw media, then graduate to
a manifest on the next resolution. Anything else is `missing` and remains
eligible for upload healing.

### Duplicate observer pruning

`journal observer prune [--day YYYYMMDD | --day-range A..B | --all] [--stream NAME] [--execute] [--cross-start]`
finds byte-identical duplicate observer segments from the old ingest suffix-ladder
defect. Dry-run is the default and performs zero writes: no manifest healing, no
history append, no index deletion, and no health marker touch. `--execute`
re-derives groups, canonical held-ness, per-file hashes, and observer attribution
from disk before deleting anything; dry-run output is advisory only.

Duplicate groups are restricted to one `(day, stream, HHMMSS start)` candidate
set. This matches the ingest planner's `HHMMSS_300`, `HHMMSS_301`, ...
collision ladder and prevents data loss from grouping unrelated windows that
happen to have identical bytes, such as two silent captures at different times.
Within that same-start set, identity is the set of `(name, sha256, size)` content
files: valid `ingest.json` files define content exactly; legacy manifest-less
segments use present media files; manifest-less non-media-only segments refuse.
The canonical is deterministic: the earliest same-start segment whose content
is held by present bytes or terminal processing proof.

`--cross-start` is opt-in. After same-start planning or execution, it also
considers different-start candidates proven by server-authored
`segment_original` provenance in observer upload history. The named origin is
resolved through existing pruned history to a surviving canonical, and the same
content, chain, held-ness, and observer-attribution gates apply.

Prune fails closed. It refuses unverifiable canonicals, near-duplicates, unknown
non-derived files, marker-less candidates, and ambiguous stream-to-observer
attribution. Recognized derived outputs are same-stem media sidecars, `events.jsonl`,
`timeline.json`, and files under `talents/`. A proof-held canonical is allowed:
when a canonical holds media only by terminal processing proof and a candidate is
the last physical copy, the CLI marks that candidate as `last-physical-copy` in
both dry-run and execute output and includes a summary count.

Execute deletes index rows for each pruned segment, repairs stream-chain
predecessors atomically on surviving `stream.json` markers, preserves stream
state metadata and monotonic `seq`, and touches `chronicle/<day>/health/stream.updated`.
It never renumbers survivor marker `seq` values. Prune appends the `pruned`
history record before deleting the directory; if deletion then fails, the group
stops loudly and the next successful run dedupes the existing record and
converges.
Observer receipt stats such as `segments_received` and `bytes_received` are not
decremented; pruning records storage cleanup, not the original receipt event.
Exit codes are `0` for a clean run, `2` when refusals are present, and `1` for
usage or unexpected errors.

### Observer health

Observer health has three distinct signals:

- The native observe/sense diagnostics-only beacon is emitted by the home-side
  sense processor on the local Callosum `observe.status` event at startup and on
  the 5s cadence, including healthy-idle. It excludes captured content and file
  paths/names, surfaces to local vantage points such as the TUI, and is not yet
  recorded into the observer registry.
- Platform upload observers can post sanitized status beacons over the HTTP
  observer API, either as extra fields on `/app/observer/ingest/event`
  `observe.status` or as a dedicated `/app/observer/health` payload. Both are
  stored on the observer record as `health.beacon`.
- The journal can record an active `health.ingest_rejection` when an upload
  fails the ingest contract. Capture health surfaces an active rejection as
  `degraded`.
- Observer auth rejections on data-bearing ingest surfaces also emit
  in-memory rate-limited operational logs: a periodic warning while the
  rejection burst is live, and one close error with the burst count after it
  goes quiet.

Missing beacons are not a failure; legacy observers without `health.beacon` use
normal liveness only. A later valid upload, including a duplicate after it passes
validation, clears the active rejection.

## Output Formats

See [captures.md](../talent/journal/references/captures.md) for detailed extract schemas:
- Audio transcripts: `audio.jsonl` with timestamps (speaker detection not included)
- Screen analysis: `screen.jsonl` with frame-by-frame categorization

## Configuration

Requires the journal directory at project root. API keys for transcription/vision services configured in `.env`.
