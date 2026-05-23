# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared per-modality data-state vocabulary."""

import json
import os
import time
from datetime import datetime, timezone
from enum import StrEnum
from pathlib import Path

ANALYZING_STALE_SECONDS = 1800


class DataState(StrEnum):
    """Read-only visibility state for modality data."""

    ANALYZED = "analyzed"
    PENDING = "pending"
    ANALYZING = "analyzing"
    FAILED = "failed"
    PURGED = "purged"
    ABSENT = "absent"


def _iso_z_now() -> str:
    return (
        datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    )


def _analyzing_path(seg_path: Path, modality: str) -> Path:
    return seg_path / f".analyzing_{modality}"


def _failed_path(seg_path: Path, modality: str) -> Path:
    return seg_path / f".analyze_failed_{modality}"


def _write_failed_marker(
    marker_path: Path,
    failed_path: Path,
    modality: str,
    reason: str,
    detail: str,
    payload: dict | None = None,
) -> None:
    failed_payload = {
        "started_at": (payload or {}).get("started_at", _iso_z_now()),
        "modality": modality,
        "reason": reason,
        "failed_at": _iso_z_now(),
        "detail": detail,
    }
    marker_path.write_text(json.dumps(failed_payload, sort_keys=True) + "\n")
    marker_path.replace(failed_path)


def create_analyzing_marker(seg_path: Path, modality: str) -> Path:
    """Atomically create a per-modality analyzing marker."""
    path = _analyzing_path(seg_path, modality)
    payload = {
        "started_at": _iso_z_now(),
        "modality": modality,
    }
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o644)
    try:
        os.write(fd, json.dumps(payload, sort_keys=True).encode("utf-8") + b"\n")
    finally:
        os.close(fd)
    return path


def derive_modality_state(
    seg_path: Path,
    modality: str,
    *,
    has_chunks: bool,
    has_jsonl: bool,
    has_raw: bool,
) -> str:
    """Resolve per-modality data state, reconciling analyzing sidecars.

    Read-path mutation contract (sanctioned by ACs 10, 11, 12):
      - chunks-win rescue: orphaned .analyzing_<m> is unlinked when has_chunks is True.
      - stale-reconcile: .analyzing_<m> older than ANALYZING_STALE_SECONDS is renamed
        to .analyze_failed_<m> with {"reason":"stale", ...}.
      - corrupt-JSON: unparseable .analyzing_<m> is renamed to .analyze_failed_<m>
        with {"reason":"marker_corrupt", ...} regardless of mtime.
    All writes touch ONLY the two sidecar files in seg_path; no chunks, no jsonl,
    no domain state. Documented exception to CLAUDE.md §7 L1/L6.
    """
    marker_path = _analyzing_path(seg_path, modality)
    failed_path = _failed_path(seg_path, modality)

    if has_chunks:
        marker_path.unlink(missing_ok=True)
        return DataState.ANALYZED.value

    if marker_path.is_file():
        try:
            payload = json.loads(marker_path.read_text(encoding="utf-8"))
            if not isinstance(payload, dict):
                raise ValueError("marker JSON must be an object")
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            _write_failed_marker(
                marker_path,
                failed_path,
                modality,
                "marker_corrupt",
                str(exc),
            )
            return DataState.FAILED.value

        try:
            marker_age = time.time() - marker_path.stat().st_mtime
        except OSError:
            marker_age = 0
        if marker_age > ANALYZING_STALE_SECONDS:
            _write_failed_marker(
                marker_path,
                failed_path,
                modality,
                "stale",
                f"analyzing marker older than {ANALYZING_STALE_SECONDS} seconds",
                payload,
            )
            return DataState.FAILED.value
        return DataState.ANALYZING.value

    if failed_path.is_file():
        return DataState.FAILED.value
    if has_jsonl or has_raw:
        return DataState.PENDING.value
    return DataState.ABSENT.value
