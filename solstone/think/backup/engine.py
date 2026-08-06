# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Scheduled restic backup and prune engine for solstone backup."""

from __future__ import annotations

import logging
import time
from collections.abc import Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from solstone.think.backup.destination import (
    Destination,
    assemble_backend_env,
)
from solstone.think.backup.hosted import (
    HostedBinding,
    HostedCredentials,
    HostedCredsUnavailable,
    fetch_hosted_credentials,
    load_hosted_binding,
    operated_destination,
)
from solstone.think.backup.hosted_provider import (
    HostedResticSession,
    hosted_append_only_restic_session,
    hosted_restic_session,
)
from solstone.think.backup.install import ensure_restic
from solstone.think.backup.rclone_install import ensure_rclone
from solstone.think.backup.runner import (
    reason_for_returncode,
    run_restic,
    select_summary,
)
from solstone.think.backup.state import (
    BackupKeys,
    get_backup_config,
    get_destination,
    get_keys,
    record_backup_result,
    record_prune_result,
    record_verification_result,
)
from solstone.think.callosum import callosum_send
from solstone.think.utils import get_journal

logger = logging.getLogger("solstone.backup.engine")

ARCHIVE_TAG = "solstone-archive"
ARCHIVE_BACKUP_TIMEOUT_SECONDS = 6 * 60 * 60
ARCHIVE_RETRY_LOCK = "30m"
ARCHIVE_LS_TIMEOUT_SECONDS = 30 * 60
BACKUP_EXCLUDES = (
    # Rebuildable derived data — never in snapshots (unchanged).
    "*.sqlite*",
    "indexer",
    "cache",
    ".cache",
    # Runtime ephemera under health/ at every depth. Bare "health" was removed:
    # restic matches a no-slash pattern by basename at ANY depth, so it dropped
    # the durable deletion audit (retention.log, pruning-runs/) and per-day
    # talent-provenance/ from every snapshot. These narrow basenames drop only
    # runtime ephemera; excluding temps is unconditionally correct (never data).
    # A segment mid-removal. The retention executor moves a segment aside under this
    # prefix, empties it there, and moves it back holding only its tombstone -- so a
    # snapshot taken during a removal would capture a PARTIALLY EMPTIED segment at a
    # path no iterator returns, and a restore would hand the owner data they cannot
    # see. ⛔ Matched by basename at any depth, which is what restic does with a
    # no-slash pattern and what this needs.
    ".removing_*",
    "*.sock",
    "*.pid",
    "*.port",
    "*.lock",
    "*.tmp",
    ".tmp*",
    "brain.json",
    "brain.log",
    "brain-fingerprint.key",
    "brain-refresh.lease",
    "supervisor.ready",
    "supervisor.start_time",
    "parakeet-cpp.placement",
    "scheduler.json",
)
PRUNE_MAX_REPACK_SIZE = "1G"
UNLOCK_TIMEOUT_SECONDS = 5 * 60
BACKUP_TIMEOUT_SECONDS = 6 * 60 * 60
INITIAL_BACKUP_TIMEOUT_SECONDS = 48 * 60 * 60
PRUNE_TIMEOUT_SECONDS = 2 * 60 * 60
BACKUP_MAX_RUNTIME = "49h"
PRUNE_MAX_RUNTIME = "3h"
BACKUP_RUN_CMD = ["journal", "maintenance", "run", "backup:run"]
VERIFY_RUN_CMD = ["journal", "maintenance", "run", "backup:verify"]
# restic check takes an exclusive lock, so long verification runs block the
# hourly backup; 1h bounds collateral to roughly one backup cycle and matches
# the 49h / 52 ~= 56m scale anchor for a fractional repository read-back.
VERIFY_TIMEOUT_SECONDS = 60 * 60
# Keep scheduler max_runtime above the subprocess timeout so run_restic can
# synthesize 124 -> timeout and record it before the scheduler kills the routine.
VERIFY_MAX_RUNTIME = "90m"


@dataclass(frozen=True)
class BackupResult:
    status: str
    snapshot_id: str | None
    error_reason: str | None


@dataclass(frozen=True)
class PruneResult:
    status: str
    error_reason: str | None


@dataclass(frozen=True)
class VerificationResult:
    status: str
    reason: str | None
    checked_subset: str | None


@dataclass(frozen=True)
class ArchiveFileVerdict:
    path: str
    confirmed: bool
    expected_size: int
    observed_size: int | None
    snapshot_id: str


@dataclass(frozen=True)
class ArchiveCheckResult:
    status: str
    error_reason: str | None
    verdicts: tuple[ArchiveFileVerdict, ...] | None


@dataclass(frozen=True)
class _Runtime:
    destination: Destination
    keys: BackupKeys
    restic_path: Path
    binding: HostedBinding | None = None
    hosted_credentials: HostedCredentials | None = None
    rclone_path: Path | None = None


class _ResticUnavailable(RuntimeError):
    """Raised when the pinned restic binary cannot be acquired."""


class _RcloneUnavailable(RuntimeError):
    """Raised when the hosted append-only adapter cannot be acquired."""


def _resolve_runtime(scope: str) -> _Runtime | None:
    config = get_backup_config()
    if config["enabled"] is not True:
        return None

    keys = get_keys()
    if keys is None:
        return None

    if config["mode"] == "operated":
        binding = load_hosted_binding()
        if binding is None:
            return None
        credential_scope = "operated" if scope == "backup" else scope
        creds = fetch_hosted_credentials(binding, scope=credential_scope)
        destination = operated_destination(binding, creds)
    else:
        binding = None
        creds = None
        destination = get_destination()
        if destination is None:
            return None

    try:
        restic_path = ensure_restic()
    except Exception as exc:
        raise _ResticUnavailable from exc

    if binding is not None and scope == "backup":
        try:
            rclone_path = ensure_rclone()
        except Exception as exc:
            raise _RcloneUnavailable from exc
    else:
        rclone_path = None

    return _Runtime(
        destination=destination,
        keys=keys,
        restic_path=restic_path,
        binding=binding,
        hosted_credentials=creds,
        rclone_path=rclone_path,
    )


def _backup_args() -> list[str]:
    args = ["backup", str(get_journal())]
    for pattern in BACKUP_EXCLUDES:
        args.extend(["--exclude", pattern])
    return args


def _archive_backup_args(paths: Sequence[Path]) -> list[str]:
    return [
        "--retry-lock",
        ARCHIVE_RETRY_LOCK,
        "backup",
        *[str(path) for path in paths],
        "--tag",
        ARCHIVE_TAG,
    ]


def _assemble_backend_env(
    destination: Destination,
    *,
    operation: str,
) -> dict[str, str] | None:
    try:
        return assemble_backend_env(destination)
    except (KeyError, ValueError):
        logger.warning(
            "backup %s backend config invalid returncode=%s reason_code=%s",
            operation,
            None,
            "failed",
        )
        return None


@contextmanager
def _runtime_backend(
    runtime: _Runtime,
    *,
    scope: str,
    operation: str,
) -> Iterator[HostedResticSession | None]:
    if runtime.binding is not None and runtime.hosted_credentials is not None:
        if scope == "backup":
            if runtime.rclone_path is None:
                yield None
                return
            with hosted_append_only_restic_session(
                runtime.binding,
                rclone_path=runtime.rclone_path,
                initial_credentials=runtime.hosted_credentials,
            ) as session:
                yield session
            return
        with hosted_restic_session(
            runtime.binding,
            initial_credentials=runtime.hosted_credentials,
        ) as session:
            yield session
        return

    backend_env = _assemble_backend_env(runtime.destination, operation=operation)
    if backend_env is None:
        yield None
        return
    yield HostedResticSession(
        destination=runtime.destination,
        backend_env=backend_env,
    )


def _session_args(
    session: HostedResticSession,
    args: list[str],
) -> list[str]:
    return [*session.global_options, *args]


def _backup_timeout() -> int:
    last_backup = get_backup_config()["last_backup"]
    snapshot_id = last_backup.get("snapshot_id")
    if (
        last_backup.get("status") != "ok"
        or not isinstance(snapshot_id, str)
        or not snapshot_id
    ):
        return INITIAL_BACKUP_TIMEOUT_SECONDS
    return BACKUP_TIMEOUT_SECONDS


def _recover_stale_lock(
    runtime: _Runtime,
    session: HostedResticSession,
) -> None:
    result = run_restic(
        _session_args(session, ["unlock"]),
        repository=session.destination.repository,
        password=runtime.keys.daily_key,
        restic_path=runtime.restic_path,
        backend_env=session.backend_env,
        timeout=UNLOCK_TIMEOUT_SECONDS,
    )
    if result.returncode == 0:
        logger.debug(
            "backup stale lock recovery completed returncode=%s reason_code=ok",
            result.returncode,
        )
        return

    logger.warning(
        "backup stale lock recovery completed returncode=%s reason_code=%s",
        result.returncode,
        reason_for_returncode(result.returncode),
    )


def _record_backup_error(
    *,
    reason: str,
    snapshot_id: str | None = None,
) -> BackupResult:
    record_backup_result(
        status="error",
        time=int(time.time()),
        snapshot_id=snapshot_id,
        error_reason=reason,
    )
    return BackupResult(status="error", snapshot_id=snapshot_id, error_reason=reason)


def _record_prune_error(*, reason: str) -> PruneResult:
    record_prune_result(
        status="error",
        time=int(time.time()),
        error_reason=reason,
    )
    return PruneResult(status="error", error_reason=reason)


def _archive_backup_error(
    *,
    reason: str,
    returncode: int | None = None,
) -> BackupResult:
    logger.warning(
        "backup archive completed returncode=%s reason_code=%s",
        returncode,
        reason,
    )
    return BackupResult(status="error", snapshot_id=None, error_reason=reason)


def _archive_check_error(
    *,
    reason: str,
    returncode: int | None = None,
) -> ArchiveCheckResult:
    logger.warning(
        "backup archive check completed returncode=%s reason_code=%s",
        returncode,
        reason,
    )
    return ArchiveCheckResult(status="error", error_reason=reason, verdicts=None)


def _archive_snapshot_id(records: list[object]) -> str | None:
    for record in records:
        if not isinstance(record, dict) or record.get("message_type") != "snapshot":
            continue
        snapshot_id = record.get("id")
        return snapshot_id if isinstance(snapshot_id, str) and snapshot_id else None
    return None


def _archive_node_sizes(records: list[object]) -> dict[str, int]:
    observed: dict[str, int] = {}
    for record in records:
        if not isinstance(record, dict) or record.get("message_type") != "node":
            continue
        path = record.get("path")
        size = record.get("size")
        if isinstance(path, str) and type(size) is int:
            observed[path] = size
    return observed


def _archive_file_verdicts(
    *,
    snapshot_id: str,
    expected_sizes: Mapping[Path, int],
    observed_sizes: Mapping[str, int],
) -> tuple[ArchiveFileVerdict, ...]:
    verdicts: list[ArchiveFileVerdict] = []
    for path, expected_size in expected_sizes.items():
        path_text = str(path)
        observed_size = observed_sizes.get(path_text)
        verdicts.append(
            ArchiveFileVerdict(
                path=path_text,
                confirmed=observed_size == expected_size,
                expected_size=expected_size,
                observed_size=observed_size,
                snapshot_id=snapshot_id,
            )
        )
    return tuple(verdicts)


def _verification_bucket_for_iso_week(week: int) -> int:
    return ((week - 1) % 52) + 1


def _verification_subset() -> str:
    week = datetime.fromtimestamp(time.time(), timezone.utc).isocalendar().week
    bucket = _verification_bucket_for_iso_week(week)
    return f"{bucket}/52"


def _verification_reason_for_returncode(returncode: int) -> str:
    if returncode == 1:
        return "integrity_failed"
    if returncode == 3:
        # restic check has no exit-3 semantic; avoid backup-specific "incomplete".
        return "failed"
    return reason_for_returncode(returncode)


def _record_verification_error(*, reason: str) -> VerificationResult:
    record_verification_result(
        status="error",
        time=int(time.time()),
        reason=reason,
        checked_subset=None,
    )
    return VerificationResult(status="error", reason=reason, checked_subset=None)


def run_backup() -> BackupResult:
    try:
        runtime = _resolve_runtime(scope="backup")
    except HostedCredsUnavailable as exc:
        logger.warning(
            "backup completed returncode=%s reason_code=%s",
            None,
            exc.reason_code,
        )
        return _record_backup_error(reason=exc.reason_code)
    except _ResticUnavailable:
        logger.warning(
            "backup completed returncode=%s reason_code=%s",
            None,
            "restic_unavailable",
        )
        return _record_backup_error(reason="restic_unavailable")
    except _RcloneUnavailable:
        logger.warning(
            "backup completed returncode=%s reason_code=%s",
            None,
            "rclone_unavailable",
        )
        return _record_backup_error(reason="rclone_unavailable")

    if runtime is None:
        return BackupResult(status="skipped", snapshot_id=None, error_reason=None)

    with _runtime_backend(runtime, scope="backup", operation="run") as backend:
        if backend is None:
            return _record_backup_error(reason="failed")
        _recover_stale_lock(runtime, backend)
        result = run_restic(
            _session_args(backend, _backup_args()),
            repository=backend.destination.repository,
            password=runtime.keys.daily_key,
            restic_path=runtime.restic_path,
            backend_env=backend.backend_env,
            json=True,
            timeout=_backup_timeout(),
        )
    summary = select_summary(result.json)
    snapshot_id = None
    if summary is not None:
        raw_snapshot_id = summary.get("snapshot_id")
        if isinstance(raw_snapshot_id, str) and raw_snapshot_id:
            snapshot_id = raw_snapshot_id

    if result.returncode == 0 and snapshot_id is not None:
        record_backup_result(
            status="ok",
            time=int(time.time()),
            snapshot_id=snapshot_id,
            error_reason=None,
        )
        logger.info(
            "backup completed returncode=%s reason_code=ok",
            result.returncode,
        )
        return BackupResult(status="ok", snapshot_id=snapshot_id, error_reason=None)

    reason = (
        "unknown"
        if result.returncode == 0
        else reason_for_returncode(result.returncode)
    )
    logger.warning(
        "backup completed returncode=%s reason_code=%s",
        result.returncode,
        reason,
    )
    partial_snapshot_id = snapshot_id if result.returncode == 3 else None
    return _record_backup_error(reason=reason, snapshot_id=partial_snapshot_id)


def run_verification() -> VerificationResult:
    try:
        runtime = _resolve_runtime(scope="backup")
    except HostedCredsUnavailable as exc:
        logger.warning(
            "backup verify completed returncode=%s reason_code=%s",
            None,
            exc.reason_code,
        )
        return _record_verification_error(reason=exc.reason_code)
    except _ResticUnavailable:
        logger.warning(
            "backup verify completed returncode=%s reason_code=%s",
            None,
            "restic_unavailable",
        )
        return _record_verification_error(reason="restic_unavailable")
    except _RcloneUnavailable:
        logger.warning(
            "backup verify completed returncode=%s reason_code=%s",
            None,
            "rclone_unavailable",
        )
        return _record_verification_error(reason="rclone_unavailable")

    if runtime is None:
        return VerificationResult(status="skipped", reason=None, checked_subset=None)

    checked_subset = _verification_subset()
    with _runtime_backend(runtime, scope="backup", operation="verify") as backend:
        if backend is None:
            return _record_verification_error(reason="failed")
        result = run_restic(
            _session_args(
                backend,
                ["check", "--read-data-subset", checked_subset],
            ),
            repository=backend.destination.repository,
            password=runtime.keys.daily_key,
            restic_path=runtime.restic_path,
            backend_env=backend.backend_env,
            timeout=VERIFY_TIMEOUT_SECONDS,
        )

    if result.returncode == 0:
        record_verification_result(
            status="ok",
            time=int(time.time()),
            reason=None,
            checked_subset=checked_subset,
        )
        logger.info(
            "backup verify completed returncode=%s reason_code=ok",
            result.returncode,
        )
        return VerificationResult(
            status="ok",
            reason=None,
            checked_subset=checked_subset,
        )

    reason = _verification_reason_for_returncode(result.returncode)
    logger.warning(
        "backup verify completed returncode=%s reason_code=%s",
        result.returncode,
        reason,
    )
    return _record_verification_error(reason=reason)


def run_archive_backup(paths: Sequence[Path]) -> BackupResult:
    """Snapshot explicit archive targets.

    Unlike run_backup(), every non-success result returns snapshot_id=None,
    including restic exit 3 with a parseable summary snapshot id.
    """
    try:
        runtime = _resolve_runtime(scope="backup")
    except HostedCredsUnavailable as exc:
        return _archive_backup_error(reason=exc.reason_code)
    except _ResticUnavailable:
        return _archive_backup_error(reason="restic_unavailable")
    except _RcloneUnavailable:
        return _archive_backup_error(reason="rclone_unavailable")

    if runtime is None:
        return BackupResult(status="skipped", snapshot_id=None, error_reason=None)

    with _runtime_backend(runtime, scope="backup", operation="archive") as backend:
        if backend is None:
            return _archive_backup_error(reason="failed")
        _recover_stale_lock(runtime, backend)
        result = run_restic(
            _session_args(backend, _archive_backup_args(paths)),
            repository=backend.destination.repository,
            password=runtime.keys.daily_key,
            restic_path=runtime.restic_path,
            backend_env=backend.backend_env,
            json=True,
            timeout=ARCHIVE_BACKUP_TIMEOUT_SECONDS,
        )

    summary = select_summary(result.json)
    snapshot_id = None
    if summary is not None:
        raw_snapshot_id = summary.get("snapshot_id")
        if isinstance(raw_snapshot_id, str) and raw_snapshot_id:
            snapshot_id = raw_snapshot_id

    if result.returncode == 0 and snapshot_id is not None:
        logger.info(
            "backup archive completed returncode=%s reason_code=ok",
            result.returncode,
        )
        return BackupResult(status="ok", snapshot_id=snapshot_id, error_reason=None)

    reason = (
        "unknown"
        if result.returncode == 0
        else reason_for_returncode(result.returncode)
    )
    return _archive_backup_error(reason=reason, returncode=result.returncode)


def check_archive_snapshot_files(
    snapshot_id: str,
    expected_sizes: Mapping[Path, int],
) -> ArchiveCheckResult:
    try:
        runtime = _resolve_runtime(scope="backup")
    except HostedCredsUnavailable as exc:
        return _archive_check_error(reason=exc.reason_code)
    except _ResticUnavailable:
        return _archive_check_error(reason="restic_unavailable")
    except _RcloneUnavailable:
        return _archive_check_error(reason="rclone_unavailable")

    if runtime is None:
        return ArchiveCheckResult(status="skipped", error_reason=None, verdicts=None)

    with _runtime_backend(
        runtime,
        scope="backup",
        operation="archive check",
    ) as backend:
        if backend is None:
            return _archive_check_error(reason="failed")
        result = run_restic(
            _session_args(backend, ["ls", "--long", snapshot_id]),
            repository=backend.destination.repository,
            password=runtime.keys.daily_key,
            restic_path=runtime.restic_path,
            backend_env=backend.backend_env,
            json=True,
            timeout=ARCHIVE_LS_TIMEOUT_SECONDS,
        )

    if result.returncode != 0:
        return _archive_check_error(
            reason=reason_for_returncode(result.returncode),
            returncode=result.returncode,
        )

    parsed = result.json
    if not isinstance(parsed, list):
        return _archive_check_error(reason="failed", returncode=result.returncode)

    checked_snapshot_id = _archive_snapshot_id(parsed)
    if checked_snapshot_id is None:
        return _archive_check_error(reason="failed", returncode=result.returncode)

    verdicts = _archive_file_verdicts(
        snapshot_id=checked_snapshot_id,
        expected_sizes=expected_sizes,
        observed_sizes=_archive_node_sizes(parsed),
    )
    logger.info(
        "backup archive check completed returncode=%s reason_code=ok",
        result.returncode,
    )
    return ArchiveCheckResult(status="ok", error_reason=None, verdicts=verdicts)


def run_prune() -> PruneResult:
    try:
        runtime = _resolve_runtime(scope="maintenance")
    except HostedCredsUnavailable as exc:
        logger.warning(
            "backup prune completed returncode=%s reason_code=%s",
            None,
            exc.reason_code,
        )
        return _record_prune_error(reason=exc.reason_code)
    except _ResticUnavailable:
        logger.warning(
            "backup prune completed returncode=%s reason_code=%s",
            None,
            "restic_unavailable",
        )
        return _record_prune_error(reason="restic_unavailable")

    if runtime is None:
        return PruneResult(status="skipped", error_reason=None)

    with _runtime_backend(runtime, scope="maintenance", operation="prune") as backend:
        if backend is None:
            return _record_prune_error(reason="failed")
        _recover_stale_lock(runtime, backend)
        retention = get_backup_config()["retention"]
        result = run_restic(
            _session_args(
                backend,
                [
                    "forget",
                    "--keep-hourly",
                    str(retention.get("hourly", 24)),
                    "--keep-daily",
                    str(retention.get("daily", 7)),
                    "--keep-weekly",
                    str(retention.get("weekly", 4)),
                    "--keep-monthly",
                    str(retention.get("monthly", 12)),
                    "--keep-tag",
                    ARCHIVE_TAG,
                    "--prune",
                ],
            ),
            repository=backend.destination.repository,
            password=runtime.keys.daily_key,
            restic_path=runtime.restic_path,
            backend_env=backend.backend_env,
            timeout=PRUNE_TIMEOUT_SECONDS,
            max_repack_size=PRUNE_MAX_REPACK_SIZE,
        )

    if result.returncode == 0:
        record_prune_result(
            status="ok",
            time=int(time.time()),
            error_reason=None,
        )
        logger.info(
            "backup prune completed returncode=%s reason_code=ok",
            result.returncode,
        )
        return PruneResult(status="ok", error_reason=None)

    reason = reason_for_returncode(result.returncode)
    logger.warning(
        "backup prune completed returncode=%s reason_code=%s",
        result.returncode,
        reason,
    )
    return _record_prune_error(reason=reason)


def request_backup_now() -> bool:
    return callosum_send("supervisor", "request", cmd=BACKUP_RUN_CMD)


def request_verification_now() -> bool:
    return callosum_send("supervisor", "request", cmd=VERIFY_RUN_CMD)


__all__ = [
    "ARCHIVE_BACKUP_TIMEOUT_SECONDS",
    "ARCHIVE_LS_TIMEOUT_SECONDS",
    "ARCHIVE_RETRY_LOCK",
    "ARCHIVE_TAG",
    "ArchiveCheckResult",
    "ArchiveFileVerdict",
    "BACKUP_MAX_RUNTIME",
    "BACKUP_TIMEOUT_SECONDS",
    "BackupResult",
    "INITIAL_BACKUP_TIMEOUT_SECONDS",
    "PRUNE_MAX_REPACK_SIZE",
    "PRUNE_MAX_RUNTIME",
    "PRUNE_TIMEOUT_SECONDS",
    "PruneResult",
    "UNLOCK_TIMEOUT_SECONDS",
    "VERIFY_MAX_RUNTIME",
    "VERIFY_RUN_CMD",
    "VERIFY_TIMEOUT_SECONDS",
    "VerificationResult",
    "check_archive_snapshot_files",
    "request_backup_now",
    "request_verification_now",
    "run_archive_backup",
    "run_backup",
    "run_prune",
    "run_verification",
]
