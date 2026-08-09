# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Restore engine hook for solstone backup."""

from __future__ import annotations

import logging
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

from solstone.think.backup.destination import Destination, assemble_backend_env
from solstone.think.backup.hosted import HostedBinding, HostedCredentials
from solstone.think.backup.hosted_provider import hosted_append_only_restic_session
from solstone.think.backup.install import ensure_restic
from solstone.think.backup.keys import parse_recovery_key
from solstone.think.backup.rclone_install import ensure_rclone
from solstone.think.backup.runner import (
    reason_for_returncode,
    run_restic,
    select_summary,
)
from solstone.think.backup.state import (
    get_backup_config,
    set_destination,
    set_mode,
    set_recovery_key,
    set_recovery_key_confirmed,
)
from solstone.think.body_native import BodyNativeError, rebuild_body_store
from solstone.think.indexer.journal import scan_journal
from solstone.think.utils import get_journal

logger = logging.getLogger("solstone.backup.restore")

RESTORE_LIST_TIMEOUT_SECONDS = 5 * 60
RESTORE_TIMEOUT_SECONDS = 48 * 60 * 60
RESTORE_CHECK_TIMEOUT_SECONDS = 6 * 60 * 60


@dataclass(frozen=True)
class RestoreResult:
    status: str
    reason_code: str | None
    integrity_ok: bool
    resumable: bool
    bytes_restored: int | None


def _restore_error(
    reason_code: str,
    *,
    returncode: int | None = None,
) -> RestoreResult:
    logger.warning(
        "backup restore completed returncode=%s reason_code=%s",
        returncode,
        reason_code,
    )
    return RestoreResult(
        status="error",
        reason_code=reason_code,
        integrity_ok=False,
        resumable=False,
        bytes_restored=None,
    )


def _original_path_from_snapshots(parsed: Any) -> str | None:
    if not isinstance(parsed, list) or not parsed:
        return None
    first = parsed[0]
    if not isinstance(first, dict):
        return None
    paths = first.get("paths")
    if not isinstance(paths, list) or not paths:
        return None
    original_path = paths[0]
    return original_path if isinstance(original_path, str) and original_path else None


def _bytes_restored(parsed: Any) -> int | None:
    summary = select_summary(parsed)
    if summary is None:
        return None
    value = summary.get("bytes_restored")
    return value if type(value) is int else None


def _run_restore(
    destination: Destination,
    entered_recovery_key: str,
    persist: Callable[[str], None],
    *,
    backend_env: Mapping[str, str | None] | None = None,
    global_options: tuple[str, ...] = (),
) -> RestoreResult:
    try:
        canonical = parse_recovery_key(entered_recovery_key)
    except ValueError:
        return _restore_error("invalid_key")

    if backend_env is None:
        try:
            backend_env = assemble_backend_env(destination)
        except (KeyError, ValueError):
            return _restore_error("failed")

    try:
        restic_path = ensure_restic()
    except Exception:
        return _restore_error("restic_unavailable")

    snapshots = run_restic(
        [*global_options, "snapshots", "latest"],
        repository=destination.repository,
        password=canonical,
        restic_path=restic_path,
        backend_env=backend_env,
        json=True,
        timeout=RESTORE_LIST_TIMEOUT_SECONDS,
    )
    if snapshots.returncode != 0:
        return _restore_error(
            reason_for_returncode(snapshots.returncode),
            returncode=snapshots.returncode,
        )

    original_path = _original_path_from_snapshots(snapshots.json)
    if original_path is None:
        return _restore_error("failed", returncode=snapshots.returncode)

    journal = get_journal()
    restore = run_restic(
        [
            *global_options,
            "restore",
            f"latest:{original_path}",
            "--target",
            str(journal),
        ],
        repository=destination.repository,
        password=canonical,
        restic_path=restic_path,
        backend_env=backend_env,
        json=True,
        timeout=RESTORE_TIMEOUT_SECONDS,
    )
    if restore.returncode != 0:
        return _restore_error(
            reason_for_returncode(restore.returncode),
            returncode=restore.returncode,
        )

    restored_size = _bytes_restored(restore.json)
    check = run_restic(
        [*global_options, "check"],
        repository=destination.repository,
        password=canonical,
        restic_path=restic_path,
        backend_env=backend_env,
        timeout=RESTORE_CHECK_TIMEOUT_SECONDS,
    )
    integrity_ok = check.returncode == 0
    if integrity_ok:
        status = "ok"
        reason_code = None
    else:
        status = "degraded"
        reason_code = (
            "integrity_unverified"
            if check.returncode in {11, 124}
            else "integrity_failed"
        )
        logger.warning(
            "backup restore integrity check completed returncode=%s reason_code=%s",
            check.returncode,
            reason_code,
        )

    try:
        rebuild_body_store(journal)
    except BodyNativeError:
        logger.warning("backup restore body-history rebuild failed")
        return _restore_error("body_rebuild_failed")

    persist(canonical)
    daily_key = get_backup_config()["daily_key"]
    resumable = isinstance(daily_key, str) and bool(daily_key)

    scan_journal(str(journal), full=True)

    logger.info(
        "backup restore completed returncode=%s reason_code=%s",
        restore.returncode,
        reason_code or "ok",
    )
    return RestoreResult(
        status=status,
        reason_code=reason_code,
        integrity_ok=integrity_ok,
        resumable=resumable,
        bytes_restored=restored_size,
    )


def restore_journal(
    destination: Destination,
    entered_recovery_key: str,
) -> RestoreResult:
    def persist(canonical: str) -> None:
        set_destination(destination)
        set_recovery_key(canonical)
        set_recovery_key_confirmed(True)

    return _run_restore(destination, entered_recovery_key, persist)


def restore_journal_operated(
    binding: HostedBinding,
    initial_credentials: HostedCredentials,
    entered_recovery_key: str,
) -> RestoreResult:
    try:
        parse_recovery_key(entered_recovery_key)
    except ValueError:
        return _restore_error("invalid_key")

    def persist(canonical: str) -> None:
        set_mode("operated")
        set_recovery_key(canonical)
        set_recovery_key_confirmed(True)

    try:
        rclone_path = ensure_rclone()
    except Exception:
        return _restore_error("rclone_unavailable")

    with hosted_append_only_restic_session(
        binding,
        rclone_path=rclone_path,
        initial_credentials=initial_credentials,
    ) as session:
        return _run_restore(
            session.destination,
            entered_recovery_key,
            persist,
            backend_env=session.backend_env,
            global_options=session.global_options,
        )


__all__ = [
    "RESTORE_CHECK_TIMEOUT_SECONDS",
    "RESTORE_LIST_TIMEOUT_SECONDS",
    "RESTORE_TIMEOUT_SECONDS",
    "RestoreResult",
    "restore_journal",
    "restore_journal_operated",
]
