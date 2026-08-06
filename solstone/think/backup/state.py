# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Journal config accessors for solstone backup state."""

from __future__ import annotations

import copy
from dataclasses import dataclass
from typing import Any

from solstone.think.backup.destination import Destination
from solstone.think.backup.hosted import load_hosted_binding
from solstone.think.backup.keys import (
    format_recovery_key_display,
    generate_daily_key,
    generate_recovery_key,
)
from solstone.think.journal_config import (
    JournalConfigMutation,
    mutate_journal_config,
    read_journal_config,
)

BACKUP_DEFAULTS: dict[str, Any] = {
    "enabled": False,
    "mode": "byo",
    "destination": {
        "repository": None,
        "backend": None,
        "credentials": {},
    },
    "daily_key": None,
    "recovery_key": None,
    "confirmed_recovery_key": False,
    "retention": {
        "hourly": 24,
        "daily": 7,
        "weekly": 4,
        "monthly": 12,
    },
    "offload": {
        "enabled": False,
        "budget_bytes": None,
        "floor_bytes": None,
    },
    "schedule": {
        "every": "daily",
        "enabled": False,
    },
    "last_backup": {
        "time": None,
        "snapshot_id": None,
        "status": None,
        "error_reason": None,
    },
    "last_prune": {
        "time": None,
        "status": None,
        "error_reason": None,
    },
    # Offload records use "reason" instead of "error_reason" because skipped
    # and stalled are expected outcomes, not errors.
    "last_offload": {
        "time": None,
        "status": None,
        "reason": None,
        "last_ok_time": None,
        "files_marked": 0,
        "bytes_marked": 0,
        "ran_out_of_markable_media": False,
    },
    "last_verification": {
        "time": None,
        "status": None,
        "reason": None,
        "last_ok_time": None,
        "checked_subset": None,
    },
    "last_restore": {
        "time": None,
        "status": None,
        "reason": None,
        "scope": None,
        "day": None,
        "segments_selected": 0,
        "segments_restored": 0,
        "files_expected": 0,
        "files_restored": 0,
        "bytes_expected": 0,
        "bytes_restored": 0,
    },
}
OFFLOAD_KEYS = ("enabled", "budget_bytes", "floor_bytes")
OFFLOAD_STATUSES = ("ok", "skipped", "stalled", "error")
RETENTION_KEYS = ("hourly", "daily", "weekly", "monthly")
RESTORE_STATUSES = ("ok", "no_op", "refused", "degraded", "error")
VERIFICATION_STATUSES = ("ok", "skipped", "error")


@dataclass(frozen=True)
class BackupKeys:
    daily_key: str
    recovery_key: str
    recovery_key_display: str


def _merge_defaults(defaults: dict[str, Any], raw: Any) -> dict[str, Any]:
    merged = copy.deepcopy(defaults)
    if not isinstance(raw, dict):
        return merged
    for key, value in raw.items():
        if isinstance(merged.get(key), dict) and isinstance(value, dict):
            merged[key] = _merge_defaults(merged[key], value)
        else:
            merged[key] = value
    return merged


def _writable_backup_section(config: dict[str, Any]) -> dict[str, Any]:
    backup = config.get("backup")
    if not isinstance(backup, dict):
        backup = {}
        config["backup"] = backup
    return backup


def _build_backup_keys(daily_key: Any, recovery_key: Any) -> BackupKeys | None:
    if daily_key is None or recovery_key is None:
        return None
    if not isinstance(daily_key, str) or not isinstance(recovery_key, str):
        raise ValueError("backup keys must be strings when present")
    return BackupKeys(
        daily_key=daily_key,
        recovery_key=recovery_key,
        recovery_key_display=format_recovery_key_display(recovery_key),
    )


def merge_backup_config(config: dict[str, Any]) -> dict[str, Any]:
    return _merge_defaults(BACKUP_DEFAULTS, config.get("backup", {}))


def get_backup_config() -> dict[str, Any]:
    config = read_journal_config()
    return merge_backup_config(config)


def get_destination() -> Destination | None:
    destination = get_backup_config()["destination"]
    repository = destination.get("repository")
    backend = destination.get("backend")
    credentials = destination.get("credentials", {})
    if repository is None or backend is None:
        return None
    if not isinstance(repository, str) or not isinstance(backend, str):
        raise ValueError("backup destination repository and backend must be strings")
    if not isinstance(credentials, dict):
        raise ValueError("backup destination credentials must be a JSON object")
    return Destination(
        repository=repository,
        backend=backend,
        credentials=dict(credentials),
    )


def get_keys() -> BackupKeys | None:
    config = get_backup_config()
    return _build_backup_keys(config["daily_key"], config["recovery_key"])


def generate_and_store_keys() -> BackupKeys:
    generated_daily_key = generate_daily_key()
    generated_recovery_key = generate_recovery_key()

    def apply(config: dict[str, Any]) -> JournalConfigMutation[BackupKeys]:
        backup = _writable_backup_section(config)
        daily_key = backup.get("daily_key")
        recovery_key = backup.get("recovery_key")
        changed = False
        if daily_key is None:
            daily_key = generated_daily_key
            changed = True
        if recovery_key is None:
            recovery_key = generated_recovery_key
            changed = True
        backup["daily_key"] = daily_key
        backup["recovery_key"] = recovery_key
        keys = _build_backup_keys(daily_key, recovery_key)
        if keys is None:
            raise RuntimeError("backup key generation failed")
        return JournalConfigMutation(changed=changed, value=keys)

    return mutate_journal_config(apply).value


def set_destination(destination: Destination) -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        next_destination = {
            "repository": destination.repository,
            "backend": destination.backend,
            "credentials": dict(destination.credentials),
        }
        changed = backup.get("destination") != next_destination
        backup["destination"] = next_destination
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def set_enabled(enabled: bool) -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        changed = backup.get("enabled") != enabled
        backup["enabled"] = enabled
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def set_mode(mode: str) -> None:
    if mode not in {"byo", "operated"}:
        raise ValueError("backup mode must be byo or operated")

    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        changed = backup.get("mode") != mode
        backup["mode"] = mode
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def set_recovery_key_confirmed(confirmed: bool = True) -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        changed = backup.get("confirmed_recovery_key") != confirmed
        backup["confirmed_recovery_key"] = confirmed
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def set_retention(retention: dict[str, int]) -> None:
    if not isinstance(retention, dict):
        raise ValueError("backup retention must be a JSON object")
    if set(retention) != set(RETENTION_KEYS):
        raise ValueError("backup retention must include hourly, daily, weekly, monthly")
    for key in RETENTION_KEYS:
        value = retention[key]
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise ValueError("backup retention values must be non-negative integers")

    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        next_retention = {key: int(retention[key]) for key in RETENTION_KEYS}
        changed = backup.get("retention") != next_retention
        backup["retention"] = next_retention
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def set_offload(offload: dict[str, Any]) -> None:
    if not isinstance(offload, dict):
        raise ValueError("backup offload must be a JSON object")
    if set(offload) != set(OFFLOAD_KEYS):
        raise ValueError(
            "backup offload must include enabled, budget_bytes, floor_bytes"
        )
    if not isinstance(offload["enabled"], bool):
        raise ValueError("backup offload enabled must be a boolean")
    for key in ("budget_bytes", "floor_bytes"):
        value = offload[key]
        if value is not None and (type(value) is not int or value <= 0):
            raise ValueError(
                "backup offload byte values must be positive integers or null"
            )

    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        next_offload = {
            "enabled": offload["enabled"],
            "budget_bytes": offload["budget_bytes"],
            "floor_bytes": offload["floor_bytes"],
        }
        changed = backup.get("offload") != next_offload
        backup["offload"] = next_offload
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def set_recovery_key(recovery_key: str) -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        changed = backup.get("recovery_key") != recovery_key
        backup["recovery_key"] = recovery_key
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def clear_backup_config() -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        next_backup = copy.deepcopy(BACKUP_DEFAULTS)
        changed = config.get("backup") != next_backup
        config["backup"] = next_backup
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def record_backup_result(
    *,
    status: str,
    time: int | None,
    snapshot_id: str | None = None,
    error_reason: str | None = None,
) -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        next_result = {
            "time": time,
            "snapshot_id": snapshot_id,
            "status": status,
            "error_reason": error_reason,
        }
        changed = backup.get("last_backup") != next_result
        backup["last_backup"] = next_result
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def record_prune_result(
    *,
    status: str,
    time: int | None,
    error_reason: str | None = None,
) -> None:
    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        next_result = {
            "time": time,
            "status": status,
            "error_reason": error_reason,
        }
        changed = backup.get("last_prune") != next_result
        backup["last_prune"] = next_result
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def record_offload_result(
    *,
    status: str,
    time: int | None,
    reason: str | None = None,
    files_marked: int,
    bytes_marked: int,
    ran_out_of_markable_media: bool,
) -> None:
    """Record the last media-offload run.

    Convention: reason is None on ok. This layer only enforces the closed
    status vocabulary; callers own richer run-state validation.
    """
    if status not in OFFLOAD_STATUSES:
        raise ValueError("backup offload status must be ok, skipped, stalled, or error")

    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        prior = backup.get("last_offload")
        prior_last_ok_time = (
            prior.get("last_ok_time") if isinstance(prior, dict) else None
        )
        last_ok_time = time if status == "ok" else prior_last_ok_time
        next_result = {
            "time": time,
            "status": status,
            "reason": reason,
            "last_ok_time": last_ok_time,
            "files_marked": files_marked,
            "bytes_marked": bytes_marked,
            "ran_out_of_markable_media": ran_out_of_markable_media,
        }
        changed = backup.get("last_offload") != next_result
        backup["last_offload"] = next_result
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def record_verification_result(
    *,
    status: str,
    time: int | None,
    reason: str | None = None,
    checked_subset: str | None = None,
) -> None:
    """Record the last backup repository read-back verification run.

    Convention: reason is None on ok. This layer only enforces the closed
    status vocabulary; callers own richer run-state validation.
    """
    if status not in VERIFICATION_STATUSES:
        raise ValueError("backup verification status must be ok, skipped, or error")

    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        prior = backup.get("last_verification")
        prior_last_ok_time = (
            prior.get("last_ok_time") if isinstance(prior, dict) else None
        )
        # last_ok_time is a historical high-water mark; checked_subset describes
        # the latest attempt and must not overclaim after a failure.
        last_ok_time = time if status == "ok" else prior_last_ok_time
        latest_checked_subset = checked_subset if status == "ok" else None
        next_result = {
            "time": time,
            "status": status,
            "reason": reason,
            "last_ok_time": last_ok_time,
            "checked_subset": latest_checked_subset,
        }
        changed = backup.get("last_verification") != next_result
        backup["last_verification"] = next_result
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def record_restore_result(
    *,
    status: str,
    time: int | None,
    reason: str | None,
    scope: str,
    day: str | None,
    segments_selected: int,
    segments_restored: int,
    files_expected: int,
    files_restored: int,
    bytes_expected: int,
    bytes_restored: int,
) -> None:
    """Record the last on-demand media restore attempt."""
    if status not in RESTORE_STATUSES:
        raise ValueError(
            "backup restore status must be ok, no_op, refused, degraded, or error"
        )
    if scope not in {"day", "all"}:
        raise ValueError("backup restore scope must be day or all")
    for value in (
        segments_selected,
        segments_restored,
        files_expected,
        files_restored,
        bytes_expected,
        bytes_restored,
    ):
        if type(value) is not int or value < 0:
            raise ValueError("backup restore counters must be non-negative integers")

    def apply(config: dict[str, Any]) -> JournalConfigMutation[None]:
        backup = _writable_backup_section(config)
        next_result = {
            "time": time,
            "status": status,
            "reason": reason,
            "scope": scope,
            "day": day,
            "segments_selected": segments_selected,
            "segments_restored": segments_restored,
            "files_expected": files_expected,
            "files_restored": files_restored,
            "bytes_expected": bytes_expected,
            "bytes_restored": bytes_restored,
        }
        changed = backup.get("last_restore") != next_result
        backup["last_restore"] = next_result
        return JournalConfigMutation(changed=changed, value=None)

    mutate_journal_config(apply)


def status_view() -> dict[str, Any]:
    config = get_backup_config()
    destination = config["destination"]
    credentials = destination.get("credentials")
    binding = load_hosted_binding()
    hosted = {"bound": False}
    if binding is not None:
        hosted = {
            "bound": True,
            "bucket": binding.bucket,
            "prefix": binding.prefix,
        }
    return {
        "enabled": config["enabled"],
        "mode": config["mode"],
        "destination": {
            "repository": destination.get("repository"),
            "backend": destination.get("backend"),
            "credentials_set": bool(credentials),
        },
        "daily_key_set": config["daily_key"] is not None,
        "recovery_key_set": config["recovery_key"] is not None,
        "recovery_key_confirmed": bool(config["confirmed_recovery_key"]),
        "retention": config["retention"],
        "offload": config["offload"],
        "schedule": config["schedule"],
        "last_backup": config["last_backup"],
        "last_prune": config["last_prune"],
        "last_offload": config["last_offload"],
        "last_verification": config["last_verification"],
        "last_restore": config["last_restore"],
        "hosted": hosted,
    }


__all__ = [
    "BACKUP_DEFAULTS",
    "BackupKeys",
    "OFFLOAD_KEYS",
    "OFFLOAD_STATUSES",
    "RESTORE_STATUSES",
    "VERIFICATION_STATUSES",
    "clear_backup_config",
    "generate_and_store_keys",
    "get_backup_config",
    "get_destination",
    "get_keys",
    "merge_backup_config",
    "record_backup_result",
    "record_offload_result",
    "record_prune_result",
    "record_restore_result",
    "record_verification_result",
    "set_destination",
    "set_enabled",
    "set_mode",
    "set_offload",
    "set_recovery_key",
    "set_recovery_key_confirmed",
    "set_retention",
    "status_view",
]
