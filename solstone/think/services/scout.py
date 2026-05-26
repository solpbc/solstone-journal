# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Scout service journal-config storage."""

from __future__ import annotations

import fcntl
import hashlib
import logging
import os
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from solstone.think.journal_config import (
    get_journal_config_path,
    read_journal_config,
    write_journal_config,
)
from solstone.think.utils import get_journal

log = logging.getLogger(__name__)

_HANDOFF_FIELDS = ("google_api_key", "dispatch_token", "account_id", "created_at")
KEY_FINGERPRINT_FIELD = "key_fingerprint_sha256"


class JournalNotInitializedError(RuntimeError):
    """Raised when the journal config file has not been initialized."""


@dataclass(frozen=True)
class DisableOutcome:
    was_enabled: bool
    env_key_preserved: bool


def _lock_path() -> Path:
    return Path(get_journal()) / "config" / ".journal.json.lock"


def _require_journal_config() -> None:
    if not get_journal_config_path().exists():
        raise JournalNotInitializedError(
            "journal config file is not present; run 'journal setup' first"
        )


def _validate_handoff_payload(payload: dict[str, Any]) -> dict[str, str]:
    validated: dict[str, str] = {}
    for field in _HANDOFF_FIELDS:
        if field not in payload:
            raise ValueError(f"malformed handoff payload: missing field '{field}'")
        value = payload[field]
        if not isinstance(value, str) or not value:
            raise ValueError(
                f"malformed handoff payload: field '{field}' must be a non-empty string"
            )
        validated[field] = value
    return validated


def _fingerprint_key(key: str) -> str:
    return hashlib.sha256(key.encode("utf-8")).hexdigest()


def provision_scout_handoff(payload: dict[str, Any]) -> None:
    """Persist a portal-provisioned scout handoff into journal config."""

    values = _validate_handoff_payload(payload)
    _require_journal_config()

    lock_path = _lock_path()
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with open(lock_path, "w", encoding="utf-8") as lock_file:
        os.chmod(lock_path, 0o600)
        fcntl.flock(lock_file, fcntl.LOCK_EX)
        try:
            _require_journal_config()
            config = read_journal_config()
            config.setdefault("env", {})["GOOGLE_API_KEY"] = values["google_api_key"]
            config.setdefault("services", {})["scout"] = {
                "enabled_at": datetime.now(timezone.utc).isoformat(),
                "account_id": values["account_id"],
                "key_created_at": values["created_at"],
                "dispatch_token": values["dispatch_token"],
                KEY_FINGERPRINT_FIELD: _fingerprint_key(values["google_api_key"]),
            }
            write_journal_config(config)
            log.debug(
                "provisioned scout service for account_id=%s", values["account_id"]
            )
        finally:
            fcntl.flock(lock_file, fcntl.LOCK_UN)


def disable_scout() -> DisableOutcome:
    """Disable scout provisioning while preserving unrelated manual keys."""

    _require_journal_config()

    lock_path = _lock_path()
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with open(lock_path, "w", encoding="utf-8") as lock_file:
        os.chmod(lock_path, 0o600)
        fcntl.flock(lock_file, fcntl.LOCK_EX)
        try:
            _require_journal_config()
            config = read_journal_config()
            services = config.setdefault("services", {})
            scout_block = services.get("scout")
            if not isinstance(scout_block, dict):
                return DisableOutcome(was_enabled=False, env_key_preserved=False)

            services.pop("scout", None)
            env = config.setdefault("env", {})
            current_key = env.get("GOOGLE_API_KEY")
            stored_fingerprint = scout_block.get(KEY_FINGERPRINT_FIELD)
            env_key_preserved = True
            if (
                isinstance(current_key, str)
                and isinstance(stored_fingerprint, str)
                and _fingerprint_key(current_key) == stored_fingerprint
            ):
                env.pop("GOOGLE_API_KEY", None)
                env_key_preserved = False

            write_journal_config(config)
            log.debug("disabled scout service")
            return DisableOutcome(
                was_enabled=True,
                env_key_preserved=env_key_preserved,
            )
        finally:
            fcntl.flock(lock_file, fcntl.LOCK_UN)


def is_scout_enabled() -> bool:
    """Return whether scout is enabled through service provisioning."""

    config = read_journal_config()
    return bool(
        config.get("services", {}).get("scout")
        and config.get("env", {}).get("GOOGLE_API_KEY")
    )


def is_manual_key_present() -> bool:
    """Return whether a manual Gemini key exists without scout provenance."""

    config = read_journal_config()
    return bool(
        config.get("env", {}).get("GOOGLE_API_KEY")
        and not config.get("services", {}).get("scout")
    )


def scout_provenance() -> dict[str, Any] | None:
    """Return the scout provenance block from journal config, if present."""

    provenance = read_journal_config().get("services", {}).get("scout")
    return provenance if isinstance(provenance, dict) else None
