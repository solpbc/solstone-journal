# Linked-device observation

Multimodal desktop records and AI-assisted analysis.

## Linked-device architecture

### Partial audio evidence

The macOS client adds an optional `audio_capture` object to protocol-v3 `meta`.
The [generated v1 schema](../core/crates/solstone-core-transcripts-web/tests/fixtures/audio-capture-v1.schema.json)
and [Swift-emitted example](../core/crates/solstone-core-transcripts-web/tests/fixtures/audio-capture-v1.example.json)
are mirrored from `solstone-macos/contracts/`; the producer's native test detects
drift. This is an additive evidence contract, not an upload-admission requirement.

`state` describes recording lifecycle and known capture issues. Source counters
count original frames, including dropped writer input, and interrupted checkpoints
are lower bounds. `finished` does not prove that speech was present or that every
intended frame arrived. `remix` separately describes the current copy disposition;
one `prior_outcome` retains earlier copy failure without granting cleanup authority.
Only the existing explicit `unreadable_audio_sources` IDs retain their established
acknowledged-sidecar disposal meaning.

Ingest receipts preserve this object even when only screens arrive. The transcript
segment response reads those receipts independently of media or transcript files,
retains earlier source failures across duplicates, and exposes bounded diagnostics
through the existing warning details. Limits are 32 sources, 16 failures per source
and 256 KiB per capture object. Malformed, unsupported or unreadable evidence is
unknown/unavailable; historical absence does not imply healthy capture or create
a loss warning. Completed later copies display earlier failures as history.
Empty or failed audio transcript headers also carry the optional capture object.

Linked-device clients send segments to the journal through protocol v3 at [`POST /app/devices/ingest`](openapi/client-ingest-contract/projection.openapi.json). Each multipart request has one JSON `envelope` part and its `files` parts, sends `X-Solstone-Protocol-Version: 3`, and authenticates with the linked-device mTLS identity. The linked-device contract is the source for the request and authorization rules. Each client runs independently; the solstone app stores and processes the resulting journal.

| Linked-device client | What it records | Repo | Runs as |
|----------|-----------------|------|---------|
| **solstone-linux** | Screen + audio on Linux | `solstone-linux` | systemd user service / standalone |
| **solstone-macos** | Screen + audio on macOS | `solstone-macos` | Native menu bar app |
| **solstone-tmux** | Tmux terminal sessions | `solstone-tmux` | systemd user service / standalone |
| **solstone-windows** | Audited protocol-v3 ingest consumer | `solstone-windows` | — |

An authenticated linked device may upload browser JSONL with `source="browser"` through the existing protocol-v3 ingest path; admission is independent of `journal sense`, `journal transcribe`, and `journal describe`; each finalized segment period is one immutable `browser_pages.jsonl`. Allocation, reconciliation, reserved-marker, and segment-listing behavior follow the rules specified below.

## Commands

| Command | Purpose |
|---------|---------|
| `journal transcribe` | Audio transcription (native STT + speaker embeddings) |
| `journal describe` | Visual analysis of screen recordings |
| `journal grab` | Walk available screen frames and optionally write frame images |
| `journal sense` | Unified observation coordination |

## Architecture

```
Linked-device clients (standalone, per-platform repos)
       ↓ HTTP multipart upload
Linked-device Ingest API (protocol-v3 multipart via mTLS)
       ↓
   Raw media files (*.flac, *.webm, tmux_*.jsonl)
       ↓
journal sense (coordination)
   ├── journal transcribe → audio.jsonl
   └── journal describe → screen.jsonl
```

## Journal processing

Screen/audio collection, platform activity detection, and the upload client live
in the per-platform repositories (`solstone-linux`, `solstone-macos`,
`solstone-tmux`). The journal processes the resulting records with:

- **`journal sense`** dispatches transcription and description jobs.
- **`journal transcribe`** creates audio transcription and speaker-analysis embeddings. Its exit-code contract is [here](transcribe-failure-and-telemetry.md).
- **`journal describe`** analyzes screen records using the category guidance in [SCREEN_CATEGORIES.md](SCREEN_CATEGORIES.md).
- **The linked-device ingest service** handles protocol-v3 upload and manifest/day/segment reconciliation.

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

## Standalone clients

Each client is a standalone package in its own repository, with its own recording internals and lifecycle:

- **`solstone-linux`** records screen and audio on Linux; it runs as a systemd user service.
- **`solstone-macos`** records screen and audio on macOS; it is a native menu-bar app.
- **`solstone-tmux`** records tmux terminal sessions; it runs as a systemd user service.
- **`solstone-windows`** is an audited protocol-v3 ingest consumer.

All linked-device segments use the same [protocol-v3 contract](openapi/client-ingest-contract/projection.openapi.json). Device association and the linked-device mTLS identity authorize uploads and reconciliation. Legacy device-record keys do not authorize this path.

The journal derives duplicate identity from the segment directory on disk, not
from an append-only history index. For an upload, the server looks
under `chronicle/<day>/<stream>/` for segment directories sharing the requested
`HHMMSS` start, checks the exact requested key first, then checks the remaining
candidates lexicographically. The content set is the uploaded audio/video files
when the bundle has any; otherwise it is the uploaded non-reserved files, so
tmux-style JSONL-only bundles never match on an empty media set.

Reserved segment markers, including `stream.json` and `ingest.json`, are
journal-authored. If a client includes those names in a bundle, the bytes are
validated when covered by the journal contract, but they are not written from the
client payload and are recorded in receipt history as received-not-written.
Segment listings filter those audit-only records so clients never treat
journal-authored marker files as proof that their own marker bytes are held.

Every resolution into an existing candidate records `duplicate`. The
[reconciliation contract](openapi/client-ingest-contract/projection.openapi.json)
then lets `/app/devices/ingest/segments/<day>` corroborate that result for
linked-device clients before they remove local files, including segments that
were originally created by import or transfer.

Segment listings report each uploaded file as `present`, `processed`, or
`missing`. `present` means the recorded file still exists at its exact path.
`processed` applies only to raw audio/video media whose recorded path is absent
but whose same-stem JSONL sidecar at that segment path carries a terminal
`solstone.processing.v1` proof for the original input size. Legacy segments
without `ingest.json` use that proof to dedupe absent raw media, then graduate to
a manifest on the next resolution. Anything else is `missing` and remains
eligible for upload healing.

### Local diagnostics

The journal-side sense processor emits a local diagnostics event on
Callosum `observe.status` at startup and on its five-second cadence. It supports
local views such as the TUI; it is not a linked-device upload or reconciliation
operation.

## Output Formats

See the [output reference](../core/payload/solstone/talent/journal/references/captures.md) for detailed extract schemas:
- Audio transcripts: `audio.jsonl` with timestamps (speaker detection not included)
- Screen analysis: `screen.jsonl` with frame-by-frame categorization

## Configuration

Requires a resolved journal (see [environment.md](environment.md)). Vision and
STT use the active brain and bundled local runtimes; owner cloud keys live in
`config/journal.json`.
