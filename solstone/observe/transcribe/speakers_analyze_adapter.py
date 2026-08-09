# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native speakers-analyze adapter for transcribed audio segments."""

from __future__ import annotations

import json
import math
import os
import selectors
import shutil
import subprocess
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np

from solstone.apps.speakers.encoder_config import (
    ENCODER_ID,
    SPEAKERS_ANALYZE_DTYPE,
    SPEAKERS_ANALYZE_PAYLOAD_FORMAT,
    WESPEAKER_EMBEDDING_WIDTH,
)
from solstone.apps.speakers.evidence import (
    VALID_SPEAKER_EVIDENCE_DECISIONS,
    SpeakerEvidenceDecision,
)
from solstone.observe.transcribe.speakers_analyze_errors import SpeakerAnalyzeError
from solstone.think.model_assets import (
    resolve_pyannote_segmentation_model,
    resolve_wespeaker_model,
)
from solstone.think.speakers_analyze_installation import (
    speakers_analyze_path_for_executable,
)

REQUEST_SCHEMA = "solstone-speaker-analyze-request-v1"
RESPONSE_SCHEMA = "solstone-speaker-analyze-response-v1"
ERROR_SCHEMA = "solstone-speaker-analyze-error-v1"
PRODUCER_ID = "solstone-core-speakers-analyze-v1"
TEMP_ROOT = Path("/var/tmp")
TEMP_PREFIX = "solstone-speakers-analyze-"
TEMP_DIR_MODE = 0o700
TEMP_FILE_MODE = 0o600

RestoredStatements = list[dict[str, Any]] | Callable[[], list[dict[str, Any]]]
HelperLocator = Callable[[], Path]
ModelPathResolver = Callable[[], tuple[Path, Path]]
TempDirFactory = Callable[[Path], Path]
HelperInvoker = Callable[[list[str], str, Path], "HelperInvocationResult"]


@dataclass(frozen=True)
class SpeakerEmbeddingPayload:
    """Validated raw embedding bytes and their native transcript metadata."""

    payload: bytes
    statement_ids: list[int]
    durations_s: list[float]
    encoder: str


@dataclass(frozen=True)
class SpeakerAnalyzeResult:
    statements: list[dict[str, Any]]
    embedding_payload: SpeakerEmbeddingPayload | None
    speaker_evidence: SpeakerEvidenceDecision
    overlap_fraction: float
    statement_labels: list[int | None] | None


@dataclass(frozen=True)
class HelperInvocationResult:
    returncode: int
    stdout: str
    stderr: str


@dataclass(frozen=True)
class SpeakersAnalyzeBudget:
    timeout_s: float = 2400.0
    stdout_limit_bytes: int = 1024 * 1024
    stderr_limit_bytes: int = 64 * 1024
    terminate_grace_s: float = 5.0
    kill_grace_s: float = 5.0


DEFAULT_INVOCATION_BUDGET = SpeakersAnalyzeBudget()


def create_speakers_analyze_temp_dir(raw_path: Path) -> Path:
    day = _safe_temp_part(
        raw_path.parent.parent.parent.name if raw_path.parents else "x"
    )
    segment = _safe_temp_part(raw_path.parent.name)
    source = _safe_temp_part(raw_path.stem)
    prefix = f"{TEMP_PREFIX}{day}-{segment}-{source}-{os.getpid()}-"
    path = Path(tempfile.mkdtemp(prefix=prefix, dir=TEMP_ROOT))
    path.chmod(TEMP_DIR_MODE)
    return path


def sweep_stale_speakers_analyze_dirs(max_age_seconds: int = 86400) -> int:
    swept = 0
    now = time.time()
    for path in TEMP_ROOT.glob(f"{TEMP_PREFIX}*"):
        if not path.is_dir():
            continue
        try:
            age_seconds = now - path.stat().st_mtime
        except OSError:
            continue
        if age_seconds <= max_age_seconds:
            continue
        shutil.rmtree(path, ignore_errors=True)
        if not path.exists():
            swept += 1
    return swept


def analyze_speakers(
    *,
    raw_path: Path,
    full_audio: np.ndarray,
    statement_audio: np.ndarray,
    reduced_audio: np.ndarray | None,
    statements_pre_restore: list[dict[str, Any]],
    statements_restored: RestoredStatements,
    sample_rate: int,
    min_statement_duration: float,
    helper_locator: HelperLocator = speakers_analyze_path_for_executable,
    helper_invoker: HelperInvoker = lambda argv, stdin, path: (
        invoke_speakers_analyze_helper(argv, stdin, path)
    ),
    model_path_resolver: ModelPathResolver = lambda: (
        resolve_wespeaker_model(),
        resolve_pyannote_segmentation_model(),
    ),
    temp_dir_factory: TempDirFactory = create_speakers_analyze_temp_dir,
) -> SpeakerAnalyzeResult:
    temp_dir: Path | None = None
    try:
        wespeaker_model_path, pyannote_model_path = model_path_resolver()
        temp_dir = temp_dir_factory(raw_path)
        restored_statements = _realize_statements_restored(statements_restored)
        request, payload_path = _build_request(
            temp_dir=temp_dir,
            full_audio=full_audio,
            statement_audio=statement_audio,
            reduced_audio=reduced_audio,
            statements_pre_restore=statements_pre_restore,
            statements_restored=restored_statements,
            sample_rate=sample_rate,
            wespeaker_model_path=wespeaker_model_path,
            pyannote_model_path=pyannote_model_path,
        )
        request_ids = [int(statement["id"]) for statement in statements_pre_restore]
        expected_statement_ids = _request_admitted_statement_ids(
            statement_audio,
            statements_pre_restore,
            sample_rate=sample_rate,
            min_statement_duration=min_statement_duration,
        )
        completed = helper_invoker(
            [str(helper_locator())],
            json.dumps(request, sort_keys=True),
            raw_path,
        )
        _raise_for_returncode(raw_path, completed)
        try:
            response = json.loads(completed.stdout)
        except json.JSONDecodeError as exc:
            raise SpeakerAnalyzeError(
                path=raw_path, stage="parse", reason="malformed-response"
            ) from exc
        return _accepted_result_from_response(
            response,
            payload_path=payload_path,
            statements_restored=restored_statements,
            expected_statement_ids=expected_statement_ids,
            request_statement_ids=request_ids,
            sample_rate=sample_rate,
        )
    except SpeakerAnalyzeError:
        raise
    except NativePayloadError as exc:
        raise SpeakerAnalyzeError(
            path=raw_path, stage=exc.stage, reason=exc.reason
        ) from exc
    except OSError as exc:
        raise SpeakerAnalyzeError(
            path=raw_path, stage="request", reason=type(exc).__name__.lower()
        ) from exc
    finally:
        if temp_dir is not None:
            shutil.rmtree(temp_dir, ignore_errors=True)


def invoke_speakers_analyze_helper(
    argv: list[str],
    stdin_text: str,
    raw_path: Path,
    *,
    budget: SpeakersAnalyzeBudget = DEFAULT_INVOCATION_BUDGET,
    popen_factory=subprocess.Popen,
    selector_factory=selectors.DefaultSelector,
    clock: Callable[[], float] = time.monotonic,
) -> HelperInvocationResult:
    deadline = clock() + budget.timeout_s
    try:
        proc = popen_factory(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as exc:
        raise SpeakerAnalyzeError(
            path=raw_path, stage="invoke", reason=type(exc).__name__.lower()
        ) from exc
    assert proc.stdin is not None
    assert proc.stdout is not None
    assert proc.stderr is not None

    stdin_bytes = memoryview(stdin_text.encode("utf-8"))
    stdin_offset = 0
    stdin_open = True
    stdout = bytearray()
    stderr = bytearray()
    with selector_factory() as selector:
        os.set_blocking(proc.stdin.fileno(), False)
        os.set_blocking(proc.stdout.fileno(), False)
        os.set_blocking(proc.stderr.fileno(), False)
        if stdin_bytes:
            selector.register(proc.stdin, selectors.EVENT_WRITE, "stdin")
        else:
            proc.stdin.close()
            stdin_open = False
        selector.register(proc.stdout, selectors.EVENT_READ, "stdout")
        selector.register(proc.stderr, selectors.EVENT_READ, "stderr")
        while selector.get_map():
            remaining = deadline - clock()
            if remaining <= 0:
                reason = (
                    "stdin-write-timeout"
                    if stdin_open and stdin_offset < len(stdin_bytes)
                    else "timeout"
                )
                _terminate_and_reap(proc, budget)
                raise SpeakerAnalyzeError(
                    path=raw_path,
                    stage="invoke",
                    reason=reason,
                    native_exit_code=proc.returncode,
                )
            for key, _events in selector.select(timeout=min(0.1, remaining)):
                stream_name = key.data
                if stream_name == "stdin":
                    try:
                        written = os.write(
                            key.fileobj.fileno(),
                            stdin_bytes[stdin_offset : stdin_offset + 8192],
                        )
                    except BlockingIOError:
                        continue
                    except BrokenPipeError:
                        selector.unregister(key.fileobj)
                        key.fileobj.close()
                        stdin_open = False
                        continue
                    stdin_offset += written
                    if stdin_offset >= len(stdin_bytes):
                        selector.unregister(key.fileobj)
                        key.fileobj.close()
                        stdin_open = False
                    continue
                chunk = os.read(key.fileobj.fileno(), 8192)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                target = stdout if stream_name == "stdout" else stderr
                limit = (
                    budget.stdout_limit_bytes
                    if stream_name == "stdout"
                    else budget.stderr_limit_bytes
                )
                if len(target) + len(chunk) > limit:
                    _terminate_and_reap(proc, budget)
                    raise SpeakerAnalyzeError(
                        path=raw_path,
                        stage="invoke",
                        reason=f"{stream_name}-too-large",
                        native_exit_code=proc.returncode,
                    )
                target.extend(chunk)
    returncode = proc.wait()
    return HelperInvocationResult(
        returncode=returncode,
        stdout=stdout.decode("utf-8", errors="replace"),
        stderr=stderr.decode("utf-8", errors="replace"),
    )


def _terminate_and_reap(proc, budget: SpeakersAnalyzeBudget) -> None:
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=budget.terminate_grace_s)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=budget.kill_grace_s)


def _raise_for_returncode(raw_path: Path, completed: HelperInvocationResult) -> None:
    if completed.returncode == 0:
        return
    if completed.returncode < 0:
        reason = f"signal-{abs(completed.returncode)}"
    else:
        reason = _helper_reason(completed.stderr) or f"exit-{completed.returncode}"
    raise SpeakerAnalyzeError(
        path=raw_path,
        stage="invoke",
        reason=reason,
        native_exit_code=completed.returncode,
    )


def _realize_statements_restored(
    statements_restored: RestoredStatements,
) -> list[dict[str, Any]]:
    if callable(statements_restored):
        return statements_restored()
    return statements_restored


def _build_request(
    *,
    temp_dir: Path,
    full_audio: np.ndarray,
    statement_audio: np.ndarray,
    reduced_audio: np.ndarray | None,
    statements_pre_restore: list[dict[str, Any]],
    statements_restored: list[dict[str, Any]],
    sample_rate: int,
    wespeaker_model_path: Path,
    pyannote_model_path: Path,
) -> tuple[dict[str, Any], Path]:
    full_audio_path = temp_dir / "full-audio.f32le"
    _write_f32le(full_audio_path, full_audio)
    reduced_audio_path: Path | None = None
    if reduced_audio is not None:
        reduced_audio_path = temp_dir / "reduced-audio.f32le"
        _write_f32le(reduced_audio_path, reduced_audio)
    payload_path = temp_dir / "statement-embeddings.f32le"

    statement_spans = _spans_from_statements(statements_pre_restore)
    diarization_spans = _spans_from_statements(statements_restored)
    _ensure_span_parity(statement_spans, diarization_spans)

    request: dict[str, Any] = {
        "schema": REQUEST_SCHEMA,
        "sample_rate_hz": sample_rate,
        "full_audio_f32le_path": str(full_audio_path),
        "models": {
            "pyannote_segmentation_onnx_path": str(pyannote_model_path),
            "wespeaker_onnx_path": str(wespeaker_model_path),
        },
        "output_payload_f32le_path": str(payload_path),
        "interval_embedding_payload_f32le_path": None,
        "statement_embedding": {"spans": statement_spans},
        "diarization": {"spans": diarization_spans},
    }
    if reduced_audio_path is not None:
        request["reduced_audio_f32le_path"] = str(reduced_audio_path)
    return request, payload_path


def _write_f32le(path: Path, audio: np.ndarray) -> None:
    data = np.asarray(audio, dtype="<f4").tobytes()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, TEMP_FILE_MODE)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def _spans_from_statements(statements: list[dict[str, Any]]) -> list[dict[str, Any]]:
    spans: list[dict[str, Any]] = []
    seen: set[int] = set()
    for statement in statements:
        statement_id = int(statement["id"])
        if statement_id in seen:
            raise NativePayloadError("request", "duplicate-statement-id")
        seen.add(statement_id)
        spans.append(
            {
                "statement_id": statement_id,
                "start_s": _optional_float(statement.get("start")),
                "end_s": _optional_float(statement.get("end")),
            }
        )
    return spans


def _optional_float(value: object) -> float | None:
    if isinstance(value, bool) or value is None:
        return None
    if not isinstance(value, int | float):
        return None
    value = float(value)
    return value if math.isfinite(value) else None


def _ensure_span_parity(
    statement_spans: list[dict[str, Any]], diarization_spans: list[dict[str, Any]]
) -> None:
    if len(statement_spans) != len(diarization_spans):
        raise NativePayloadError("request", "span-parity-length")
    for left, right in zip(statement_spans, diarization_spans):
        if left["statement_id"] != right["statement_id"]:
            raise NativePayloadError("request", "span-parity-statement-id")


def _request_admitted_statement_ids(
    audio: np.ndarray,
    statements: list[dict[str, Any]],
    *,
    sample_rate: int,
    min_statement_duration: float,
) -> list[int]:
    audio_duration = len(audio) / sample_rate
    admitted: list[int] = []
    for statement in statements:
        start = statement.get("start")
        end = statement.get("end")
        if start is None or end is None:
            continue
        if not isinstance(start, int | float) or not isinstance(end, int | float):
            continue
        start = max(0.0, min(float(start), audio_duration))
        end = max(0.0, min(float(end), audio_duration))
        if end - start < min_statement_duration:
            continue
        start_sample = int(start * sample_rate)
        end_sample = int(end * sample_rate)
        if end_sample - start_sample < int(min_statement_duration * sample_rate):
            continue
        admitted.append(int(statement["id"]))
    if len(admitted) != len(set(admitted)):
        raise NativePayloadError("request", "duplicate-admitted-statement-id")
    return admitted


def _accepted_result_from_response(
    response: object,
    *,
    payload_path: Path,
    statements_restored: list[dict[str, Any]],
    expected_statement_ids: list[int],
    request_statement_ids: list[int],
    sample_rate: int,
) -> SpeakerAnalyzeResult:
    if not isinstance(response, dict):
        raise NativePayloadError("parse", "response-not-object")
    if response.get("schema") != RESPONSE_SCHEMA:
        raise NativePayloadError("parse", "unknown-schema")
    if response.get("sample_rate_hz") != sample_rate:
        raise NativePayloadError("payload", "sample-rate-mismatch")

    for key in (
        "inputs",
        "statement_embeddings",
        "pyannote",
        "evidence",
        "diarization",
    ):
        if key not in response:
            raise NativePayloadError("payload", f"missing-{_reason_key(key)}")
    _validate_inputs(response, request_statement_ids)
    _validate_pyannote(response)

    statement_embeddings = _required_object(response, "statement_embeddings")
    _require_equal(statement_embeddings, "audio_buffer", {"full", "reduced"})
    _require_value(statement_embeddings, "encoder", ENCODER_ID)
    _require_value(
        statement_embeddings,
        "payload_format",
        SPEAKERS_ANALYZE_PAYLOAD_FORMAT,
    )
    _require_value(statement_embeddings, "payload_path", str(payload_path))
    _require_value(statement_embeddings, "dtype", SPEAKERS_ANALYZE_DTYPE)
    statement_ids = _required_int_list(statement_embeddings, "statement_ids")
    if len(statement_ids) != len(set(statement_ids)):
        raise NativePayloadError("payload", "duplicate-statement-id")
    if any(
        statement_id not in set(request_statement_ids) for statement_id in statement_ids
    ):
        raise NativePayloadError("payload", "foreign-statement-id")
    if statement_ids != expected_statement_ids:
        raise NativePayloadError("payload", "statement-id-divergence")
    durations_s = _required_float_list(statement_embeddings, "durations_s")
    if any(not math.isfinite(duration) for duration in durations_s):
        raise NativePayloadError("payload", "nonfinite-duration")
    rows = len(statement_ids)
    if len(durations_s) != rows:
        raise NativePayloadError("payload", "duration-count-mismatch")
    shape = statement_embeddings.get("shape")
    if shape != [rows, WESPEAKER_EMBEDDING_WIDTH]:
        raise NativePayloadError("payload", "embedding-shape-mismatch")
    expected_bytes = rows * WESPEAKER_EMBEDDING_WIDTH * 4
    if _required_int(statement_embeddings, "byte_count") != expected_bytes:
        raise NativePayloadError("payload", "embedding-byte-count-mismatch")
    if _required_int(statement_embeddings, "admitted_count") != rows:
        raise NativePayloadError("payload", "embedding-admitted-count-mismatch")
    skipped_count = _required_int(statement_embeddings, "skipped_count")
    if skipped_count != len(request_statement_ids) - rows:
        raise NativePayloadError("payload", "embedding-skipped-count-mismatch")

    payload_bytes = _read_payload_bytes(payload_path, expected_bytes)
    embedding_payload: SpeakerEmbeddingPayload | None
    if rows > 0:
        embeddings = np.frombuffer(payload_bytes, dtype="<f4").reshape(
            (rows, WESPEAKER_EMBEDDING_WIDTH)
        )
        if not np.isfinite(embeddings).all():
            raise NativePayloadError("payload", "nonfinite-embedding")
        embedding_payload = SpeakerEmbeddingPayload(
            payload=payload_bytes,
            statement_ids=statement_ids,
            durations_s=durations_s,
            encoder=ENCODER_ID,
        )
    else:
        embedding_payload = None

    evidence = _required_object(response, "evidence")
    decision = _required_str(evidence, "speaker_evidence")
    if decision not in VALID_SPEAKER_EVIDENCE_DECISIONS:
        raise NativePayloadError("payload", "unknown-speaker-evidence")
    multi_window_fraction = _fraction(evidence, "multi_window_fraction")
    mean_window_overlap_share = _fraction(evidence, "mean_window_overlap_share")
    overlap_fraction = _fraction(evidence, "overlap_fraction")
    speaker_evidence = SpeakerEvidenceDecision(
        speaker_evidence=decision,
        multi_window_fraction=multi_window_fraction,
        mean_window_overlap_share=mean_window_overlap_share,
    )

    statements = [dict(statement) for statement in statements_restored]
    _validate_diarization_keys(response)
    labels = _statement_labels(response)
    if labels is not None:
        if len(labels) != len(statements):
            raise NativePayloadError("payload", "statement-label-count-mismatch")
        for statement, label in zip(statements, labels):
            if label is not None:
                statement["speaker"] = int(label)

    return SpeakerAnalyzeResult(
        statements=statements,
        embedding_payload=embedding_payload,
        speaker_evidence=speaker_evidence,
        overlap_fraction=overlap_fraction,
        statement_labels=labels,
    )


def _validate_inputs(
    response: dict[str, Any], request_statement_ids: list[int]
) -> None:
    inputs = _required_object(response, "inputs")
    for section_name in ("statement_embedding", "diarization"):
        section = _required_object(inputs, section_name)
        if _required_int_list(section, "statement_ids") != request_statement_ids:
            raise NativePayloadError(
                "payload", f"{_reason_key(section_name)}-input-id-mismatch"
            )
        spans_s = section.get("spans_s")
        if not isinstance(spans_s, list) or len(spans_s) != len(request_statement_ids):
            raise NativePayloadError(
                "payload", f"invalid-{_reason_key(section_name)}-spans"
            )
        for span in spans_s:
            if not isinstance(span, list) or len(span) != 2:
                raise NativePayloadError(
                    "payload", f"invalid-{_reason_key(section_name)}-spans"
                )
            for value in span:
                if value is None:
                    continue
                if (
                    isinstance(value, bool)
                    or not isinstance(value, int | float)
                    or not math.isfinite(float(value))
                ):
                    raise NativePayloadError(
                        "payload", f"invalid-{_reason_key(section_name)}-spans"
                    )


def _validate_pyannote(response: dict[str, Any]) -> None:
    pyannote = _required_object(response, "pyannote")
    window_stats = pyannote.get("window_stats")
    if not isinstance(window_stats, list):
        raise NativePayloadError("payload", "invalid-pyannote-window-stats")
    for item in window_stats:
        if not isinstance(item, dict):
            raise NativePayloadError("payload", "invalid-pyannote-window-stats")
        for key in ("speech_frames", "active_slot_count", "overlap_frames"):
            value = item.get(key)
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                raise NativePayloadError("payload", "invalid-pyannote-window-stats")


def _validate_diarization_keys(response: dict[str, Any]) -> None:
    diarization = _required_object(response, "diarization")
    for key in (
        "intervals",
        "valid_intervals",
        "interval_embeddings",
        "cluster_labels",
        "statement_labels",
        "silhouette_k",
        "effective_k",
    ):
        if key not in diarization:
            raise NativePayloadError(
                "payload", f"missing-diarization-{_reason_key(key)}"
            )
    if diarization["interval_embeddings"] is not None:
        raise NativePayloadError("payload", "unexpected-interval-embeddings")


def _required_object(container: dict[str, Any], key: str) -> dict[str, Any]:
    value = container.get(key)
    if not isinstance(value, dict):
        raise NativePayloadError("payload", f"missing-{_reason_key(key)}")
    return value


def _required_int(container: dict[str, Any], key: str) -> int:
    value = container.get(key)
    if isinstance(value, bool) or not isinstance(value, int):
        raise NativePayloadError("payload", f"invalid-{_reason_key(key)}")
    return int(value)


def _required_int_list(container: dict[str, Any], key: str) -> list[int]:
    value = container.get(key)
    if not isinstance(value, list) or any(
        isinstance(item, bool) or not isinstance(item, int) for item in value
    ):
        raise NativePayloadError("payload", f"invalid-{_reason_key(key)}")
    return [int(item) for item in value]


def _required_float_list(container: dict[str, Any], key: str) -> list[float]:
    value = container.get(key)
    if not isinstance(value, list) or any(
        isinstance(item, bool) or not isinstance(item, int | float) for item in value
    ):
        raise NativePayloadError("payload", f"invalid-{_reason_key(key)}")
    return [float(item) for item in value]


def _required_str(container: dict[str, Any], key: str) -> str:
    value = container.get(key)
    if not isinstance(value, str):
        raise NativePayloadError("payload", f"invalid-{_reason_key(key)}")
    return value


def _fraction(container: dict[str, Any], key: str) -> float:
    value = container.get(key)
    if isinstance(value, bool) or not isinstance(value, int | float):
        raise NativePayloadError("payload", f"invalid-{_reason_key(key)}")
    value = float(value)
    if not math.isfinite(value) or value < 0.0 or value > 1.0:
        raise NativePayloadError("payload", f"invalid-{_reason_key(key)}")
    return value


def _require_value(container: dict[str, Any], key: str, expected: object) -> None:
    if container.get(key) != expected:
        raise NativePayloadError("payload", f"invalid-{_reason_key(key)}")


def _require_equal(
    container: dict[str, Any], key: str, expected_values: set[str]
) -> None:
    value = container.get(key)
    if value not in expected_values:
        raise NativePayloadError("payload", f"invalid-{_reason_key(key)}")


def _reason_key(key: str) -> str:
    return key.replace("_", "-")


def _read_payload_bytes(path: Path, expected_bytes: int) -> bytes:
    try:
        actual_bytes = path.stat().st_size
    except OSError as exc:
        raise NativePayloadError("payload", "embedding-payload-missing") from exc
    if actual_bytes != expected_bytes:
        raise NativePayloadError("payload", "embedding-payload-size-mismatch")
    return path.read_bytes()


def _statement_labels(response: dict[str, Any]) -> list[int | None] | None:
    diarization = _required_object(response, "diarization")
    value = diarization.get("statement_labels")
    if value is None:
        return None
    if not isinstance(value, list):
        raise NativePayloadError("payload", "invalid-statement-labels")
    labels: list[int | None] = []
    for item in value:
        if item is None:
            labels.append(None)
        elif isinstance(item, bool) or not isinstance(item, int) or int(item) <= 0:
            raise NativePayloadError("payload", "invalid-statement-labels")
        else:
            labels.append(int(item))
    return labels


def _helper_reason(stderr: str) -> str | None:
    for line in stderr.splitlines():
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            isinstance(payload, dict)
            and payload.get("schema") == ERROR_SCHEMA
            and isinstance(payload.get("reason"), str)
        ):
            return str(payload["reason"])
    return None


def _safe_temp_part(value: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch in ("-", "_") else "_" for ch in value)
    return cleaned[:80] or "x"


class NativePayloadError(RuntimeError):
    def __init__(self, stage: str, reason: str) -> None:
        super().__init__(reason)
        self.stage = stage
        self.reason = reason


__all__ = [
    "DEFAULT_INVOCATION_BUDGET",
    "PRODUCER_ID",
    "RESPONSE_SCHEMA",
    "SpeakerEmbeddingPayload",
    "SpeakerAnalyzeResult",
    "SpeakersAnalyzeBudget",
    "analyze_speakers",
    "create_speakers_analyze_temp_dir",
    "invoke_speakers_analyze_helper",
    "sweep_stale_speakers_analyze_dirs",
]
