# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""On-demand restore for media files removed by the offload pass."""

from __future__ import annotations

import hashlib
import logging
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal

from solstone.think import retention_executor
from solstone.think.backup.engine import (
    _RcloneUnavailable,
    _resolve_runtime,
    _ResticUnavailable,
    _runtime_backend,
    _session_args,
)
from solstone.think.backup.hosted import HostedCredsUnavailable
from solstone.think.backup.runner import reason_for_returncode, run_restic
from solstone.think.backup.state import (
    get_backup_config,
    record_restore_result,
)
from solstone.think.offload_ledger import (
    OffloadFile,
    SegmentOffloadSummary,
    append_restore_event,
    summarize_day,
    summarize_journal,
)
from solstone.think.offload_measurement import (
    RawMediaUsage,
    SuggestedOffloadDefaults,
    device_free_bytes,
    device_total_bytes,
    measure_raw_media_usage,
    suggest_offload_defaults,
)
from solstone.think.utils import DATE_RE, get_journal, resolve_segment_dir

logger = logging.getLogger(__name__)

RESTORE_RESERVE_BYTES = 1_000_000_000
OFFLOAD_RESTORE_TIMEOUT_SECONDS = 6 * 60 * 60
OFFLOAD_RESTORE_STATUSES = ("ok", "no_op", "refused", "degraded", "error")
OFFLOAD_RESTORE_REASONS = frozenset(
    {
        "auth_failed",
        "backup_not_ready",
        "failed",
        "insufficient_free_space",
        "ledger_degraded",
        "locked",
        "missing_file_after_restore",
        "nothing_to_restore",
        "repo_missing",
        "restic_unavailable",
        "rclone_unavailable",
        "segment_missing",
        "timeout",
        "verification_failed",
    }
)


@dataclass(frozen=True)
class OffloadRestoreSegmentResult:
    status: str
    reason: str | None
    day: str
    stream: str
    segment: str
    snapshot_id: str | None
    files_expected: int
    files_restored: int
    bytes_expected: int
    bytes_restored: int


@dataclass(frozen=True)
class OffloadRestoreResult:
    status: str
    reason: str | None
    scope: Literal["day", "all"]
    day: str | None
    segments_selected: int
    segments_restored: int
    files_expected: int
    files_restored: int
    bytes_expected: int
    bytes_restored: int
    details: tuple[OffloadRestoreSegmentResult, ...]


@dataclass(frozen=True)
class OffloadStatusMeasurement:
    usage: RawMediaUsage
    free_bytes: int
    total_bytes: int
    suggested_defaults: SuggestedOffloadDefaults


def restore_day(day: str) -> OffloadRestoreResult:
    _validate_day(day)
    summary = summarize_day(day)
    if summary.degraded:
        return _record_result(
            _base_result(
                status="error",
                reason="ledger_degraded",
                scope="day",
                day=day,
            )
        )
    return _record_result(_restore("day", day, tuple(summary.segments)))


def restore_all() -> OffloadRestoreResult:
    summary = summarize_journal()
    if summary.degraded:
        return _record_result(
            _base_result(
                status="error", reason="ledger_degraded", scope="all", day=None
            )
        )
    segments = tuple(segment for day in summary.days for segment in day.segments)
    return _record_result(_restore("all", None, segments))


def measure_offload_status() -> OffloadStatusMeasurement:
    usage = measure_raw_media_usage()
    total_bytes = device_total_bytes()
    return OffloadStatusMeasurement(
        usage=usage,
        free_bytes=device_free_bytes(),
        total_bytes=total_bytes,
        suggested_defaults=suggest_offload_defaults(total_bytes),
    )


def build_offload_status(
    measurement: OffloadStatusMeasurement | None = None,
) -> dict[str, Any]:
    measurement = measure_offload_status() if measurement is None else measurement
    config = get_backup_config()
    ledger = summarize_journal()
    raw_by_day = {
        day.day: {"raw_media_bytes": day.bytes, "raw_media_files": day.files}
        for day in measurement.usage.per_day
    }
    offloaded_by_day: dict[str, dict[str, Any]] = {}
    pending_by_day: dict[str, dict[str, int]] = {}
    for ledger_day in ledger.days:
        backup_only_bytes = 0
        backup_only_files = 0
        backup_only_segments = 0
        pending_release_bytes = 0
        pending_release_files = 0
        pending_release_segments = 0
        for segment in ledger_day.segments:
            if not segment.currently_offloaded:
                continue
            segment_dir = resolve_segment_dir(
                segment.day,
                stream=segment.stream,
                segment=segment.segment,
            )
            has_backup_only = False
            has_pending_release = False
            for file in segment.files:
                if (segment_dir / file.name).is_file():
                    pending_release_bytes += file.bytes
                    pending_release_files += 1
                    has_pending_release = True
                else:
                    backup_only_bytes += file.bytes
                    backup_only_files += 1
                    has_backup_only = True
            backup_only_segments += int(has_backup_only)
            pending_release_segments += int(has_pending_release)
        offloaded_by_day[ledger_day.day] = {
            "backup_only_bytes": backup_only_bytes,
            "backup_only_files": backup_only_files,
            "backup_only_segments": backup_only_segments,
            "degraded": ledger_day.degraded,
            "skipped_records": ledger_day.skipped_records,
            "unreadable_ledgers": list(ledger_day.unreadable_ledgers),
        }
        pending_by_day[ledger_day.day] = {
            "pending_release_bytes": pending_release_bytes,
            "pending_release_files": pending_release_files,
            "pending_release_segments": pending_release_segments,
        }

    pending_release_bytes = sum(
        day["pending_release_bytes"] for day in pending_by_day.values()
    )
    pending_release_files = sum(
        day["pending_release_files"] for day in pending_by_day.values()
    )
    pending_release_segments = sum(
        day["pending_release_segments"] for day in pending_by_day.values()
    )
    backup_only_bytes = sum(
        day["backup_only_bytes"] for day in offloaded_by_day.values()
    )
    backup_only_files = sum(
        day["backup_only_files"] for day in offloaded_by_day.values()
    )
    backup_only_segments = sum(
        day["backup_only_segments"] for day in offloaded_by_day.values()
    )
    days = sorted({*raw_by_day, *offloaded_by_day})

    return {
        "offload": config["offload"],
        "last_offload": config["last_offload"],
        "last_verification": config["last_verification"],
        "last_restore": config["last_restore"],
        "device": {
            "free_bytes": measurement.free_bytes,
            "total_bytes": measurement.total_bytes,
        },
        "suggested_defaults": {
            "budget_bytes": measurement.suggested_defaults.budget_bytes,
            "floor_bytes": measurement.suggested_defaults.floor_bytes,
        },
        "raw_media": {
            "total_bytes": measurement.usage.total_bytes - pending_release_bytes,
            "total_files": measurement.usage.total_files - pending_release_files,
        },
        "backup_only": {
            "total_bytes": backup_only_bytes,
            "total_files": backup_only_files,
            "total_segments": backup_only_segments,
            "total_days": sum(
                1
                for day in offloaded_by_day.values()
                if day["backup_only_segments"] > 0
            ),
            "degraded": ledger.degraded,
            "skipped_records": ledger.skipped_records,
            "unreadable_ledgers": list(ledger.unreadable_ledgers),
        },
        "pending_release": {
            "total_bytes": pending_release_bytes,
            "total_files": pending_release_files,
            "total_segments": pending_release_segments,
            "total_days": sum(
                1
                for day in pending_by_day.values()
                if day["pending_release_segments"] > 0
            ),
        },
        "days": [
            {
                "day": day,
                "raw_media_bytes": raw_by_day.get(day, {}).get("raw_media_bytes", 0)
                - pending_by_day.get(day, {}).get("pending_release_bytes", 0),
                "raw_media_files": raw_by_day.get(day, {}).get("raw_media_files", 0)
                - pending_by_day.get(day, {}).get("pending_release_files", 0),
                **offloaded_by_day.get(
                    day,
                    {
                        "backup_only_bytes": 0,
                        "backup_only_files": 0,
                        "backup_only_segments": 0,
                        "degraded": False,
                        "skipped_records": 0,
                        "unreadable_ledgers": [],
                    },
                ),
                **pending_by_day.get(
                    day,
                    {
                        "pending_release_bytes": 0,
                        "pending_release_files": 0,
                        "pending_release_segments": 0,
                    },
                ),
            }
            for day in days
        ],
    }


def _validate_day(day: str) -> None:
    if not isinstance(day, str) or DATE_RE.fullmatch(day) is None:
        raise ValueError("day must be in YYYYMMDD format")
    time.strptime(day, "%Y%m%d")


def _restore(
    scope: Literal["day", "all"],
    day: str | None,
    segments: tuple[SegmentOffloadSummary, ...],
) -> OffloadRestoreResult:
    selected = tuple(segment for segment in segments if segment.currently_offloaded)
    if not selected:
        return _base_result(
            status="no_op",
            reason="nothing_to_restore",
            scope=scope,
            day=day,
        )

    expected_bytes = sum(segment.offloaded_bytes for segment in selected)
    expected_files = sum(segment.offloaded_file_count for segment in selected)
    if not _space_available(expected_bytes):
        return _base_result(
            status="refused",
            reason="insufficient_free_space",
            scope=scope,
            day=day,
            segments_selected=len(selected),
            files_expected=expected_files,
            bytes_expected=expected_bytes,
        )

    runtime_result = _restore_runtime()
    if isinstance(runtime_result, OffloadRestoreResult):
        return _replace_counts(
            runtime_result,
            scope=scope,
            day=day,
            segments_selected=len(selected),
            files_expected=expected_files,
            bytes_expected=expected_bytes,
        )
    runtime = runtime_result

    details: list[OffloadRestoreSegmentResult] = []
    with _runtime_backend(
        runtime, scope="backup", operation="offload restore"
    ) as backend:
        if backend is None:
            return _base_result(
                status="error",
                reason="backup_not_ready",
                scope=scope,
                day=day,
                segments_selected=len(selected),
                files_expected=expected_files,
                bytes_expected=expected_bytes,
            )
        for segment in selected:
            detail = _restore_segment(runtime, backend, segment)
            details.append(detail)
            if detail.status == "error" and detail.reason not in {
                "missing_file_after_restore",
                "verification_failed",
            }:
                return _aggregate_result(
                    scope,
                    day,
                    selected,
                    tuple(details),
                    status="error",
                    reason=detail.reason,
                )

    failed_details = tuple(detail for detail in details if detail.status == "error")
    if failed_details:
        if not any(detail.status == "ok" for detail in details):
            return _aggregate_result(
                scope,
                day,
                selected,
                tuple(details),
                status="error",
                reason=failed_details[0].reason,
            )
        return _aggregate_result(
            scope,
            day,
            selected,
            tuple(details),
            status="degraded",
            reason="verification_failed",
        )
    return _aggregate_result(
        scope, day, selected, tuple(details), status="ok", reason=None
    )


def _restore_runtime() -> Any | OffloadRestoreResult:
    try:
        runtime = _resolve_runtime(scope="backup")
    except HostedCredsUnavailable:
        return _base_result(
            status="error", reason="backup_not_ready", scope="all", day=None
        )
    except _ResticUnavailable:
        return _base_result(
            status="error",
            reason="restic_unavailable",
            scope="all",
            day=None,
        )
    except _RcloneUnavailable:
        return _base_result(
            status="error",
            reason="rclone_unavailable",
            scope="all",
            day=None,
        )
    if runtime is None:
        return _base_result(
            status="error", reason="backup_not_ready", scope="all", day=None
        )
    return runtime


def _space_available(expected_bytes: int) -> bool:
    floor = get_backup_config()["offload"].get("floor_bytes")
    floor_bytes = floor if type(floor) is int else 0
    return (
        expected_bytes + max(floor_bytes, RESTORE_RESERVE_BYTES) <= device_free_bytes()
    )


def _restore_segment(
    runtime: Any,
    backend: Any,
    summary: SegmentOffloadSummary,
) -> OffloadRestoreSegmentResult:
    segment_dir = resolve_segment_dir(
        summary.day,
        stream=summary.stream,
        segment=summary.segment,
    )
    if not segment_dir.is_dir():
        return _segment_error(summary, "segment_missing")

    missing_before = tuple(
        file for file in summary.files if not (segment_dir / file.name).is_file()
    )
    if missing_before:
        include_args: list[str] = []
        for file in missing_before:
            include_args.extend(["--include", f"/{file.name}"])

        result = run_restic(
            _session_args(
                backend,
                [
                    "restore",
                    f"{summary.snapshot_id}:{segment_dir}",
                    "--target",
                    str(segment_dir),
                    *include_args,
                ],
            ),
            repository=backend.destination.repository,
            password=runtime.keys.daily_key,
            restic_path=runtime.restic_path,
            backend_env=backend.backend_env,
            json=True,
            timeout=OFFLOAD_RESTORE_TIMEOUT_SECONDS,
        )
        if result.returncode != 0:
            _rollback_attempted_files(segment_dir, missing_before)
            return _segment_error(summary, reason_for_returncode(result.returncode))

    reason = _verification_reason(segment_dir, summary.files)
    if reason is not None:
        _rollback_attempted_files(segment_dir, missing_before)
        return _segment_error(summary, reason)

    retention_executor.resolve_offload_mark(
        journal=str(get_journal()),
        day=summary.day,
        segment_dir=summary.segment,
        files=sorted(file.name for file in summary.files),
        stream=summary.stream,
    )
    append_restore_event(
        day=summary.day,
        stream=summary.stream,
        segment=summary.segment,
    )
    return OffloadRestoreSegmentResult(
        status="ok",
        reason=None,
        day=summary.day,
        stream=summary.stream,
        segment=summary.segment,
        snapshot_id=summary.snapshot_id,
        files_expected=summary.offloaded_file_count,
        files_restored=summary.offloaded_file_count,
        bytes_expected=summary.offloaded_bytes,
        bytes_restored=summary.offloaded_bytes,
    )


def _verification_reason(
    segment_dir: Path, files: tuple[OffloadFile, ...]
) -> str | None:
    for file in files:
        path = segment_dir / file.name
        if not path.is_file():
            return "missing_file_after_restore"
        try:
            stat = path.stat()
        except OSError:
            return "missing_file_after_restore"
        if stat.st_size != file.bytes:
            return "verification_failed"
        if _sha256(path) != file.sha256:
            return "verification_failed"
    return None


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while chunk := handle.read(64 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _rollback_attempted_files(
    segment_dir: Path, files: tuple[OffloadFile, ...]
) -> None:
    for file in files:
        path = segment_dir / file.name
        if not path.is_file():
            continue
        try:
            path.unlink()
        except OSError:
            logger.warning("offload restore rollback could not remove %s", path)


def _segment_error(
    summary: SegmentOffloadSummary,
    reason: str,
) -> OffloadRestoreSegmentResult:
    return OffloadRestoreSegmentResult(
        status="error",
        reason=reason,
        day=summary.day,
        stream=summary.stream,
        segment=summary.segment,
        snapshot_id=summary.snapshot_id,
        files_expected=summary.offloaded_file_count,
        files_restored=0,
        bytes_expected=summary.offloaded_bytes,
        bytes_restored=0,
    )


def _base_result(
    *,
    status: str,
    reason: str | None,
    scope: Literal["day", "all"],
    day: str | None,
    segments_selected: int = 0,
    files_expected: int = 0,
    bytes_expected: int = 0,
) -> OffloadRestoreResult:
    if status not in OFFLOAD_RESTORE_STATUSES:
        raise AssertionError(f"unknown offload restore status: {status}")
    if reason is not None and reason not in OFFLOAD_RESTORE_REASONS:
        raise AssertionError(f"unknown offload restore reason: {reason}")
    return OffloadRestoreResult(
        status=status,
        reason=reason,
        scope=scope,
        day=day,
        segments_selected=segments_selected,
        segments_restored=0,
        files_expected=files_expected,
        files_restored=0,
        bytes_expected=bytes_expected,
        bytes_restored=0,
        details=(),
    )


def _replace_counts(
    result: OffloadRestoreResult,
    *,
    scope: Literal["day", "all"],
    day: str | None,
    segments_selected: int,
    files_expected: int,
    bytes_expected: int,
) -> OffloadRestoreResult:
    return OffloadRestoreResult(
        status=result.status,
        reason=result.reason,
        scope=scope,
        day=day,
        segments_selected=segments_selected,
        segments_restored=result.segments_restored,
        files_expected=files_expected,
        files_restored=result.files_restored,
        bytes_expected=bytes_expected,
        bytes_restored=result.bytes_restored,
        details=result.details,
    )


def _aggregate_result(
    scope: Literal["day", "all"],
    day: str | None,
    selected: tuple[SegmentOffloadSummary, ...],
    details: tuple[OffloadRestoreSegmentResult, ...],
    *,
    status: str,
    reason: str | None,
) -> OffloadRestoreResult:
    if status not in OFFLOAD_RESTORE_STATUSES:
        raise AssertionError(f"unknown offload restore status: {status}")
    if reason is not None and reason not in OFFLOAD_RESTORE_REASONS:
        raise AssertionError(f"unknown offload restore reason: {reason}")
    ok_details = tuple(detail for detail in details if detail.status == "ok")
    return OffloadRestoreResult(
        status=status,
        reason=reason,
        scope=scope,
        day=day,
        segments_selected=len(selected),
        segments_restored=len(ok_details),
        files_expected=sum(segment.offloaded_file_count for segment in selected),
        files_restored=sum(detail.files_restored for detail in ok_details),
        bytes_expected=sum(segment.offloaded_bytes for segment in selected),
        bytes_restored=sum(detail.bytes_restored for detail in ok_details),
        details=details,
    )


def _record_result(result: OffloadRestoreResult) -> OffloadRestoreResult:
    record_restore_result(
        status=result.status,
        time=int(time.time()),
        reason=result.reason,
        scope=result.scope,
        day=result.day,
        segments_selected=result.segments_selected,
        segments_restored=result.segments_restored,
        files_expected=result.files_expected,
        files_restored=result.files_restored,
        bytes_expected=result.bytes_expected,
        bytes_restored=result.bytes_restored,
    )
    return result


__all__ = [
    "OFFLOAD_RESTORE_REASONS",
    "OFFLOAD_RESTORE_STATUSES",
    "OFFLOAD_RESTORE_TIMEOUT_SECONDS",
    "RESTORE_RESERVE_BYTES",
    "OffloadStatusMeasurement",
    "OffloadRestoreResult",
    "OffloadRestoreSegmentResult",
    "build_offload_status",
    "measure_offload_status",
    "restore_all",
    "restore_day",
]
