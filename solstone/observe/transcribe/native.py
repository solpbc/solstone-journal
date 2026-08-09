# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Native transcript-writer boundary for the transcription handler.

The native command is the sole durable JSONL/NPZ writer.  This boundary raises
on every unavailable or failed outcome so callers cannot mistake a failed write
for a completed terminal marker and remove the raw media.
"""

from __future__ import annotations

import contextlib
import json
import logging
import os
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from solstone.apps.speakers.encoder_config import ENCODER_ID
from solstone.think import core_handshake
from solstone.think.indexer.native import runtime_has_solstone_core_wheel_coverage

REQUEST_SCHEMA = "solstone-speaker-transcript-write-request-v1"
RESPONSE_SCHEMA = "solstone-speaker-transcript-write-response-v1"
ERROR_SCHEMA = "solstone-speaker-transcript-write-error-v1"

EXIT_TEMPFAIL = 75

COMPOSED_COMMAND_WARNING = (
    "Native transcript write did not complete after publishing an embedding sidecar; "
    "the raw audio was preserved and the segment should be retried."
)


@dataclass(frozen=True)
class SpeakerTranscriptWriteResponse:
    jsonl_path: str
    npz_path: str
    statement_count: int
    embedding_row_count: int


class NativeSpeakerTranscriptWriteError(RuntimeError):
    """A native transcript writer outcome that must stop the handler."""

    def __init__(
        self,
        *,
        reason: str,
        message: str,
        detail: str | None = None,
        exit_code: int | None = None,
        partial_output: bool = False,
    ) -> None:
        super().__init__(message if detail is None else f"{message}: {detail}")
        self.reason = reason
        self.message = message
        self.detail = detail
        self.exit_code = exit_code
        self.partial_output = partial_output


def write_speaker_transcript(
    *,
    raw_path: Path,
    jsonl_path: Path,
    npz_path: Path,
    base_time_us_of_day: int,
    statements: list[dict[str, Any]],
    header: dict[str, Any],
    source: str | None,
    embedding_payload: bytes | None,
    embedding_statement_ids: list[int] | None,
    embedding_durations_s: list[float] | None,
    embedding_encoder: str | None,
    redo: bool,
) -> SpeakerTranscriptWriteResponse:
    """Write one transcript through ``solstone-core`` or raise a typed failure."""
    if not runtime_has_solstone_core_wheel_coverage():
        raise _error(
            "unsupported-host",
            "This host has no compatible native transcript writer.",
            exit_code=core_handshake.EX_CONFIG,
        )

    handshake = core_handshake.check_solstone_core_handshake()
    if handshake.status == "skip":
        raise _error(
            "handshake-skip",
            "The source checkout lacks installed solstone-core distribution metadata.",
            detail=handshake.message,
            exit_code=core_handshake.EX_CONFIG,
        )
    if handshake.status == "fail":
        raise _error(
            "handshake-fail",
            "The native transcript helper or installed version needs repair.",
            detail=handshake.message,
            exit_code=core_handshake.EX_CONFIG,
        )

    _remove_orphan_npz(jsonl_path, npz_path)
    payload = embedding_payload or b""
    statement_ids = embedding_statement_ids or []
    durations_s = embedding_durations_s or []
    encoder = embedding_encoder or ENCODER_ID

    payload_path: Path | None = None
    try:
        try:
            payload_path = _write_payload_file(payload)
        except OSError as exc:
            raise _error(
                "payload-tempfile-failed",
                "The native transcript payload file could not be prepared; retry later.",
                detail=str(exc),
                exit_code=EXIT_TEMPFAIL,
            ) from exc
        request = {
            "schema": REQUEST_SCHEMA,
            "output": {
                "jsonl_path": str(jsonl_path),
                "npz_path": str(npz_path),
                "redo": redo,
            },
            "base_time_us_of_day": base_time_us_of_day,
            "source": source,
            "statements": statements,
            "header": header,
            "embeddings": {
                "payload_path": str(payload_path),
                "payload_format": "raw-f32le-row-major-v1",
                "dtype": "float32-le",
                "shape": [len(statement_ids), 256],
                "byte_count": len(payload),
                "statement_ids": statement_ids,
                "durations_s": durations_s,
                "encoder": encoder,
            },
        }
        helper = core_handshake.helper_path_for_executable()
        try:
            completed = subprocess.run(
                [str(helper), "speaker-transcript-write"],
                input=json.dumps(request),
                text=True,
                capture_output=True,
                check=False,
            )
        except OSError as exc:
            raise _error(
                "launch-failed",
                "The native transcript helper could not be started; retry later.",
                detail=str(exc),
                exit_code=EXIT_TEMPFAIL,
            ) from exc

        if completed.returncode != 0:
            reason, detail = _parse_error(completed.stderr, completed.returncode)
            raise _error(
                reason,
                _message_for_reason(reason),
                detail=detail,
                exit_code=completed.returncode,
                partial_output=npz_path.exists() and not jsonl_path.exists(),
            )
        return _parse_response(completed.stdout)
    finally:
        if payload_path is not None:
            try:
                payload_path.unlink(missing_ok=True)
            except OSError:
                logging.warning(
                    "Could not remove temporary transcript embedding payload: %s",
                    payload_path,
                    exc_info=True,
                )


def _remove_orphan_npz(jsonl_path: Path, npz_path: Path) -> None:
    if not npz_path.exists() or jsonl_path.exists():
        return
    logging.warning(
        "Removing orphaned transcript embeddings sidecar before retry: %s", npz_path
    )
    try:
        npz_path.unlink()
    except OSError as exc:
        raise _error(
            "orphan-npz-remove-failed",
            "The stale transcript embedding sidecar could not be removed; retry later.",
            detail=str(exc),
            exit_code=EXIT_TEMPFAIL,
        ) from exc


def _write_payload_file(payload: bytes) -> Path:
    path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            prefix="solstone-speaker-transcript-", suffix=".f32le", delete=False
        ) as handle:
            path = Path(handle.name)
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        return path
    except OSError:
        if path is not None:
            with contextlib.suppress(OSError):
                path.unlink(missing_ok=True)
        raise


def _parse_error(stderr: str, returncode: int) -> tuple[str, str | None]:
    for line in reversed(stderr.splitlines()):
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            isinstance(value, dict)
            and value.get("schema") == ERROR_SCHEMA
            and isinstance(value.get("reason"), str)
        ):
            detail = value.get("detail")
            return value["reason"], detail if isinstance(detail, str) else None
    if returncode < 0:
        return f"signal-{abs(returncode)}", stderr.strip() or None
    return f"native-exit-{returncode}", stderr.strip() or None


def _parse_response(stdout: str) -> SpeakerTranscriptWriteResponse:
    try:
        value = json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise _error(
            "invalid-response",
            "The native transcript helper returned an unusable response; retry later.",
            detail=str(exc),
            exit_code=EXIT_TEMPFAIL,
        ) from exc
    if not isinstance(value, dict) or value.get("schema") != RESPONSE_SCHEMA:
        raise _error(
            "invalid-response",
            "The native transcript helper returned an unusable response; retry later.",
            exit_code=EXIT_TEMPFAIL,
        )
    try:
        jsonl_path = _required_string(value, "jsonl_path")
        npz_path = _required_string(value, "npz_path")
        statement_count = _required_count(value, "statement_count")
        embedding_row_count = _required_count(value, "embedding_row_count")
    except ValueError as exc:
        raise _error(
            "invalid-response",
            "The native transcript helper returned an unusable response; retry later.",
            detail=str(exc),
            exit_code=EXIT_TEMPFAIL,
        ) from exc
    return SpeakerTranscriptWriteResponse(
        jsonl_path=jsonl_path,
        npz_path=npz_path,
        statement_count=statement_count,
        embedding_row_count=embedding_row_count,
    )


def _required_string(value: dict[str, Any], key: str) -> str:
    item = value.get(key)
    if not isinstance(item, str):
        raise ValueError(f"response {key} must be a string")
    return item


def _required_count(value: dict[str, Any], key: str) -> int:
    item = value.get(key)
    if isinstance(item, bool) or not isinstance(item, int) or item < 0:
        raise ValueError(f"response {key} must be a non-negative integer")
    return item


def _error(
    reason: str,
    message: str,
    *,
    detail: str | None = None,
    exit_code: int | None = None,
    partial_output: bool = False,
) -> NativeSpeakerTranscriptWriteError:
    return NativeSpeakerTranscriptWriteError(
        reason=reason,
        message=message,
        detail=detail,
        exit_code=exit_code,
        partial_output=partial_output,
    )


def _message_for_reason(reason: str) -> str:
    if reason == "destination-exists":
        return "The transcript output already exists; use --redo only when reprocessing is intended."
    if reason == "invalid-output-path":
        return "The handler constructed invalid transcript output paths; report this problem."
    if reason in {
        "malformed-request",
        "unknown-schema",
        "missing-statement-id",
        "invalid-statement-id",
        "duplicate-statement-id",
        "invalid-statement",
        "invalid-header",
    }:
        return "The handler constructed an invalid native transcript request; report this problem."
    if reason in {"payload-unreadable", "payload-invalid", "payload-non-finite"}:
        return "Handler-generated embedding data was rejected; retry later or report the local processing failure."
    if reason in {"output-unwritable", "npz-verification-failed", "internal-error"}:
        return "The native transcript write failed temporarily; check local storage and retry."
    return "The native transcript writer failed; retry later or report the failure."
