# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Transcribe audio files with pluggable STT and native speaker analysis.

Transcription pipeline:
1. VAD stage: Run Silero VAD to detect speech and filter silent files early
2. Audio reduction: Trim long silence gaps for faster processing
3. Transcription: Dispatch to the configured or resource-aware STT backend
4. Speaker analysis: Call the native helper for labels, evidence, and embeddings
5. Output: JSONL format compatible with format_audio() in observe/hear.py

Output files:
- <stem>.jsonl: Transcript with HH:MM:SS timestamps and optional speaker labels
- <stem>.npz: Native helper embeddings indexed by statement id

Configuration (journal config transcribe section):
- transcribe.backend: STT backend ("parakeet", "parakeet-cpp", "confidential"). If unset, auto-selected by lane and resources.
- transcribe.preserve_all: Keep audio files even when no speech detected (default: false)
- transcribe.min_speech_seconds: Minimum speech duration to proceed. Default: 1.0

Parakeet backend settings (transcribe.parakeet):
- model_version: Parakeet model version ("v3"). Default: "v3"
- cache_dir: Optional helper cache directory
- timeout_sec: Helper timeout in seconds. Default: 120.0

Platform optimizations:
- Apple Silicon hosts use the CoreML Parakeet helper.
- Linux hosts use a supervised parakeet.cpp server.

Failure semantics & telemetry:
- Exit 0 = output written or silence-filtered; 69 = honest provider or generated-
  payload deferral; 75 = temporary native write failure; 78 = native installation
  configuration failure; 1 = hard failure.
- Every attempt emits one content-free observe.transcribed event carrying per-stage
  timings and a machine-readable reason.
- Full contract: solstone/observe/transcribe/failure-and-telemetry.md
"""

from __future__ import annotations

import argparse
import contextlib
import datetime
import json
import logging
import os
import resource
import sys
import time
from collections.abc import Iterator
from pathlib import Path
from typing import TYPE_CHECKING

from solstone.apps.settings.install_copy import (
    STT_DETECTED_MEMORY_TEMPLATE,
    STT_DETECTED_MEMORY_UNKNOWN,
    STT_EXPLICIT_LOCAL_LOW_TEMPLATE,
    STT_LOCAL_REQUIREMENTS_TEMPLATE,
    STT_LOCAL_UNSUPPORTED,
    STT_NO_LOCAL_STT_RECOVERY,
)
from solstone.apps.speakers.encoder_config import (
    ENCODER_ID,
    OVERLAP_DETECTOR_ID,
    SPEAKER_EVIDENCE_VERSION,
)
from solstone.observe.exit_codes import EXIT_PROVIDER_BLOCKED
from solstone.observe.processing_record import (
    HANDLER_TRANSCRIBE,
    REASON_CORRUPT_INPUT,
    REASON_NO_DECODABLE_AUDIO,
    REASON_OK,
    STATE_ANALYZED,
    STATE_EMPTY,
    STATE_FAILED,
    build_processing_record,
)
from solstone.observe.transcribe import (
    BACKEND_REGISTRY,
    ConfidentialAudioEgressError,
    ConfidentialTranscribeDeferral,
    get_backend,
)
from solstone.observe.transcribe import transcribe as stt_transcribe
from solstone.observe.transcribe.config import confidential_audio_enabled
from solstone.observe.transcribe.native import (
    COMPOSED_COMMAND_WARNING,
    NativeSpeakerTranscriptWriteError,
    SpeakerTranscriptWriteResponse,
    write_speaker_transcript,
)
from solstone.observe.transcribe.resource import (
    CONFIDENTIAL_STT_MAX_AUDIO_SECONDS,
    STT_SURFACE,
    local_stt_backend,
    resolve_stt_backend_choice,
    stt_local_floor_bytes,
)
from solstone.observe.transcribe.sound_tags import tag_audio
from solstone.observe.transcribe.speakers_analyze_errors import (
    SPEAKER_ANALYSIS_FAILURE_LABEL,
    SPEAKER_ANALYSIS_FAILURE_REASON,
    SpeakerAnalyzeError,
)
from solstone.observe.utils import (
    SAMPLE_RATE,
    AudioDecodeError,
    get_segment_key,
    load_audio,
)
from solstone.think.callosum import callosum_send
from solstone.think.media import AUDIO_EXTENSIONS as SUPPORTED_AUDIO_FORMATS
from solstone.think.providers.memory import gb, read_available_bytes
from solstone.think.providers.parakeet_install import ParakeetProviderError
from solstone.think.providers.parakeet_server import ParakeetServerNotReady
from solstone.think.utils import (
    day_dirs,
    day_from_path,
    get_config,
    get_journal,
    iter_segments,
    journal_relative_path,
    require_solstone,
    resolve_journal_path,
    setup_cli,
)

SPEAKERS_ANALYZE_EX_CONFIG = 78

if TYPE_CHECKING:
    import numpy as np

    from solstone.apps.speakers.evidence import SpeakerEvidenceDecision
    from solstone.observe.vad import AudioReduction, VadResult

# Re-export defaults for backwards compatibility
__all__ = [
    "DEFAULT_MIN_SPEECH_SECONDS",
    "MIN_STATEMENT_DURATION",
    "main",
]

# Default transcription settings
DEFAULT_BACKEND = "parakeet"
DEFAULT_MIN_SPEECH_SECONDS = 1.0

# Minimum statement duration for embedding (seconds)
MIN_STATEMENT_DURATION = 0.3


def _join_missing_fields(fields: list[str]) -> str:
    if len(fields) == 1:
        return fields[0]
    if len(fields) == 2:
        return f"{fields[0]} and {fields[1]}"
    return f"{', '.join(fields[:-1])}, and {fields[-1]}"


def _confidential_backend_fallback_reason(
    journal_config: dict,
    *,
    confidential_channel_usable: bool,
    confidential_audio: bool,
) -> str:
    if confidential_channel_usable and not confidential_audio:
        return "confidential audio is disabled"

    from solstone.think.providers.local_endpoint import confidential_provenance_block

    if confidential_provenance_block(dict(journal_config)) is None:
        return "confidential lane is inactive"

    providers = journal_config.get("providers")
    local = providers.get("local", {}) if isinstance(providers, dict) else {}
    if not isinstance(local, dict):
        local = {}
    missing: list[str] = []
    if not local.get("credential"):
        missing.append("credential")
    if not str(local.get("endpoint_url") or "").strip():
        missing.append("endpoint URL")
    if not str(local.get("served_model_id") or "").strip():
        missing.append("served model ID")
    return (
        f"confidential channel is incomplete: missing {_join_missing_fields(missing)}"
    )


def resolve_default_backend(
    args: argparse.Namespace,
    transcribe_config: dict,
    *,
    journal_config: dict | None = None,
) -> str:
    """Resolve the effective default STT backend once, from a single free-RAM read.

    Honors explicit CLI/config choices, warns on an explicit local choice below
    the platform floor, and raises SystemExit(1) with a clear requirement when
    there is no viable backend.
    """
    available_bytes = read_available_bytes()
    floor_bytes = stt_local_floor_bytes()
    local_backend = local_stt_backend()
    configured_backend = transcribe_config.get("backend")
    explicit_backend = args.backend or configured_backend
    if explicit_backend:
        if explicit_backend not in BACKEND_REGISTRY:
            logging.warning(
                "Configured STT backend %r is unavailable; treating it as unset",
                explicit_backend,
            )
            explicit_backend = None
    from solstone.think.services import spp

    if journal_config is None:
        journal_config = get_config()
    # Routing uses channel usability; the dispatch refusal gate separately keys
    # on bare confidential block presence to prevent accidental egress.
    confidential_channel_usable = spp.is_confidential_channel_usable(journal_config)
    confidential_audio = confidential_audio_enabled(transcribe_config)
    backend = resolve_stt_backend_choice(
        explicit_backend,
        available_bytes,
        floor_bytes=floor_bytes,
        local_backend=local_backend,
        confidential_lane_active=confidential_channel_usable,
        confidential_audio_enabled=confidential_audio,
    )
    if explicit_backend == "confidential" and backend != "confidential":
        reason = _confidential_backend_fallback_reason(
            journal_config,
            confidential_channel_usable=confidential_channel_usable,
            confidential_audio=confidential_audio,
        )
        logging.warning(
            "Configured STT backend 'confidential' cannot run because %s; using local STT placement",
            reason,
        )
    if explicit_backend and backend in {"parakeet", "parakeet-cpp"}:
        _warn_if_local_below_floor(backend, available_bytes, floor_bytes)
    if backend == STT_SURFACE:
        _surface_stt_requirement(available_bytes, floor_bytes)
        raise SystemExit(1)
    return backend


def _warn_if_local_below_floor(
    backend: str, available_bytes: int | None, floor_bytes: int | None
) -> None:
    if (
        backend == "parakeet"
        and floor_bytes is not None
        and available_bytes is not None
        and available_bytes < floor_bytes
    ):
        logging.warning(
            STT_EXPLICIT_LOCAL_LOW_TEMPLATE.format(ram_gb=floor_bytes // 1024**3)
        )


def _surface_stt_requirement(
    available_bytes: int | None, floor_bytes: int | None
) -> None:
    if floor_bytes is None:
        requirement = STT_LOCAL_UNSUPPORTED
    else:
        requirement = STT_LOCAL_REQUIREMENTS_TEMPLATE.format(
            ram_gb=floor_bytes // 1024**3
        )
    available_gb = gb(available_bytes)
    detected = (
        STT_DETECTED_MEMORY_UNKNOWN
        if available_gb is None
        else STT_DETECTED_MEMORY_TEMPLATE.format(available_gb=available_gb)
    )
    logging.error("%s %s %s", requirement, detected, STT_NO_LOCAL_STT_RECOVERY)


def _get_jsonl_path(audio_path: Path) -> Path:
    """Generate the corresponding JSONL path."""
    return audio_path.with_suffix(".jsonl")


def _get_embeddings_path(audio_path: Path) -> Path:
    """Generate the corresponding embeddings path."""
    return audio_path.with_suffix(".npz")


class _StageTimings:
    """Content-free per-stage wall-clock accumulator for observe.transcribed.

    Records only stages that actually ran, as integer milliseconds under a
    ``<stage>_ms`` key.  Holds no audio, no transcript, and no file content.
    """

    def __init__(self) -> None:
        self._stages: dict[str, int] = {}

    @contextlib.contextmanager
    def time(self, stage: str) -> Iterator[None]:
        """Time a pipeline stage, recording it as ``<stage>_ms``.

        Repeat entries for one stage accumulate, so a stage split across several
        calls (``write`` covers the jsonl and the npz) reports its total.
        """
        started = time.perf_counter()
        try:
            yield
        finally:
            key = f"{stage}_ms"
            elapsed_ms = int((time.perf_counter() - started) * 1000)
            self._stages[key] = self._stages.get(key, 0) + elapsed_ms

    def set_ms(self, stage: str, value: int) -> None:
        """Record a stage duration measured outside this process."""
        self._stages[f"{stage}_ms"] = value

    def get_ms(self, stage: str) -> int | None:
        return self._stages.get(f"{stage}_ms")

    def as_dict(self) -> dict[str, int]:
        return dict(self._stages)


def _read_queue_wait_ms() -> int | None:
    """Read the queue wait sense.py measured for this file, if it set one."""
    raw = os.getenv("SOL_QUEUE_WAIT_MS")
    if not raw:
        return None
    try:
        return int(raw)
    except ValueError:
        logging.warning("Invalid SOL_QUEUE_WAIT_MS: %s", raw[:50])
        return None


def _peak_rss_mib() -> int:
    """Peak resident set size of this process, in MiB.

    ``ru_maxrss`` is KiB on Linux and bytes on macOS.  ``resource`` here is the
    stdlib module, not the sibling ``transcribe/resource.py`` (absolute imports).
    """
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    divisor = 1024 * 1024 if sys.platform == "darwin" else 1024
    return int(peak / divisor)


def _uses_parakeet_cpp(backend: str | None) -> bool:
    """Return whether this backend dispatches to the Linux parakeet.cpp path."""
    return backend == "parakeet-cpp" or (
        backend == "parakeet" and sys.platform.startswith("linux")
    )


def _emit_transcribed(
    event: dict,
    *,
    outcome: str,
    timings: _StageTimings | None = None,
    backend: str | None = None,
    model_info: dict | None = None,
    backend_config: dict | None = None,
    audio_seconds: float | None = None,
    reduced_seconds: float | None = None,
    reason: str | None = None,
    error: str | None = None,
) -> None:
    """Attach the content-free envelope to ``event`` and emit observe.transcribed.

    Every outcome (transcribed / deferred / failed / filtered / preserved) flows
    through here so the envelope is built exactly once.  Fields are attached only
    when they are actually known; nothing is fabricated.  No transcript text,
    words, topics, setting, or emotion is ever carried.
    """
    event["outcome"] = outcome
    if backend:
        event["backend"] = backend

    # device: backend-reported value first, then parakeet.cpp's supervisor placement
    # when that record exists, then configured value. Deferred events still omit
    # model because resolving it can cost a CoreML helper subprocess.
    device = (model_info or {}).get("device")
    if not device:
        if _uses_parakeet_cpp(backend):
            from solstone.observe.transcribe import _parakeet_cpp

            device = _parakeet_cpp.resolve_serving_device(backend_config or {})
        else:
            device = (backend_config or {}).get("device")
    if device:
        event["device"] = device
    model = (model_info or {}).get("model")
    if model:
        event["model"] = model

    if audio_seconds is not None:
        event["audio_seconds"] = round(audio_seconds, 1)
    if reduced_seconds is not None:
        event["reduced_seconds"] = round(reduced_seconds, 1)
    if reason:
        event["reason"] = reason
    if error:
        event["error"] = error

    if timings is not None:
        stages = timings.as_dict()
        if stages:
            event["timings"] = stages
        asr_ms = timings.get_ms("asr")
        if outcome == "transcribed" and audio_seconds is not None and asr_ms:
            event["rtfx"] = round(audio_seconds / (asr_ms / 1000), 2)

    event["peak_rss_mib"] = _peak_rss_mib()
    callosum_send("observe", "transcribed", **event)


def _emit_deferred(
    raw_path: Path,
    vad_result: VadResult,
    segment: str | None,
    observer: str | None,
    *,
    reason: str,
    timings: _StageTimings,
    backend: str | None,
    backend_config: dict | None,
    audio_seconds: float | None,
    reduced_seconds: float | None,
) -> None:
    """Emit the honest-deferral event for a provider that could not do the work.

    Deliberately swallows its own failure: the caller must still exit
    EXIT_PROVIDER_BLOCKED so the input is preserved for retry even if the bus is
    down.  ``model`` is not carried -- see the note in _emit_transcribed.
    """
    try:
        event = _build_base_event(raw_path, vad_result, segment, observer)
        _emit_transcribed(
            event,
            outcome="deferred",
            timings=timings,
            backend=backend,
            backend_config=backend_config,
            audio_seconds=audio_seconds,
            reduced_seconds=reduced_seconds,
            reason=reason,
        )
    except Exception:
        logging.exception("Failed to emit transcription deferral event")


def _failure_reason(exc: Exception) -> str:
    """Machine-readable classification for a hard transcription failure.

    Provider errors already carry a reason code; anything else is labelled by its
    exception type.
    """
    if isinstance(exc, NativeSpeakerTranscriptWriteError):
        return exc.reason
    if isinstance(exc, SpeakerAnalyzeError):
        return SPEAKER_ANALYSIS_FAILURE_REASON
    if isinstance(exc, ParakeetProviderError):
        return exc.reason_code
    return type(exc).__name__


def _failure_label(exc: Exception) -> str:
    """The exception's type name -- the only part of it safe to put on the bus.

    Exception *messages* are not safe: SchemaValidationError embeds a preview of the
    raw model output (think/models.py), and provider wrappers may interpolate
    that into their own messages, so a message could carry transcript text onto the event.
    The full message and traceback go to the handler log, which is where the health
    UI already deep-links.  Keeping only the type name makes the content-free
    guarantee structural instead of a per-exception audit that any new provider
    error could quietly break.
    """
    if isinstance(exc, SpeakerAnalyzeError):
        return SPEAKER_ANALYSIS_FAILURE_LABEL
    return type(exc).__name__


def _build_base_event(
    audio_path: Path,
    vad_result: VadResult,
    segment: str | None = None,
    observer: str | None = None,
) -> dict:
    """Build base event dict for callosum emission.

    Args:
        audio_path: Path to the audio file
        vad_result: VAD result with speech detection info
        segment: Optional segment key (e.g., "143022_300")
        observer: Optional observer name

    Returns:
        Event dict with common fields for observe.transcribed events
    """
    journal_path = Path(get_journal())
    day = day_from_path(audio_path)

    try:
        rel_input = journal_relative_path(journal_path, audio_path)
    except ValueError:
        rel_input = audio_path

    event = {
        "input": str(rel_input),
        "vad_duration": round(vad_result.duration, 1),
        "vad_speech": round(vad_result.speech_duration, 1),
        "noisy": vad_result.is_noisy(),
    }

    # Add RMS values if available
    if vad_result.noisy_rms is not None:
        event["noisy_rms"] = round(vad_result.noisy_rms, 4)
        event["noisy_s"] = round(vad_result.noisy_s, 1)
    if vad_result.loud_windows > 0:
        event["loud_windows"] = vad_result.loud_windows
        event["speech_loud_windows"] = vad_result.speech_loud_windows
        ratio = vad_result.loud_speech_ratio
        if ratio is not None:
            event["loud_speech_ratio"] = round(ratio, 2)

    if day:
        event["day"] = day
    if segment:
        event["segment"] = segment
    if observer:
        event["observer"] = observer

    return event


def _native_exit_code(error: NativeSpeakerTranscriptWriteError) -> int:
    if error.reason in {
        "payload-unreadable",
        "payload-invalid",
        "payload-non-finite",
    }:
        return EXIT_PROVIDER_BLOCKED
    if error.reason in {"unsupported-host", "handshake-skip", "handshake-fail"}:
        return SPEAKERS_ANALYZE_EX_CONFIG
    if error.reason in {
        "launch-failed",
        "invalid-response",
        "orphan-npz-remove-failed",
        "payload-tempfile-failed",
    }:
        return 75
    if error.exit_code == 75:
        return 75
    return 1


def _log_native_write_failure(
    raw_path: Path, error: NativeSpeakerTranscriptWriteError
) -> None:
    logging.error("Native transcript write failed for %s: %s", raw_path, error)
    if error.partial_output:
        logging.warning("%s Input: %s", COMPOSED_COMMAND_WARNING, raw_path)


def _base_time_us_of_day(base_datetime: datetime.datetime) -> int:
    return (
        (base_datetime.hour * 3600 + base_datetime.minute * 60 + base_datetime.second)
        * 1_000_000
        + base_datetime.microsecond
    )


def _write_native_transcript(
    raw_path: Path,
    jsonl_path: Path,
    *,
    statements: list[dict],
    base_datetime: datetime.datetime,
    model_info: dict,
    source: str | None = None,
    observer: str | None = None,
    vad_result: VadResult | None = None,
    segment_meta: dict | None = None,
    backend: str | None = None,
    overlap_fraction: float | None = None,
    overlap_detector: str | None = None,
    speaker_evidence: SpeakerEvidenceDecision | None = None,
    processing_record: dict | None = None,
    sound_tags: dict | None = None,
    speaker_analysis_producer: str | None = None,
    embedding_payload: object | None = None,
    redo: bool = False,
) -> SpeakerTranscriptWriteResponse:
    """Route every transcript disposition through the sole native writer."""
    header: dict = {
        "raw": f"{raw_path.stem}{raw_path.suffix}",
        "backend": backend or "unknown",
        "model": model_info.get("model", "unknown"),
        "device": model_info.get("device", "unknown"),
        "compute_type": model_info.get("compute_type", "unknown"),
    }
    if observer:
        header["observer"] = observer

    if vad_result:
        header["duration"] = round(vad_result.duration, 2)
        header["noisy"] = vad_result.is_noisy()
        if vad_result.noisy_rms is not None:
            header["noisy_rms"] = round(vad_result.noisy_rms, 4)
            header["noisy_s"] = round(vad_result.noisy_s, 1)
        if vad_result.loud_windows > 0:
            header["loud_windows"] = vad_result.loud_windows
            header["speech_loud_windows"] = vad_result.speech_loud_windows
            ratio = vad_result.loud_speech_ratio
            if ratio is not None:
                header["loud_speech_ratio"] = round(ratio, 2)
    if overlap_fraction is not None and overlap_detector is not None:
        header["overlap_fraction"] = round(float(overlap_fraction), 4)
        header["overlap_detector"] = overlap_detector
    if speaker_evidence is not None:
        header["speaker_evidence"] = speaker_evidence.speaker_evidence
        header["speaker_evidence_multi_fraction"] = round(
            float(speaker_evidence.multi_window_fraction), 4
        )
        header["speaker_evidence_version"] = SPEAKER_EVIDENCE_VERSION
    if speaker_analysis_producer is not None:
        header["speaker_analysis_producer"] = speaker_analysis_producer
    if segment_meta:
        header["segment_meta"] = segment_meta
    if processing_record is not None:
        header["_solstone_processing"] = processing_record
    if sound_tags is not None:
        header["sound_tags"] = sound_tags

    native_statements: list[dict] = []
    for stmt in statements:
        start_seconds = stmt["start"] if stmt["start"] is not None else 0.0
        entry = {
            "id": stmt["id"],
            "start_offset_us": int(round(float(start_seconds) * 1_000_000)),
            "text": stmt["text"],
        }
        if "speaker" in stmt:
            entry["speaker"] = stmt["speaker"]
        native_statements.append(entry)

    payload = embedding_payload
    return write_speaker_transcript(
        jsonl_path=jsonl_path,
        npz_path=_get_embeddings_path(raw_path),
        base_time_us_of_day=_base_time_us_of_day(base_datetime),
        statements=native_statements,
        header=header,
        source=source,
        embedding_payload=getattr(payload, "payload", None),
        embedding_statement_ids=getattr(payload, "statement_ids", None),
        embedding_durations_s=getattr(payload, "durations_s", None),
        embedding_encoder=getattr(payload, "encoder", ENCODER_ID),
        redo=redo,
    )


def process_audio(
    raw_path: Path,
    audio_buffer: np.ndarray,
    vad_result: VadResult,
    backend_config: dict,
    redo: bool = False,
    reduction: AudioReduction | None = None,
    reduced_audio: np.ndarray | None = None,
    backend: str | None = None,
    *,
    sound_tags: dict | None = None,
    timings: _StageTimings | None = None,
) -> None:
    """Process a raw audio file with pre-computed VAD.

    This is the main orchestration function that coordinates:
    - STT backend dispatch
    - Native speaker analysis
    - Output file writing
    - Event emission

    Args:
        raw_path: Path to audio file in journal segment directory (HHMMSS_LEN/)
        audio_buffer: Full audio waveform (float32 mono at SAMPLE_RATE)
        vad_result: Pre-computed VAD result from run_vad()
        backend_config: Configuration for STT backend
        redo: If True, skip "already processed" check
        reduction: Optional AudioReduction mapping for timestamp restoration
        reduced_audio: Optional reduced audio buffer (used if reduction provided)
        backend: STT backend name. If omitted, uses DEFAULT_BACKEND.
        sound_tags: Optional ambient sound-tag metadata computed from full audio
        timings: Stage-timing accumulator carrying the pre-STT stages measured by
            _process_one. A fresh one is created when called without it.

    Raises:
        SystemExit: EXIT_PROVIDER_BLOCKED when the STT provider is not ready, the
            confidential lane refuses egress, or confidential audio exceeds the
            hosted duration cap -- an honest deferral that preserves the input for
            the next run. 1 on hard failure.
    """
    start_time = time.time()
    resolved_backend = backend or DEFAULT_BACKEND
    if timings is None:
        timings = _StageTimings()

    audio_seconds = len(audio_buffer) / SAMPLE_RATE
    reduced_seconds = (
        len(reduced_audio) / SAMPLE_RATE if reduced_audio is not None else None
    )
    model_info: dict = {}

    # Derive segment from path
    segment = get_segment_key(raw_path)

    # Skip if already processed (unless redo mode)
    jsonl_path = _get_jsonl_path(raw_path)
    if not redo and jsonl_path.exists():
        logging.info(f"Already processed: {raw_path}")
        return

    # Get observer name once for use in metadata and events
    observer = os.getenv("OBSERVER_NAME")

    # Get segment metadata (from sense.py via SEGMENT_META env var)
    segment_meta = None
    segment_meta_str = os.getenv("SEGMENT_META")
    if segment_meta_str:
        try:
            segment_meta = json.loads(segment_meta_str)
        except json.JSONDecodeError:
            logging.warning(f"Invalid SEGMENT_META JSON: {segment_meta_str[:100]}")

    if reduced_audio is not None:
        stt_buffer = reduced_audio
    else:
        stt_buffer = audio_buffer

    try:
        if (
            resolved_backend == "confidential"
            and audio_seconds > CONFIDENTIAL_STT_MAX_AUDIO_SECONDS
        ):
            logging.info(
                "Confidential STT cap exceeded (duration=%.1fs cap=%.1fs); deferring transcription",
                audio_seconds,
                CONFIDENTIAL_STT_MAX_AUDIO_SECONDS,
            )
            raise ConfidentialTranscribeDeferral("confidential_audio_too_long")

        # Dispatch to STT backend
        with timings.time("asr"):
            statements = stt_transcribe(
                resolved_backend, stt_buffer, SAMPLE_RATE, backend_config
            )

        # Get model info for metadata (dynamic import based on backend)
        backend_module = get_backend(resolved_backend)
        model_info = backend_module.get_model_info(backend_config)

        # Load config for preserve_all setting
        config = get_config()
        preserve_all = config.get("transcribe", {}).get("preserve_all", False)

        # Build base event fields (always emitted as observe.transcribed)
        event = _build_base_event(raw_path, vad_result, segment, observer)

        # Handle no speech detected
        if not statements:
            logging.info(
                "STT backend returned 0 statements, treating as silence "
                "(VAD: %.1fs speech of %.1fs)",
                vad_result.speech_duration,
                vad_result.duration,
            )
            # Routed: terminal-empty output is written only by solstone-core.
            with timings.time("write"):
                _write_native_transcript(
                    raw_path,
                    jsonl_path,
                    statements=[],
                    base_datetime=datetime.datetime.min,
                    model_info=model_info,
                    observer=observer,
                    vad_result=vad_result,
                    segment_meta=segment_meta,
                    backend=resolved_backend,
                    sound_tags=sound_tags,
                    processing_record=build_processing_record(
                        state=STATE_EMPTY,
                        reason_code=REASON_NO_DECODABLE_AUDIO,
                        handler=HANDLER_TRANSCRIBE,
                        input_size=raw_path.stat().st_size,
                    ),
                    redo=redo,
                )
            if preserve_all:
                outcome = "preserved"
                logging.info(
                    f"No speech detected in {raw_path}, preserving file "
                    f"(preserve_all=true, VAD: {vad_result.speech_duration:.1f}s "
                    f"of {vad_result.duration:.1f}s)"
                )
            else:
                outcome = "filtered"
                logging.info(
                    "No speech detected in %s, wrote terminal empty marker before "
                    "removing file (VAD: %.1fs speech of %.1fs)",
                    raw_path,
                    vad_result.speech_duration,
                    vad_result.duration,
                )
                raw_path.unlink()

            _emit_transcribed(
                event,
                outcome=outcome,
                timings=timings,
                backend=resolved_backend,
                model_info=model_info,
                backend_config=backend_config,
                audio_seconds=audio_seconds,
                reduced_seconds=reduced_seconds,
            )
            return

        # Extract date and time from path structure
        journal_path = Path(get_journal())
        day = day_from_path(raw_path)
        time_part = segment.split("_")[0] if segment else "000000"
        if day is None:
            logging.error(f"Could not extract day from path: {raw_path}")
            time_obj = datetime.datetime.strptime(time_part, "%H%M%S").time()
            base_dt = datetime.datetime.combine(datetime.date.today(), time_obj)
        else:
            base_dt = datetime.datetime.strptime(f"{day}_{time_part}", "%Y%m%d_%H%M%S")

        # Extract source from <source>_audio pattern
        source = None
        suffix = raw_path.stem
        if suffix.endswith("_audio") and suffix != "audio":
            source = suffix[:-6]  # Remove "_audio" suffix

        from solstone.observe.transcribe.speakers_analyze_adapter import (
            PRODUCER_ID as SPEAKERS_ANALYZE_PRODUCER_ID,
        )
        from solstone.observe.transcribe.speakers_analyze_adapter import (
            analyze_speakers,
        )

        def native_restored_statements() -> list[dict]:
            if not reduction:
                return statements
            from solstone.observe.vad import restore_statement_timestamps

            return restore_statement_timestamps(statements, reduction)

        with timings.time("speakers_analyze"):
            speaker_result = analyze_speakers(
                raw_path=raw_path,
                full_audio=audio_buffer,
                statement_audio=stt_buffer,
                reduced_audio=reduced_audio,
                statements_pre_restore=statements,
                statements_restored=native_restored_statements,
                sample_rate=SAMPLE_RATE,
                min_statement_duration=MIN_STATEMENT_DURATION,
            )
        statements = speaker_result.statements
        embedding_payload = speaker_result.embedding_payload
        speaker_evidence = speaker_result.speaker_evidence
        overlap_fraction_value = speaker_result.overlap_fraction
        speaker_analysis_producer = SPEAKERS_ANALYZE_PRODUCER_ID

        processing_record = build_processing_record(
            state=STATE_ANALYZED,
            reason_code=REASON_OK,
            handler=HANDLER_TRANSCRIBE,
            input_size=raw_path.stat().st_size,
        )
        # Routed: successful transcript output is written only by solstone-core.
        with timings.time("write"):
            native_response = _write_native_transcript(
                raw_path,
                jsonl_path,
                statements=statements,
                base_datetime=base_dt,
                model_info=model_info,
                source=source,
                observer=observer,
                vad_result=vad_result,
                segment_meta=segment_meta,
                backend=resolved_backend,
                overlap_fraction=overlap_fraction_value,
                overlap_detector=OVERLAP_DETECTOR_ID,
                speaker_evidence=speaker_evidence,
                processing_record=processing_record,
                sound_tags=sound_tags,
                speaker_analysis_producer=speaker_analysis_producer,
                embedding_payload=embedding_payload,
                redo=redo,
            )
        logging.info(f"Transcribed {raw_path} -> {jsonl_path}")

        if native_response.embedding_row_count > 0:
            logging.info("Saved embeddings: %s", native_response.npz_path)
            try:
                from solstone.apps.speakers.candidate_tracker import CandidateTracker

                tracker_day = day or day_from_path(raw_path)
                tracker_segment = segment or get_segment_key(raw_path)
                tracker_stream = raw_path.parent.parent.name
                if tracker_day and tracker_segment and tracker_stream:
                    CandidateTracker().process_segment(
                        day=tracker_day,
                        segment_key=tracker_segment,
                        stream=tracker_stream,
                        source=raw_path.stem,
                        seg_dir=raw_path.parent,
                    )
            except Exception:
                logging.warning(
                    "Speaker candidate tracking failed for %s",
                    raw_path,
                    exc_info=True,
                )
        else:
            logging.warning(f"No embeddings generated for {raw_path}")

        # Add completion fields and emit event
        event["duration_ms"] = int((time.time() - start_time) * 1000)
        try:
            rel_output = journal_relative_path(journal_path, jsonl_path)
        except ValueError:
            rel_output = jsonl_path
        event["output"] = rel_output

        _emit_transcribed(
            event,
            outcome="transcribed",
            timings=timings,
            backend=resolved_backend,
            model_info=model_info,
            backend_config=backend_config,
            audio_seconds=audio_seconds,
            reduced_seconds=reduced_seconds,
        )

    except ParakeetServerNotReady as e:
        # The STT provider is unreachable -- a deferral, not a failure.  Nothing has
        # been written, so the audio stays on disk and the next sense scan re-picks
        # it.  Exit blocked so sense records neither a success nor a failure.
        logging.info(
            "Parakeet server not ready for %s (%s); deferring for retry: %s",
            raw_path,
            e.retry_reason,
            e,
        )
        _emit_deferred(
            raw_path,
            vad_result,
            segment,
            observer,
            reason=e.retry_reason,
            timings=timings,
            backend=resolved_backend,
            backend_config=backend_config,
            audio_seconds=audio_seconds,
            reduced_seconds=reduced_seconds,
        )
        raise SystemExit(EXIT_PROVIDER_BLOCKED) from e

    except ConfidentialTranscribeDeferral as e:
        logging.info(
            "Confidential STT deferred for %s (%s)",
            raw_path,
            e.reason_code,
        )
        _emit_deferred(
            raw_path,
            vad_result,
            segment,
            observer,
            reason=e.reason_code,
            timings=timings,
            backend=resolved_backend,
            backend_config=backend_config,
            audio_seconds=audio_seconds,
            reduced_seconds=reduced_seconds,
        )
        raise SystemExit(EXIT_PROVIDER_BLOCKED) from e

    except ConfidentialAudioEgressError as e:
        logging.warning(
            "Confidential lane refused cloud STT for %s; deferring for retry: %s",
            raw_path,
            e,
        )
        _emit_deferred(
            raw_path,
            vad_result,
            segment,
            observer,
            reason="confidential_egress_blocked",
            timings=timings,
            backend=resolved_backend,
            backend_config=backend_config,
            audio_seconds=audio_seconds,
            reduced_seconds=reduced_seconds,
        )
        raise SystemExit(EXIT_PROVIDER_BLOCKED) from e

    except SpeakerAnalyzeError as e:
        logging.error(
            "Native speaker analysis failed for %s: %s",
            raw_path,
            e,
            exc_info=True,
        )
        try:
            event = _build_base_event(raw_path, vad_result, segment, observer)
            event.update(e.event_fields())
            _emit_transcribed(
                event,
                outcome="failed",
                timings=timings,
                backend=resolved_backend,
                model_info=model_info,
                backend_config=backend_config,
                audio_seconds=audio_seconds,
                reduced_seconds=reduced_seconds,
                reason=_failure_reason(e),
                error=_failure_label(e),
            )
        except Exception:
            logging.exception("Failed to emit transcription failure event")
        raise

    except NativeSpeakerTranscriptWriteError as e:
        _log_native_write_failure(raw_path, e)
        exit_code = _native_exit_code(e)
        try:
            event = _build_base_event(raw_path, vad_result, segment, observer)
            _emit_transcribed(
                event,
                outcome="deferred" if exit_code == EXIT_PROVIDER_BLOCKED else "failed",
                timings=timings,
                backend=resolved_backend,
                model_info=model_info,
                backend_config=backend_config,
                audio_seconds=audio_seconds,
                reduced_seconds=reduced_seconds,
                reason=e.reason,
                error=None
                if exit_code == EXIT_PROVIDER_BLOCKED
                else _failure_label(e),
            )
        except Exception:
            logging.exception("Failed to emit native transcript write failure event")
        raise SystemExit(exit_code) from e

    except Exception as e:
        logging.error(f"Failed to transcribe {raw_path}: {e}", exc_info=True)
        try:
            event = _build_base_event(raw_path, vad_result, segment, observer)
            _emit_transcribed(
                event,
                outcome="failed",
                timings=timings,
                backend=resolved_backend,
                model_info=model_info,
                backend_config=backend_config,
                audio_seconds=audio_seconds,
                reduced_seconds=reduced_seconds,
                reason=_failure_reason(e),
                error=_failure_label(e),
            )
        except Exception:
            logging.exception("Failed to emit transcription failure event")
        from solstone.think.models import IncompleteJSONError

        if isinstance(e, IncompleteJSONError) and e.partial_text:
            text = e.partial_text
            logging.error(f"Partial response ({len(text)} chars) HEAD: {text[:1000]}")
            logging.error(f"Partial response TAIL: {text[-1000:]}")
        raise SystemExit(1) from e


def _process_one(
    audio_path: Path,
    args: argparse.Namespace,
    transcribe_config: dict,
    default_backend: str,
) -> None:
    """Run the full transcription pipeline for a single audio file."""
    min_speech_seconds = transcribe_config.get(
        "min_speech_seconds", DEFAULT_MIN_SPEECH_SECONDS
    )
    preserve_all = transcribe_config.get("preserve_all", False)

    logging.info(f"Processing audio: {audio_path}")

    jsonl_path = _get_jsonl_path(audio_path)
    if not getattr(args, "redo", False) and jsonl_path.exists():
        logging.info(f"Already processed: {audio_path}")
        return

    from solstone.observe.vad import reduce_audio, run_vad

    timings = _StageTimings()
    queue_wait_ms = _read_queue_wait_ms()
    if queue_wait_ms is not None:
        timings.set_ms("queue_wait", queue_wait_ms)

    # Load audio once - handles M4A multi-stream mixing
    try:
        with timings.time("decode"):
            audio_buffer = load_audio(audio_path)
    except AudioDecodeError as e:
        logging.error("Failed to decode %s: %s", audio_path, e)
        journal_path = Path(get_journal())
        try:
            rel_input = journal_relative_path(journal_path, audio_path)
        except ValueError:
            rel_input = audio_path
        event = {"input": str(rel_input)}
        segment = get_segment_key(audio_path)
        day = day_from_path(audio_path)
        observer = os.getenv("OBSERVER_NAME")
        if day:
            event["day"] = day
        if segment:
            event["segment"] = segment
        if observer:
            event["observer"] = observer
        try:
            # Routed: terminal-failed output is written only by solstone-core.
            with timings.time("write"):
                _write_native_transcript(
                    audio_path,
                    jsonl_path,
                    statements=[],
                    base_datetime=datetime.datetime.min,
                    model_info={},
                    processing_record=build_processing_record(
                        state=STATE_FAILED,
                        reason_code=REASON_CORRUPT_INPUT,
                        handler=HANDLER_TRANSCRIBE,
                        input_size=audio_path.stat().st_size,
                    ),
                    redo=getattr(args, "redo", False),
                )
            _emit_transcribed(
                event,
                outcome="failed",
                timings=timings,
                reason=_failure_reason(e),
                error=_failure_label(e),
            )
        except NativeSpeakerTranscriptWriteError as native_error:
            _log_native_write_failure(audio_path, native_error)
            exit_code = _native_exit_code(native_error)
            try:
                _emit_transcribed(
                    event,
                    outcome="deferred" if exit_code == EXIT_PROVIDER_BLOCKED else "failed",
                    timings=timings,
                    reason=native_error.reason,
                    error=None
                    if exit_code == EXIT_PROVIDER_BLOCKED
                    else _failure_label(native_error),
                )
            except Exception:
                logging.exception("Failed to emit native decode-write failure event")
            raise SystemExit(exit_code) from native_error
        except Exception:
            logging.exception("Failed to emit decode failure event")
        return

    # Stage 1: Run VAD to detect speech (lightweight, before loading STT model)
    with timings.time("vad"):
        vad_result = run_vad(audio_buffer, min_speech_seconds=min_speech_seconds)
    try:
        sound_tags = tag_audio(audio_buffer, SAMPLE_RATE)
    except Exception as exc:
        logging.warning(
            "sound tagging failed for %s: %s",
            audio_path,
            exc,
            exc_info=True,
        )
        sound_tags = None

    # Early exit if no speech detected (skip loading heavy STT model)
    if not vad_result.has_speech:
        observer = os.getenv("OBSERVER_NAME")
        segment = get_segment_key(audio_path)
        event = _build_base_event(audio_path, vad_result, segment, observer)

        try:
            # Routed: terminal-empty output is written only by solstone-core.
            with timings.time("write"):
                _write_native_transcript(
                    audio_path,
                    _get_jsonl_path(audio_path),
                    statements=[],
                    base_datetime=datetime.datetime.min,
                    model_info={},
                    observer=observer,
                    vad_result=vad_result,
                    segment_meta=None,
                    backend=None,
                    sound_tags=sound_tags,
                    processing_record=build_processing_record(
                        state=STATE_EMPTY,
                        reason_code=REASON_NO_DECODABLE_AUDIO,
                        handler=HANDLER_TRANSCRIBE,
                        input_size=audio_path.stat().st_size,
                    ),
                    redo=getattr(args, "redo", False),
                )
        except NativeSpeakerTranscriptWriteError as error:
            _log_native_write_failure(audio_path, error)
            exit_code = _native_exit_code(error)
            _emit_transcribed(
                event,
                outcome="deferred" if exit_code == EXIT_PROVIDER_BLOCKED else "failed",
                timings=timings,
                audio_seconds=len(audio_buffer) / SAMPLE_RATE,
                reason=error.reason,
                error=None
                if exit_code == EXIT_PROVIDER_BLOCKED
                else _failure_label(error),
            )
            raise SystemExit(exit_code) from error
        if preserve_all:
            outcome = "preserved"
            logging.info(
                f"Insufficient speech in {audio_path}, preserving file "
                f"(preserve_all=true, VAD: {vad_result.speech_duration:.1f}s "
                f"of {vad_result.duration:.1f}s, threshold: {min_speech_seconds:.1f}s)"
            )
        else:
            outcome = "filtered"
            logging.info(
                "Insufficient speech in %s, wrote terminal empty marker before "
                "removing file (VAD: %.1fs of %.1fs, threshold: %.1fs)",
                audio_path,
                vad_result.speech_duration,
                vad_result.duration,
                min_speech_seconds,
            )
            audio_path.unlink()

        _emit_transcribed(
            event,
            outcome=outcome,
            timings=timings,
            audio_seconds=len(audio_buffer) / SAMPLE_RATE,
        )
        return

    # Stage 2: Reduce audio by trimming long silence gaps (>2s)
    # Skip reduction for noisy clips with >70% speech — the "silence" gaps are
    # mostly noise and VAD boundaries are less reliable, so process the full audio.
    if vad_result.is_noisy() and vad_result.speech_ratio >= 0.7:
        logging.info(
            f"  Skipping audio reduction: noisy clip with "
            f"{vad_result.speech_ratio:.0%} speech"
        )
        reduced_audio, reduction = None, None
    else:
        with timings.time("reduce"):
            reduced_audio, reduction = reduce_audio(audio_buffer, vad_result)

    # Stage 3: Determine backend and build backend config
    # CLI --backend flag overrides the invocation-level default
    backend = args.backend or default_backend

    # Get backend-specific config from nested structure
    if _uses_parakeet_cpp(backend):
        parakeet_cpp_config = transcribe_config.get("parakeet-cpp", {})
        backend_config = {k: v for k, v in parakeet_cpp_config.items() if k == "device"}
    elif backend == "parakeet":
        parakeet_config = transcribe_config.get("parakeet", {})
        backend_config = {
            k: v
            for k, v in parakeet_config.items()
            if k
            in (
                "model_version",
                "cache_dir",
                "timeout_sec",
                "device",
                "quantization",
            )
        }
    elif backend == "confidential":
        backend_config = {}
    else:
        # Unknown backend - let get_backend() raise the error
        backend_config = {}

    # Stage 4: Process audio with STT backend
    process_audio(
        audio_path,
        audio_buffer,
        vad_result,
        backend_config,
        redo=args.redo,
        reduction=reduction,
        reduced_audio=reduced_audio,
        backend=backend,
        sound_tags=sound_tags,
        timings=timings,
    )


def main():
    parser = argparse.ArgumentParser(
        description="Transcribe audio files using pluggable STT and native speaker analysis"
    )
    parser.add_argument(
        "audio_path",
        nargs="?",
        type=str,
        help="Path to audio file in journal segment directory, e.g. HHMMSS_LEN/audio.flac",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        dest="all",
        help="Batch-transcribe all unprocessed audio segments in the journal",
    )
    parser.add_argument(
        "--redo",
        action="store_true",
        help="Reprocess file, overwriting existing outputs",
    )
    parser.add_argument(
        "--backend",
        type=str,
        choices=list(BACKEND_REGISTRY.keys()),
        help="STT backend to use (overrides config and resource-aware auto default)",
    )
    args = setup_cli(parser)
    require_solstone()

    if args.all and args.audio_path:
        parser.error("--all and audio_path are mutually exclusive")
    if not args.all and not args.audio_path:
        parser.error("provide audio_path or --all")

    config = get_config()
    transcribe_config = config.get("transcribe", {})
    default_backend = resolve_default_backend(
        args,
        transcribe_config,
        journal_config=config,
    )
    from solstone.think.speakers_analyze_installation import (
        check_speakers_analyze_installation,
    )

    speakers_installation = check_speakers_analyze_installation()
    if not speakers_installation.ok:
        print(speakers_installation.message, file=sys.stderr)
        logging.error(speakers_installation.message)
        raise SystemExit(SPEAKERS_ANALYZE_EX_CONFIG)

    if args.all:
        processed = 0
        skipped = 0
        failed = 0
        deferred = 0

        for day_name, _day_path_str in sorted(day_dirs().items()):
            for _stream_name, _seg_key, seg_path in iter_segments(day_name):
                for audio_file in sorted(seg_path.iterdir()):
                    if audio_file.suffix.lower() not in SUPPORTED_AUDIO_FORMATS:
                        continue
                    jsonl_path = audio_file.with_suffix(".jsonl")
                    if jsonl_path.exists() and not args.redo:
                        logging.info(f"Skipping (already transcribed): {audio_file}")
                        skipped += 1
                        continue
                    try:
                        logging.info(f"Transcribing: {audio_file}")
                        _process_one(
                            audio_file,
                            args,
                            transcribe_config,
                            default_backend,
                        )
                        processed += 1
                    except SpeakerAnalyzeError:
                        logging.error(
                            "Native speaker analysis failed for %s",
                            audio_file,
                            exc_info=True,
                        )
                        failed += 1
                    except SystemExit as exit_signal:
                        # A provider deferral is per-file, not per-batch: the audio is
                        # preserved for the next run and the batch moves on. SystemExit
                        # is a BaseException, so the `except Exception` below cannot
                        # see it -- without this, one deferred clip aborts everything.
                        if exit_signal.code != EXIT_PROVIDER_BLOCKED:
                            raise
                        logging.info("Deferred (provider not ready): %s", audio_file)
                        deferred += 1
        summary = f"{processed} processed, {skipped} skipped (already transcribed)"
        if deferred:
            summary += f", {deferred} deferred (provider not ready, will retry)"
        if failed:
            summary += f", {failed} failed"
        print(summary)
        return

    audio_path = Path(args.audio_path)
    if not audio_path.exists():
        if audio_path.is_absolute():
            journal_relative = Path(get_journal()) / audio_path.as_posix().lstrip("/")
        else:
            journal_relative = resolve_journal_path(get_journal(), args.audio_path)
        if journal_relative.exists():
            audio_path = journal_relative
        else:
            parser.error(
                f"Audio file not found.\n"
                f"  Tried absolute:         {audio_path}\n"
                f"  Tried journal-relative: {journal_relative}"
            )

    if audio_path.suffix.lower() not in SUPPORTED_AUDIO_FORMATS:
        parser.error(
            f"Unsupported audio format: {audio_path.suffix}. "
            f"Supported formats: {', '.join(sorted(SUPPORTED_AUDIO_FORMATS))}"
        )

    segment = get_segment_key(audio_path)
    if segment is None:
        parser.error(
            f"Audio file must be in a segment directory (HHMMSS_LEN/), "
            f"but parent is: {audio_path.parent.name}"
        )

    try:
        _process_one(audio_path, args, transcribe_config, default_backend)
    except SpeakerAnalyzeError as exc:
        raise SystemExit(1) from exc


if __name__ == "__main__":
    main()
