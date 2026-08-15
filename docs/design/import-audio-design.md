# the native audio importer — native generic audio import

## Purpose and boundary

the native audio importer replaces the generic-audio segmentation half of
`solstone/think/importers/audio.py::prepare_audio_segments` and the associated
submit-and-wait path in `solstone/think/importers/cli.py`.  It owns probing an
already staged media file, allocating chronicle segment directories, stream-copy
remuxing each requested slice, retaining the durable audio-specific account of
that work, emitting existing `observe.observing` messages, and (when requested)
waiting for their terminal transcription evidence.

It does not own staging, source resolution, manifest policy, publication,
stream advancement, or owner-facing grammar.  The future dispatcher passes this
module's `CreatedSegment` and file-path lists unchanged to the publication layer's
`PublicationInput`; the publication layer remains the owner of `imported.json`.

## Acceptance criteria (scope §7)

1. `[test]` The reference's segment arithmetic is preserved for integral
   durations: 120 → 1 segment; 600 → 2; 601 → 3; and 0 → 1.  The final
   segment is the remaining duration, not an unconditional 300 seconds.
2. `[test]` Fractional duration arithmetic is preserved: 300.5 → one segment
   holding 300.5 seconds; 600.5 → two segments with a final 300.5-second
   slice; 601.482 → three segments with a final 1.482-second slice.  ⛔ Do not
   truncate duration and do not clamp the final slice to 300 seconds.
3. `[test]` A duration probe that cannot answer aborts the import; it does not
   default or guess a duration.
4. `[test]` A failure slicing one chunk drops that chunk and continues with
   later chunks; zero created segments aborts the import.  🔴 The injected
   failure is on a non-first, non-last chunk of a recording with at least three
   chunks.
5. `[test]` Dropped chunks are durable and re-readable from disk.
6. `[test]` Segment-directory allocation has no silent overwrite path.
7. `[test]` The true start time and true duration of every created chunk are
   recoverable from data written beside the segment, not inferred from its key.
8. `[test]` One observing event per prepared segment, carrying segment key,
   day, files, stream, and the metadata block with facet/setting present only
   when set.
9. `[test]` `wait=false` returns after segment creation and observing-event
   submission; with waiting enabled, the bound is 600 seconds since last
   progress.  Waiting is a native-function parameter, not owner grammar.
10. `[test]` Waiting reconciles on-disk transcription output before/during/after
    polling and collects and reports transcription failures without failing the
    import.  A dropped completion event with durable completion evidence is not
    a failure.
11. `[check]` The native generic-audio import re-encodes nothing it was not
    asked to.
12. `[check]` Run `make ci` after the settled implementation and report its
    actual result honestly; do not claim green without the run.

The implementation test names retain these AC labels.  This document does not
turn the reference's random collision sample into an acceptance condition.

## Decisions

### D1 — public surface: one typed outcome, not a segment vector

Add an async entry point and a seam-taking twin:

* `import_audio(request: AudioImportRequest) -> Result<AudioImportOutcome,
  ImportError>`;
* `import_audio_with_seams(request, seams) -> Result<AudioImportOutcome,
  ImportError>`.

`AudioImportRequest` carries `source_media`, `journal_root`, `day`,
`base_timestamp`, `import_id`, `stream`, optional `facet` and `setting`,
`wait_for_processing`, `stall_timeout`, and `poll_interval`.  The day directory is derived as
`journal_root/chronicle/day`; callers do not pass an arbitrary leaf directory.
`wait_for_processing` is deliberately a library parameter, never a new CLI
flag.

`AudioImportOutcome` is an enum:

* `Complete(AudioImportComplete)` has a nonempty `created` payload and no
  dropped chunks;
* `Partial(AudioImportPartial)` has the same created payload plus a nonempty
  `dropped_chunks: Vec<DroppedAudioChunk>`.

This makes “the whole recording imported” and “minutes 5–10 are missing”
different Rust values and makes a caller handle the latter explicitly.  The
shared payload contains `segments: Vec<CreatedSegment>`,
`files_created: Vec<PathBuf>`, the durable record path, and
`processing: ProcessingWaitOutcome`.  Thus it names the existing hand-off
vocabulary: the dispatcher uses `segments` and `files_created` unchanged in
`PublicationInput` and receives the later `PublicationRecord` from the publication layer rather
than inventing a competing publication record.

The module itself emits one `ObservingSegment` for every successful slice,
through `events.rs::emit_observe_observing`; `observing_fields()` already
produces the required `batch: true` wire shape and omits facet/setting when
they are `None` (`core/crates/solstone-core-import/src/events.rs:241-264`).
The emitter is a seam, not a returned duplicate event list.

Every `CreatedSegment` uses `StreamHints { kind:
Some(Kind::Imported(ImportSource::Named("audio".to_owned()))), host: None,
platform: None }`: this is a local batch import, not a device capture,
and lets `publish.rs` advance the supplied import stream without a the publication layer change
(`CreatedSegment` and its the publication layer consumer are in
`core/crates/solstone-core-import/src/publish.rs:28-43,181-195`).

Rejected: a bare `Vec<CreatedSegment>` plus logs.  It cannot express partial
source coverage structurally, invites callers to write a positive manifest,
and loses the durable-drop obligation behind incidental logging.

### D2 — allocation and segment keys

#### D2a. Exclusive leaf creation

Create `chronicle/<day>/<stream>` separately with `create_dir_all`; allocate
only its candidate leaf with `std::fs::create_dir`.  `AlreadyExists` is then a
real collision, not the Python `mkdir(parents=True, exist_ok=True)` race at
`audio.py:157-171`.  A slice file is written only after that exclusive leaf is
owned; a failed slice removes the empty leaf best-effort.

Rejected: `create_dir_all` at the leaf.  It retains the silent overwrite/race
where two imports can both regard the same segment directory as theirs.

#### D2b. Deterministic bounded forward probe

On `AlreadyExists`, advance only the candidate *start time* by one second and
retry exclusive creation, up to the module constant
`COLLISION_FORWARD_PROBE_LIMIT = 60`.  Sixty is comfortably below 300, so a
probe for chunk *i* cannot reach chunk *i + 1*'s nominal key 300 seconds ahead.
The duration suffix stays tied to the same slice; the source offset and true
start remain in the durable record.  Stop with
`ImportError::AudioSegmentCollision` when the limit is exhausted, and with
`ImportError::AudioSegmentDayOverflow` if the next candidate would cross
midnight.  The owner sees the source, day, stream, requested start, and limit
in the error; recovery is to choose a new base timestamp/day or retry after
resolving the competing import.

This removes the realistic same-five-minute-slot loss path, is repeatable in a
test, and does not pretend a random oracle draw is a contract.  No test may
assert one literal deconflicted key; it asserts exclusive creation, forward
movement, bounded exhaustion, and the durable true range.

Rejected: abort on the first collision.  It is safe but turns ordinary
concurrent/nearby imports into needless hard failures.  Rejected: RNG or a
seeded RNG.  Neither is reproducible nor safe, and both perpetuate the
reference random walk.

#### D2c. True rounded duration in new keys

New segment keys use `HHMMSS_<ceil(true_duration_seconds).max(1)>`.  The
suffix is an integer because the journal grammar is integer-based; ceiling
never understates the covered interval, and the import record remains the
authoritative fractional duration.  For example, a 1.482-second final chunk
keys as `_2`, while its record says `1.482`.

Readers do not assume 300: Python `segment_parse` converts the suffix to an
integer and adds it to the start (`solstone/think/utils.py:602-655` and its
following length parse); system-health explicitly splits and parses the suffix
to calculate the end (`core/crates/solstone-core-system-health/src/scan.rs:64-76`).
The Rust shared formatter accepts any nonempty numeric suffix
(`core/crates/solstone-core-format/src/segment.rs:47-88`), and the indexer only
uses the parsed start for buckets (`solstone/think/indexer/journal.py:123-140`).
Native `journal sense` dispatch uses the segment path supplied by the observing
message, not a 300-second assumption (`core/crates/solstone-core-sense/src/dispatch.rs`).

Rejected: preserve a constant `_300`.  It knowingly misdescribes final chunks
and makes duration-based readers wrong.  Rejected: floor/round-to-nearest.
Floor understates interval coverage; nearest has the same defect for values
below .5 and no compensating benefit.

#### D2d. Write new, read old

No reader or migration is changed.  Existing random-walk keys remain valid
because every existing `HHMMSS_<digits>` key still satisfies the parsers above.
The new import record is additive and is used only to recover exact source
ranges; it is not a prerequisite for reading old chronicles.  This is the
compatibility constraint: old journals remain fully readable.

### D3 — one import-owned durable audio record

Write `imports/<id>/audio-import.json` with
`schema: "solstone.import.audio.v1"`.  The new module owns
`write_audio_import_record` and `read_audio_import_record`; both use atomic
JSON replacement and the reader is used directly by AC5.  This filename avoids
the existing `import.json` (metadata), `manifest.json` (dedupe), and
`imported.json` (the publication layer publication) records.

The strict serde record shape (`#[serde(deny_unknown_fields)]`) is:

* `schema`, `source_media`, `day`, `stream`, and `import_id`;
* `created_segments`: for each, `key`, `day`, `stream`, `chunk_index`,
  `start_offset_seconds`, `start_timestamp`, `duration_seconds`, `file_path`,
  and `processing` (`not_requested`, `pending`, `succeeded`, `failed`, or
  `stalled`);
* `dropped_chunks`: for each, `chunk_index`, `start_offset_seconds`,
  `duration_seconds`, and the named `reason`;
* optional `abort`: the chunk index/range when known and the named abort
  reason, preserving work completed before a later allocation or slice abort;
* `wait`: `not_requested` or an outcome carrying terminal failed/stalled
  segment keys.

The start values are the requested source range and corresponding
`base_timestamp + offset`; they, not the rounded key, are the authoritative
range for AC7.  A normal record is written atomically after all slice attempts
and before the first `observe.observing` emit.  An abort after any prior
created or dropped chunk writes the same record first, with its `abort`
context; created audio is never deleted merely because a later chunk cannot be
allocated or remuxed.  This also retains total-loss drops before
`NoAudioSegmentsCreated` returns.  If waiting is requested, every durable
completion reconciliation atomically rewrites this same record with the
per-segment processing state; there is no append-only second record.  That
makes both AC5 drops and AC10 wait failures/recoveries re-readable.

Rejected: extend `PublicationRecord`.  `publish.rs` owns that the publication layer record and
this would broaden the native audio importer into an out-of-scope change.  Rejected: a sidecar inside
each chronicle segment.  The capturing module could own such a file, but it
would enter chronicle scans and add files visible to sense, system-health, and
the indexer.  A single import-local record is one DRY mechanism for every
source-range and outcome fact and keeps import bookkeeping out of capture
consumer directories.

### D4 — precise error taxonomy and no re-encode fallback

Add these `ImportError` variants and matching `Display` arms in `lib.rs`:

* `AudioDurationUnavailable { path, detail }` — `"could not determine audio
  duration <path>: <detail>"`;
* `AudioInputUnreadable { path, detail }` — `"could not read audio input
  <path>: <detail>"`;
* `AudioSliceRejected { path, chunk_index, start_offset_seconds,
  duration_seconds, detail }` — names the rejected source range and the
  underlying output/remux detail;
* `AudioSegmentDirectory { path, message }` — `"could not create audio segment
  directory <path>: <message>"`;
* `AudioSegmentCollision { day, stream, start, attempts }` — `"audio segment
  collision for <day>/<stream> at <start> after <attempts> attempts"`;
* `AudioSegmentDayOverflow { day, stream, start }` — `"audio segment collision
  probe crosses day boundary for <day>/<stream> at <start>"`;
* `NoAudioSegmentsCreated { path }` — `"no audio segments created from <path>"`.
* `AudioRecordRead { path, message }` and `AudioRecordWrite { path, message
  }` — durable audio-record read/write failures.

Abort the import for: an unknown/sentinel duration; `format::input` failures
such as `InvalidData`, `DemuxerNotFound`, `ProtocolNotFound`, or
`Other { errno: ENOENT|EACCES|EIO }`; any leaf-directory error other than
`AlreadyExists`; collision exhaustion/day overflow; and the total-loss guard
where no slice succeeded.  This retains the reference's `cli.py:1256-1267`
whole-import boundary without a broad catch.

Tolerate only errors produced while slicing/remuxing an individual allocated
chunk: packet/remux `InvalidData`, `OutputChanged`, and
`Other { errno: EINVAL }`.  They produce a `DroppedAudioChunk` with a
stable reason name and continue to later chunks.  `MuxerNotFound`,
`External`, destination errors such as `EACCES`, `EIO`, `ENOSPC`, and
`EROFS`, and `DecoderNotFound`/`EncoderNotFound` are whole-output or
input/configuration failures, so they abort with `AudioSliceRejected` rather
than draining into partial drops.  The native error vocabulary is
`ffmpeg-next/src/util/error.rs:40-81`.

Do **not** port Python's re-encode fallback (`audio.py:57-75`).  the native audio importer is a
lossless stream-copy import; AC11 forbids silently re-encoding.  A packet-level
`EINVAL` rejection of one copied chunk becomes a named durable drop.  A missing
or unsupported muxer configuration aborts instead, and all drops still trigger
the zero-created total-loss guard when applicable.

Rejected: `catch Err(_)` around a slice.  It would hide input/ownership/disk
errors that must abort and would blur a corrupt recording with one rejected
output chunk.

### D5 — partial imports suppress the dedupe manifest

Partial (`AudioImportOutcome::Partial`) must not write a generic dedupe
manifest.  A generic manifest with a positive `entry_count` causes the the resolver layer
dedupe guard to skip every normal recovery import.  `should_write_manifest`
cannot express this policy without changing `contract.rs`: it returns true
whenever `entries_written > 0`, even when `hard_failures` is nonempty
(`core/crates/solstone-core-import/src/contract.rs:67-71`).
`AudioImportOutcome::writes_dedupe_manifest() -> bool` is the single policy
surface: it returns false for `Partial` and true for `Complete`.  The
dispatcher owns consuming that method while constructing `ImportResult` and
writing the manifest, so that wiring is out of scope.

An owner can re-import normally after a partial result because no manifest
blocks it.  The consequence is explicit: successful chunks are made again as
additional, deterministically deconflicted segments; the native audio importer does not deduplicate
or delete the first run's good chunks.

Rejected: write the manifest and require `--force` for recovery.  That turns a
recoverable per-chunk failure into destructive owner grammar.  Rejected:
misreport `entries_written = 0` to force the existing predicate false.  That
corrupts the import result to work around a policy seam.

### D6 — async wait with durable reconciliation

`import_audio` is async and is driven by its caller's runtime; the native audio importer owns no
Tokio runtime.  This matches `identity::steward::wait_for_uses`
(`core/crates/solstone-core/src/identity/steward.rs:276-321`) and keeps a
library crate from nesting runtimes.  `wait=false` writes the initial record,
emits all observing messages, and returns immediately with every created
segment marked `not_requested`.

For `wait=true`, construct `CallosumSocketConnection` with the journal socket,
start it, reconcile durable output before the loop, poll `next_message` with
`tokio::time::timeout(min(poll_interval, remaining), ...)`, reconcile after
each poll, stop, then reconcile once after the loop.  `remaining` is
`last_progress + stall_timeout - now`: there is no second absolute deadline.
`last_progress` resets only when a previously pending segment reaches a
terminal state, including a transcription failure, matching the Python wait
loop.  The caller-supplied/default `STALL_TIMEOUT = Duration::from_secs(600)`
therefore bounds a stall, not total import duration.  Tests pass
millisecond-scale `stall_timeout` and `poll_interval`.

The disk predicate is the sibling `imported_audio.jsonl` of each created
`imported_audio.<ext>`.  It reads the first bounded JSONL header using a small
local `read_audio_processing_record` helper copied at the lower level from
system-health's bounded reader (`core/crates/solstone-core-system-health/src/scan.rs:285-313`),
then uses `solstone-core-processing-record::{vocab,predicate}`.  A
`STATE_ANALYZED` record for `HANDLER_TRANSCRIBE` with matching input size is
success; `STATE_FAILED` is terminal only under `is_failure_exhausted` (corrupt
input or `FAILED_ATTEMPT_BOUND`), otherwise it remains pending/retryable.

Do not depend on `solstone-core-system-health`: that would pull a presentation/
scan owner into import merely to share a private helper.  Do not add a new
processing state machine: `processing-record` already owns the vocabulary and
failure predicate.  A lower-level reader crate does not exist, so a local
bounded reader is the smallest dependency direction.

When it has a terminal answer, the disk record decides the state.  When disk is
silent, an `observe.observed` event with `error` marks failure and one
without `error` marks success; either event resets last-progress.  The
after-loop reconciliation catches AC10's dropped-event case where
`imported_audio.jsonl` already proves success before remaining pending keys are
reported stalled/failed.  Connection methods and feature gate are in
`core/crates/solstone-core-callosum/src/wire/connection.rs:39-135` and
`Cargo.toml:12-24`.

Add `solstone-core-callosum = { workspace = true, features = ["wire"] }` and
`tokio = { workspace = true, features = ["time"] }` to import.  Callosum owns
its needed `net`, `io-util`, `rt`, `sync`, and `time` feature set.  This changes
no callosum crate code.

Rejected: event-only waiting.  It produces false failure on dropped bus events.
Rejected: an owned runtime.  It makes calling context and shutdown behavior
implicit and conflicts with an async dispatcher.

`Complete` versus `Partial` is solely the slice result.  Transcription failures
never make the outcome `Partial`: even a recording whose every submitted
segment fails transcription is a `Complete` import when every slice was
created, with failed processing states recorded and reported without failing
the import.

### D7 — seams, arithmetic, and tests

Use the resolver layer's named generic `FnMut` seam pattern, not traits or global function
patching.  `AudioImportSeams<P, S, E>` has:

* `duration_probe: P`, where `P: FnMut(&Path) -> Result<f64, AudioProbeError>`;
* `slice: S`, where `S: FnMut(&Path, &Path, f64, f64) -> Result<(),
  AudioSliceError>`.
* `emit_observing: E`, where `E: FnMut(&ObservingSegment)`.

Production supplies the ffmpeg-next probe/remux bodies.  The seam errors retain
the ffmpeg detail so the entry point can apply D4's narrow classification; test
closures can inject named chunk failures and record emitted events without
media or a socket.  Production's emitter closure calls
`events.rs::emit_observe_observing`; it does not recreate its wire fields. This
is the same shape as `ResolutionSeams` /
`apple_detector: FnMut(&Path) -> Result<bool, AE>` in
`detect.rs:103-120,214-234`.

Reject a non-finite or negative probe result as `AudioDurationUnavailable`.
For a finite nonnegative `d`, ship
`let count = (((d + 299.0) / 300.0).floor() as u64).max(1);`; final duration is
`d - (index as f64 * 300.0)` and may exceed 300.

| `d` | count expression | final duration |
|---:|---:|---:|
| 300.5 | `floor(599.5 / 300) = 1` | 300.5 |
| 600.5 | `floor(899.5 / 300) = 2` | 300.5 |
| 601.482 | `floor(900.482 / 300) = 3` | 1.482 |

`ceil((d + 299.0) / 300.0)` is wrong for 300.5, and any integer cast of `d`
loses fractional tails (including 600.5, 601.482, and 0.02).  The
`fractional_durations` block in the vendored oracle is the authority; its
`rules.count` prose is not.

Vendor `~/import-fixtures-260811/audio-oracles.json` verbatim as
`core/fixtures/import_audio_oracles.json`; add
`core/crates/solstone-core-import/tests/audio.rs` using `include_str!` and
serde_json, following `tests/stream_name_oracle.rs`.

| AC | Test and seam |
|---|---|
| 1–2 | Oracle-driven unit cases for every integral `(start_seconds, chunk_duration)` call in order and the derived fractional call sequence; `duration_probe` and `slice` are pure fakes. |
| 3 | Probe returns its named unavailable/error result; assert `AudioDurationUnavailable` and no allocation/slice. |
| 4–5 | Three-or-more chunk input with `slice` failing only the non-first, non-last middle chunk; assert later slice continues, `Partial` identifies that true range, and `read_audio_import_record` returns its named drop. |
| 6–7 | Construct collision directories; assert exclusive allocation, deterministic forward probe, no literal random key, parsed rounded key, and record-held true start/duration that differs from a deconflicted key.  Exhaustion creates 60 occupied candidate leaves; midnight starts at `23:59:59` with its first leaf occupied, so the next forward probe refuses. |
| 8 | Recording `emit_observing` seam receives exactly one `ObservingSegment` per successful slice; assert its segment/day/basename files/stream and optional facet/setting values, plus the serialized `observing_fields()` shape.  The production closure delegates to `emit_observe_observing`. |
| 9 | `wait=false` uses the recording emitter seam, returns after emissions, and never starts a listener loop. |
| 10 | Fake callosum listener omits an event while the test writes a valid sibling processing record; before/during/after reconciliation marks success.  Separate millisecond-scale no-progress test proves the stall bound, durable record rewrite, and recorded transcription failures without changing `Complete` to `Partial`. |
| 11 | Manual stream-copy check described below. |
| 12 | Settled-tree `make ci` check; commit body records its exact pass/fail result. |

AC11 uses `tests/fixtures/audio/aac_single_track.m4a` without copying it into
`core/fixtures`.  At implementation time, run the real probe/remux path for a
nonzero range, decode that source range and the emitted segment to PCM, and
compare PCM samples (and sample format/rate/channel layout) for equality.  The
commit body records the exact result.  If that check cannot be completed in the
implementation environment, the commit body says “could not verify” and why;
the design does not pre-concede the check.

### D8 — FFmpeg dependency wiring

Add the established static-build dependencies to import:

```toml
ffmpeg-next = { version = "9.0.0", default-features = false, features = ["format", "build"] }
ffmpeg-sys-next = { version = "9.0.0", default-features = false, features = ["build-portable"] }
```

`format` enables the format/codec APIs needed for input, output, streams,
packets, seek, and duration; no scaling or resampling feature is needed.  The
three current precedents are describe, grab, and observe-audio.  `build` plus
`build-portable` compiles FFmpeg into the shipped native binary; the native audio importer does not
dynamically load a local tool or shell out to `ffmpeg`/`ffprobe`.

Regenerate and commit `core/Cargo.lock`.  The resolved ffmpeg packages already
exist, but the lock's `solstone-core-import` package dependency edge changes;
a stale lock makes `check-rust-msrv` fail misleadingly as an MSRV error.

Rejected: subprocesses.  They recreate the missing-tool distinction and are
not linked product behavior.  Rejected: dynamic loading.  It violates the
portable static-binary constraint.

### D9 — explicit non-goals and follow-ups

the native audio importer changes no supervisor, callosum protocol/crate, drain behavior,
transcription consumer, `sense.py`, owner-facing grammar, the input boundary/the resolver layer/the publication layer module
bodies, or Python.  It does not add a new `sol`/`journal` flag.  It leaves the
scope §9 follow-ups—dispatcher wiring, manifest finalization, publication,
and any end-to-end owner-command integration—to their owning waves.  The
dispatcher supplies a unique `import_id` for each invocation; reusing one
replaces `audio-import.json` wholesale, matching the reference record's
replacement behavior.

## File-by-file implementation order and commit shape

1. Add the vendored oracle at `core/fixtures/import_audio_oracles.json` and
   add `core/crates/solstone-core-import/tests/audio.rs`; establish arithmetic,
   seam, allocation, durable-record, and wait tests first.
2. Replace `core/crates/solstone-core-import/src/audio.rs`'s reserved seam with
   the request/outcome/seams, ffmpeg probe/remux, allocation, durable record,
   and async wait implementation.  Remove only the `audio` row from
   `MODULE_STUBS` and update `tests/stub_table.rs` accordingly.
3. Add the D4 `ImportError` variants/Display arms in
   `core/crates/solstone-core-import/src/lib.rs`; export the new public audio
   types/read-write functions there.
4. Update `core/crates/solstone-core-import/Cargo.toml` for ffmpeg, callosum
   wire, processing-record, and Tokio; regenerate `core/Cargo.lock`.
5. Leave dispatcher/manifest/publication integration for its own commit/wave.

The implementation commit should contain only the import crate, its fixture and
tests, and the lockfile.  No Python or the publication layer changes are authorized by this
design.
