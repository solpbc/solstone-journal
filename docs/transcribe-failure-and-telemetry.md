# Transcribe: failure semantics & telemetry

How `journal transcribe` reports what happened, and what the `observe.transcribed`
event carries. Two rules govern everything here:

1. **A failure is never reported as a success.** If the transcript was not produced,
   the process says so with an exit code and a reasoned event.
2. **Telemetry is content-free.** Timings and labels only — no transcript text, words,
   topics, setting, or emotion ever rides on an event.

## Exit-code contract

`journal transcribe <file>` exits with exactly one of:

| Exit | Meaning | Input file | Output |
|------|---------|-----------|--------|
| `0` | Work is done. Either a transcript was written, or the clip was an empty terminal (no decodable audio). | Preserved | Transcript `.jsonl` (+ `.npz`) written, or a header-only terminal `.jsonl` record written for empty silence. Retention owns any later raw release. |
| `69` (`EXIT_PROVIDER_BLOCKED`) | **Honest deferral.** The STT provider, or the handler-generated native embedding payload, could not complete the work. | **Preserved on disk** | None |
| `75` | Temporary native transcript-write failure (launch, local output, verification, or malformed response). | Preserved on disk | None or a detected partial NPZ sidecar |
| `78` | Native writer host or installation configuration failure. | Preserved on disk | None |
| `1` | Hard failure, including an invalid native transcript-write request. | Preserved on disk | None |

`sense.py` treats `69` as neither a success nor a failure — it calls
`_check_segment_observed()` with no error and does not record a successful contact.
The hard/configuration/temporary codes (`1`, `75`, and `78`) are handler failures
and retain the raw input for investigation or retry.

The deferral path was previously a bare `return`, which exited `0` and made
`sense.py` log "Handler completed successfully" for a segment that was never
transcribed. That is the bug this contract closes.

## How the retry actually happens

Retry after a deferral is always **cross-process**. `FileSensor.start()` has no rescan
loop; there is no in-process retry, no backoff timer, and no attempt counter.

The re-attempt comes from the daily think run's sense-repair pre-phase, which shells
out to `journal sense --day <day>`. That builds a *fresh* `FileSensor` whose
`scan_unprocessed` re-picks any input that still lacks a `.jsonl`. Because the deferral
path writes nothing, the audio is still there and still lacks its output, so it is
picked up again on the next pass.

This is why a deferral must not write a placeholder or an empty output — doing so would
mark the segment done and the audio would never be retried.

`journal transcribe --all` absorbs a deferral per file and moves on to the next one,
reporting the count in its summary. (`SystemExit` is a `BaseException`, so the batch
loop has to catch it explicitly — a plain `except Exception` would let one deferred clip
abort the entire batch.)

## Defer vs. fail

Backend-specific policy:

| Backend | Failure surface |
|---------|-----------------|
| `parakeet` / `parakeet-cpp` | Local STT. Supervised-server unavailability defers; live-server bad responses and contract failures fail loudly. |
| `confidential` | Hosted STT over the verified loopback forwarder. Lane, attestation, backpressure, transport, rejected-request, unexpected-status, and bad-200 contract failures defer with hosted reason codes. |

| Condition | Classified as | Why |
|-----------|--------------|-----|
| Parakeet server unreachable, warming, or **dead mid-request** | Defer (`69`) | The server is a supervised process. It comes back. |
| Confidential lane refuses cloud egress | Defer (`69`) | The lane may permit a local backend later; the audio must not be lost. |
| Confidential backend already dispatched, then lane inactive or audio disabled | Defer (`69`) | The selected backend can no longer carry audio at dispatch time. The input is preserved for a later local or confidential retry. |
| Confidential backend selected and decoded audio exceeds the hosted duration cap | Defer (`69`) | No fallback is attempted; the input stays on disk for a later retry after the provider limit is addressed. |
| Stranded confidential channel before dispatch and local RAM below floor | Fail (`1`) | `resolve_default_backend` surfaces the local-STT requirement before `_process_one`; no JSONL is written, input audio stays on disk, and the segment remains `incomplete`. |
| Confidential hosted STT 400/413 or bad 200 contract | Defer (`69`) | Hosted STT is operated infrastructure. A 400 can be an engine-side regression, and owner audio is irreplaceable; preserving it for a post-fix drain is safer than failing permanently. |
| Confidential hosted STT unreachable, backpressured, or unexpected status | Defer (`69`) | These are service-side or lane-side conditions. The deferred event carries the reason so health surfaces can show the condition without leaking content. |
| Native speakers-analyze helper exits non-zero, times out, exceeds stream bounds, emits malformed JSON, or violates the response schema | Fail (`1`) | Speaker analysis is now the only speaker plane. A failed helper leaves the input audio on disk and writes no transcript or embedding archive. |
| HTTP 5xx from a live server, malformed JSON, contract violation | Fail (`1`) | The server answered — it is broken, not absent. Retrying the same request reproduces it. |
| Malformed request or bad URL scheme (`LocalProtocolError`, `UnsupportedProtocol`) | Fail (`1`) | These are transport errors, but the bug is on *our* side of the wire. A retry cannot fix them, and deferring would hide the bug behind a daily retry forever. |
| Anything else unexpected | Fail (`1`) | Surface it. |

Server-death-mid-request is the subtle one. When the parakeet.cpp server is OOM-killed
partway through a request (measured: a clip longer than roughly 320–340 s exhausts the
6 GiB Vulkan backend, and the server exits 139 with no HTTP body), the connection drops
without a response and `httpx` raises `RemoteProtocolError`. That is a `TransportError`
but **not** a `NetworkError` — so the old explicit catch tuple missed it and the crash
surfaced as a hard failure. `_parakeet_cpp.transcribe()` now catches `httpx.TransportError`,
which covers connect, timeout, network *and* protocol failures in one class, while
deliberately leaving `DecodingError` / `TooManyRedirects` / `HTTPStatusError` uncaught.

Note that this makes the failure *honest*, not *rare*. A long clip will still OOM the
server; it will now defer, be re-picked the next day, and OOM again. Chunking long audio
so it stops OOMing is separate work.

The confidential hosted lane deliberately deviates from the local-backend fail-loud
rule for 400/413 and bad-200 contract failures. For the hosted lane only, those
conditions defer because the operated engine can regress independently of the journal,
the audio cannot be recreated, and the deferred event makes the condition visible on
health surfaces. Local backends keep the fail-loud semantics above.

## Reason strings

Every deferred and failed event carries a machine-readable `reason`.

| Reason | Raised by | Means |
|--------|-----------|-------|
| `no_port` | `parakeet_server.connect()` | The supervisor has published no port for the service. |
| `server_not_ready` | `parakeet_server.connect()` | Port exists; the health probe did not report ready (usually still loading the model). |
| `read_timeout` | transport classifier | Any `httpx.TimeoutException`, including `ConnectTimeout` (which is a timeout, not a connect error, in the httpx hierarchy). |
| `server_disconnected` | transport classifier | `httpx.ProtocolError` / `RemoteProtocolError` — **the server died mid-response.** |
| `connect_error` | transport classifier | `httpx.ConnectError` — nothing listening. |
| `network_error` | transport classifier | Other `httpx.NetworkError` (read/write errors on an established connection). |
| `transport_error` | transport classifier | Any other `TransportError` (proxy, unsupported protocol). |
| `confidential_egress_blocked` | `process_audio` | The confidential lane refused to send audio to a cloud backend. |
| `confidential_lane_inactive` | confidential STT gate or backend | Confidential STT was selected, but the confidential lane was no longer active. |
| `confidential_audio_disabled` | confidential STT gate | Confidential STT was selected under the lane, but `transcribe.confidential_audio` was off. |
| `confidential_audio_too_long` | `process_audio` | Confidential STT was selected, but the decoded input exceeded the hosted audio duration cap. |
| `attestation_unreachable` | confidential probe status | Confidential attestation could not reach the gateway. Reused from the confidential lane health vocabulary. |
| `attestation_failed` | confidential probe status | Confidential attestation completed but did not verify. Reused from the confidential lane health vocabulary. |
| `attestation_not_yet_verified` | confidential probe status | Confidential provenance exists, but this process has no verified attestation session yet. Reused from the confidential lane health vocabulary. |
| `attestation_stale` | confidential probe status | Confidential attestation cadence lapsed. Reused from the confidential lane health vocabulary. |
| `hosted_transcribe_rejected` | confidential backend | Hosted STT returned 400 or 413. |
| `hosted_transcribe_backpressure` | confidential backend | Hosted STT returned 429, 503, or 504. |
| `hosted_transcribe_unreachable` | confidential backend | The hosted STT POST timed out or failed at the transport layer, or required local credential/device header data was unavailable. |
| `hosted_transcribe_contract_failed` | confidential backend | Hosted STT returned 200 with invalid JSON, a non-object body, or a body that violated the expected word-timing contract. |
| `hosted_transcribe_unexpected_status` | confidential backend | Hosted STT returned a non-200 status outside the named rejected/backpressure buckets. |
| `speaker_analysis_native_failure` | native speakers-analyze typed failure | Speaker analysis failed after STT. The event carries content-free native attribution fields below; the local log carries full details. |
| `destination-exists` | native transcript writer | An output appeared despite the handler's redo guard; the event fails hard but the log explains that it is already processed / redo-shaped. |
| `invalid-output-path`, `malformed-request`, `unknown-schema`, `missing-statement-id`, `invalid-statement-id`, `duplicate-statement-id`, `invalid-statement`, `invalid-header` | native transcript writer | Handler request-construction failure; hard failure (`1`). |
| `payload-unreadable`, `payload-invalid`, `payload-non-finite` | native transcript writer | The handler's own generated embedding payload was rejected; deferred (`69`) without blaming owner input. |
| `output-unwritable`, `npz-verification-failed`, `internal-error`, `launch-failed`, `invalid-response`, `orphan-npz-remove-failed`, `payload-tempfile-failed` | native transcript writer | Temporary write failure (`75`); logs warn if an NPZ exists without its JSONL. |
| `unsupported-host`, `handshake-skip`, `handshake-fail` | native transcript writer | Native writer compatibility or installation failure (`78`). |
| *(provider reason code)* | failed path | On a hard failure from a provider error — e.g. `transcription_http_error`, `invalid_json`, `contract_violation`. |
| *(exception type name)* | failed path | On any other hard failure. |

The transport classifier (`_transport_retry_reason` in `_parakeet_cpp.py`) is the single
source of truth for the five transport reasons. Its checks run subclass-before-base
because the httpx exception tree overlaps.

The confidential backend reuses `confidential_probe_status()` for attestation reasons
instead of inventing audio-specific attestation names. `AttestationFailedError` and
`AttestationStaleError` therefore surface as one of the four attestation reasons above.

## The `observe.transcribed` event

One event name, four outcomes. Every attempt emits exactly one event.

| Field | Type | Present on |
|-------|------|-----------|
| `outcome` | `transcribed` \| `deferred` \| `failed` \| `preserved` | always |
| `input` | journal-relative path of the audio | always |
| `output` | journal-relative path of the `.jsonl` | success |
| `reason` | machine reason (table above) | deferred, failed |
| `error` | exception **type name** — never the message (see below) | failed |
| `backend` | STT backend name (`parakeet`, `parakeet-cpp`, or `confidential`) | whenever resolved |
| `device` | resolved placement (`cpu` / `gpu`) when a placement record exists; configured device otherwise | whenever known (see below) |
| `model` | model filename | success, and failures after the backend reported it |
| `audio_seconds` | original decoded length, 1 dp | whenever decoded |
| `reduced_seconds` | length after silence-trimming, 1 dp | when reduction ran |
| `rtfx` | `audio_seconds / (asr_ms / 1000)`, 2 dp | success, when ASR took ≥ 1 ms |
| `peak_rss_mib` | peak RSS of this process, MiB | always |
| `timings` | nested `{<stage>_ms: int}` | always (only the stages that ran) |
| `vad_duration`, `vad_speech`, `noisy`, RMS/loud stats | VAD summary | always |
| `duration_ms` | total wall-clock of `process_audio` | success |
| `day`, `segment`, `observer` | provenance | when derivable |
| `speaker_analysis_failure_path` | currently `native` | failed native speakers-analyze path only |
| `speaker_analysis_failure_stage` | `request` \| `invoke` \| `parse` \| `payload` | failed native speakers-analyze path only |
| `speaker_analysis_failure_reason` | lowercase machine label, e.g. `timeout`, `stdout-too-large`, `malformed-response`, `embedding-payload-size-mismatch` | failed native speakers-analyze path only |
| `speaker_analysis_failure_native_exit_code` | helper exit code, including negative signal codes | failed native speakers-analyze path only, when known |

### Timing stages

Only stages that actually ran appear. A stage split across several calls (`write` covers
the jsonl and the npz) reports its total.

| Key | Stage |
|-----|-------|
| `queue_wait_ms` | Enqueue-to-spawn latency, so it *includes* any memory-gate throttle wait. Measured by `sense.py` and passed in via `SOL_QUEUE_WAIT_MS`; the handler cannot compute it itself. Absent when not supplied. |
| `decode_ms` | `load_audio` |
| `vad_ms` | `run_vad` |
| `reduce_ms` | `reduce_audio` (absent when reduction was skipped) |
| `asr_ms` | `stt_transcribe` — the STT call itself |
| `speakers_analyze_ms` | native helper request construction, invocation, response validation, speaker evidence, diarization, and statement embeddings |
| `write_ms` | jsonl + npz writes |

Deferred and failed events carry whatever completed before the failure — typically
`queue_wait`, `decode`, `vad`, `reduce`, and (on a mid-ASR death) `asr`.

### The content-free guarantee

No transcript text, word list, topic, setting, or emotion appears in any event field.
The event carries numbers, paths, and labels only.

**`error` is the exception's type name, never its message.** This is the load-bearing
detail, and it is structural rather than a matter of care: exception *messages* can
embed provider output. `SchemaValidationError` (`think/models.py`) builds its message
with a ~197-character preview of the raw response, and provider wrappers may
interpolate that into their own exceptions. Putting `str(e)` on the bus would
therefore publish transcript text whenever a provider response failed schema
validation.

Carrying only `type(e).__name__` makes the guarantee hold by construction, so a new
provider exception cannot quietly reintroduce the leak. The full message and traceback
go to the handler log — which is where they belong, and where the health UI already
deep-links from the failure notification.

## Intentionally not measured

These were considered and deliberately left out. Each would cost more than it is worth
right now.

- **Retry count.** Not stored, because it is **derivable**: every attempt emits one
  reason-tagged `deferred` event, so counting those per input across the daily retry
  cadence *is* the count. An in-memory counter would always read 1 (each retry is a
  fresh process), and a durable attempt ledger is a metrics service — a much larger
  thing than this problem justifies.
- **Cold vs. warm start.** STT runs one process per file with a fresh HTTP client. The
  persistent server's warmth is a property of the supervisor, not reliably observable
  from the client. A `cold` flag here would be a guess.
- **VRAM / GPU memory.** Needs a resource sampler (an `nvidia-smi` or Vulkan polling
  loop) — a standing subsystem, not a field. The single `resource.getrusage` read behind
  `peak_rss_mib` is deliberately the whole budget.
- **`model` on deferred events.** `get_model_info()` is cheap for the parakeet-cpp and
  cloud backends, but on Apple Silicon it shells out to the CoreML helper (`--version`,
  10 s timeout). Rather than hoist a subprocess probe onto a path whose whole point is
  *not* to do expensive work, deferred events omit `model`. `device` reports the
  supervisor placement for parakeet-cpp when that record exists, and otherwise falls
  back to the configured value when the config names one.

## Rollback

Roll back the confidential STT backend, deny-by-default transcribe dispatcher gate,
strict forwarder accessor, shared audio-wire helper, resolver changes, and API
payload updates together. The config surface includes `transcribe.confidential_audio`,
but there is no on-disk migration to undo because an absent value means enabled.
Reverting restores the previous provider-selection and transcription failure behavior.
