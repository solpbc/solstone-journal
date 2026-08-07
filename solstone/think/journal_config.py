# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared journal configuration file helpers."""

from __future__ import annotations

import copy
import json
import logging
import random
import subprocess
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Generic, TypeVar

from solstone.think import core_handshake
from solstone.think.journal_io import LockTimeout
from solstone.think.utils import (
    CorruptConfigError,
    _load_default_config,
    _resolve_os_identity,
    _resolve_os_timezone,
    get_config,
    get_journal,
)

T = TypeVar("T")
logger = logging.getLogger(__name__)

_CONFIG_CONFLICT_RETRY_BUDGET_SECONDS = 0.4
_CONFIG_CONFLICT_BACKOFF_INITIAL_SECONDS = 0.005
_CONFIG_CONFLICT_BACKOFF_MAX_SECONDS = 0.04


@dataclass(frozen=True)
class JournalConfigMutation(Generic[T]):
    """Explicit outcome from a journal config mutator."""

    changed: bool
    value: T


@dataclass(frozen=True)
class JournalConfigTransaction(Generic[T]):
    """Result of a completed journal config transaction."""

    value: T
    changed: bool
    written: bool


class JournalConfigPostCommitError(RuntimeError):
    """Raised when config committed but a required secondary effect failed."""

    def __init__(
        self,
        message: str,
        *,
        result: JournalConfigTransaction[Any],
        error: Exception,
    ):
        super().__init__(message)
        self.result = result
        self.error = error


def get_journal_config_path(journal_path: str | Path | None = None) -> Path:
    """Return the canonical journal config path."""

    return Path(journal_path or get_journal()) / "config" / "journal.json"


def read_journal_config(journal_path: str | Path | None = None) -> dict[str, Any]:
    """Read journal config through the canonical config resolver."""

    if journal_path is None:
        return get_config()

    config_path = get_journal_config_path(journal_path)
    if not config_path.exists():
        return copy.deepcopy(_load_default_config())

    try:
        with config_path.open("r", encoding="utf-8") as handle:
            return json.load(handle)
    except (json.JSONDecodeError, OSError) as exc:
        raise CorruptConfigError(config_path, error=exc) from exc


def _require_journal_config_core() -> Path:
    handshake = core_handshake.check_solstone_core_handshake()
    if handshake.status != "ok":
        raise RuntimeError(
            "journal config requires a usable solstone-core helper: "
            f"{handshake.message or 'unknown handshake failure'}"
        )
    return core_handshake.helper_path_for_executable()


def _native_failure(operation: str, completed: subprocess.CompletedProcess[str]) -> str:
    detail = completed.stderr.strip()
    message = f"journal-config {operation} failed with exit {completed.returncode}"
    return f"{message}: {detail}" if detail else message


def _run_journal_config_read(
    helper: Path,
    journal_root: Path,
    config_path: Path,
) -> tuple[bool, str | None, dict[str, Any] | None]:
    try:
        completed = subprocess.run(
            [str(helper), "journal-config", "read", "--journal", str(journal_root)],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise OSError(f"journal-config read failed to launch: {exc}") from exc

    if completed.returncode == 69:
        raise CorruptConfigError(config_path)
    if completed.returncode in {73, 74}:
        raise OSError(_native_failure("read", completed))
    if completed.returncode != 0:
        raise RuntimeError(_native_failure("read", completed))

    try:
        envelope = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError("journal-config read returned malformed JSON") from exc
    if not isinstance(envelope, dict) or not isinstance(envelope.get("present"), bool):
        raise RuntimeError("journal-config read returned an invalid response envelope")

    present = envelope["present"]
    fingerprint = envelope.get("sha256")
    config = envelope.get("config")
    if not present:
        if fingerprint is not None or config is not None:
            raise RuntimeError("journal-config read returned an invalid absent response")
        return False, None, None
    if (
        not isinstance(fingerprint, str)
        or not fingerprint.startswith("sha256:")
        or not isinstance(config, dict)
    ):
        raise RuntimeError("journal-config read returned an invalid present response")
    return True, fingerprint, config


def _run_journal_config_commit(
    helper: Path,
    journal_root: Path,
    config_path: Path,
    config: dict[str, Any],
    expect: str,
    lock_timeout_ms: int,
) -> bool:
    try:
        completed = subprocess.run(
            [
                str(helper),
                "journal-config",
                "commit",
                "--journal",
                str(journal_root),
                "--lock-timeout-ms",
                str(lock_timeout_ms),
                "--expect",
                expect,
            ],
            input=json.dumps(config, ensure_ascii=False),
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as exc:
        raise OSError(f"journal-config commit failed to launch: {exc}") from exc

    if completed.returncode == 0:
        return False
    if completed.returncode == 65:
        return True
    if completed.returncode == 69:
        raise CorruptConfigError(config_path)
    if completed.returncode in {73, 74}:
        raise OSError(_native_failure("commit", completed))
    if completed.returncode == 75:
        raise LockTimeout(config_path, lock_timeout_ms / 1000)
    raise RuntimeError(_native_failure("commit", completed))


def _remaining_lock_timeout_ms(deadline: float, previous: int | None) -> int:
    remaining = int((deadline - time.monotonic()) * 1000)
    if previous is not None:
        remaining = min(remaining, previous - 1)
    return remaining


def mutate_journal_config(
    mutator: Callable[[dict[str, Any]], JournalConfigMutation[T]],
    *,
    journal_path: str | Path | None = None,
) -> JournalConfigTransaction[T]:
    """Mutate journal config through native compare-and-swap transactions.

    The mutator may run more than once when a concurrent commit conflicts.
    Logging or other side effects inside the mutator can therefore be duplicated.
    Validation computed from state captured outside the mutator can be committed on
    a retry against configuration that changed after that pre-validation.
    """

    helper = _require_journal_config_core()
    journal_root = Path(journal_path) if journal_path is not None else get_journal()
    config_path = get_journal_config_path(journal_path)
    deadline = time.monotonic() + _CONFIG_CONFLICT_RETRY_BUDGET_SECONDS
    previous_timeout_ms: int | None = None
    attempt = 0

    while True:
        if attempt and time.monotonic() >= deadline:
            raise LockTimeout(config_path, _CONFIG_CONFLICT_RETRY_BUDGET_SECONDS)
        present, fingerprint, existing = _run_journal_config_read(
            helper,
            journal_root,
            config_path,
        )
        config = existing if present else copy.deepcopy(_load_default_config())
        if not present:
            try:
                full_name, login_name = _resolve_os_identity()
            except Exception:
                logger.debug("Failed to resolve OS identity", exc_info=True)
                full_name = ""
                login_name = ""
            try:
                timezone = _resolve_os_timezone()
            except Exception:
                logger.debug("Failed to resolve OS timezone", exc_info=True)
                timezone = ""
            config.setdefault("identity", {})
            config["identity"]["name"] = full_name
            config["identity"]["preferred"] = login_name
            config["identity"]["timezone"] = timezone
        assert config is not None
        mutation = mutator(config)
        written = not present or mutation.changed
        if not written:
            return JournalConfigTransaction(
                value=mutation.value,
                changed=mutation.changed,
                written=False,
            )

        lock_timeout_ms = _remaining_lock_timeout_ms(deadline, previous_timeout_ms)
        if lock_timeout_ms < 1:
            raise LockTimeout(config_path, _CONFIG_CONFLICT_RETRY_BUDGET_SECONDS)
        conflict = _run_journal_config_commit(
            helper,
            journal_root,
            config_path,
            config,
            "absent" if not present else fingerprint,
            lock_timeout_ms,
        )
        if not conflict:
            return JournalConfigTransaction(
                value=mutation.value,
                changed=mutation.changed,
                written=True,
            )

        previous_timeout_ms = lock_timeout_ms
        attempt += 1
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise LockTimeout(config_path, _CONFIG_CONFLICT_RETRY_BUDGET_SECONDS)
        backoff_cap = min(
            _CONFIG_CONFLICT_BACKOFF_MAX_SECONDS,
            _CONFIG_CONFLICT_BACKOFF_INITIAL_SECONDS * (2**(attempt - 1)),
        )
        time.sleep(min(random.uniform(0, backoff_cap), remaining))


def ensure_journal_config(
    journal_path: str | Path | None = None,
) -> dict[str, Any]:
    """Materialize config/journal.json and return its contents."""

    result = mutate_journal_config(
        lambda config: JournalConfigMutation(
            changed=False,
            value=copy.deepcopy(config),
        ),
        journal_path=journal_path,
    )
    return result.value


__all__ = [
    "JournalConfigMutation",
    "JournalConfigPostCommitError",
    "JournalConfigTransaction",
    "ensure_journal_config",
    "get_journal_config_path",
    "mutate_journal_config",
    "read_journal_config",
]
