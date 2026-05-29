# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from datetime import datetime, timezone
from typing import Literal, TypedDict, cast, get_args

from solstone.think.journal_config import read_journal_config, write_journal_config

InstallState = Literal[
    "idle",
    "resolving",
    "downloading",
    "verifying",
    "installing",
    "installed",
    "failed",
]


class InstallStatus(TypedDict):
    name: str
    install_state: InstallState
    last_transition_at: str | None
    last_progress_at: str | None
    progress_bytes_received: int | None
    progress_bytes_total: int | None
    install_error: str | None


_InstallScope = Literal["bundled", "mlx"]

INSTALL_STATE_NO_PROGRESS_SECONDS = 60
IN_FLIGHT_STATES: frozenset[InstallState] = frozenset(
    {"resolving", "downloading", "verifying", "installing"}
)
TERMINAL_STATES: frozenset[InstallState] = frozenset({"idle", "installed", "failed"})
_INSTALL_STATES = frozenset(get_args(InstallState))


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def make_idle_status(name: str) -> InstallStatus:
    return {
        "name": name,
        "install_state": "idle",
        "last_transition_at": None,
        "last_progress_at": None,
        "progress_bytes_received": None,
        "progress_bytes_total": None,
        "install_error": None,
    }


def transition_state(
    status: InstallStatus,
    *,
    new_state: InstallState,
    error: str | None = None,
) -> InstallStatus:
    timestamp = now_iso()
    is_terminal = new_state in TERMINAL_STATES
    return {
        "name": status["name"],
        "install_state": new_state,
        "last_transition_at": timestamp,
        "last_progress_at": timestamp if new_state in IN_FLIGHT_STATES else None,
        "progress_bytes_received": (
            None if is_terminal else status["progress_bytes_received"]
        ),
        "progress_bytes_total": None if is_terminal else status["progress_bytes_total"],
        "install_error": error if new_state == "failed" else None,
    }


def bump_progress(
    status: InstallStatus,
    *,
    received: int | None = None,
    total: int | None = None,
) -> InstallStatus:
    if status["install_state"] not in IN_FLIGHT_STATES:
        raise ValueError("install progress can only be bumped for in-flight states")
    return {
        "name": status["name"],
        "install_state": status["install_state"],
        "last_transition_at": status["last_transition_at"],
        "last_progress_at": now_iso(),
        "progress_bytes_received": (
            received if received is not None else status["progress_bytes_received"]
        ),
        "progress_bytes_total": (
            total if total is not None else status["progress_bytes_total"]
        ),
        "install_error": status["install_error"],
    }


def is_stalled(status: InstallStatus, *, now: datetime | None = None) -> bool:
    if status["install_state"] not in IN_FLIGHT_STATES:
        return False
    last_progress_at = status["last_progress_at"]
    if last_progress_at is None:
        return False
    parsed = datetime.fromisoformat(last_progress_at)
    now = now or datetime.now(timezone.utc)
    return (now - parsed).total_seconds() > INSTALL_STATE_NO_PROGRESS_SECONDS


def read_install_status(*, scope: _InstallScope, name: str) -> InstallStatus:
    config = read_journal_config()
    try:
        record = config["providers"][scope][name]
        install_state = record["install_state"]
    except (KeyError, TypeError, ValueError):
        return make_idle_status(name)

    if install_state not in _INSTALL_STATES:
        return make_idle_status(name)

    return {
        "name": name,
        "install_state": cast(InstallState, install_state),
        "last_transition_at": record.get("last_transition_at"),
        "last_progress_at": record.get("last_progress_at"),
        "progress_bytes_received": record.get("progress_bytes_received"),
        "progress_bytes_total": record.get("progress_bytes_total"),
        "install_error": record.get("install_error"),
    }


def write_install_status(status: InstallStatus, *, scope: _InstallScope) -> None:
    config = read_journal_config()
    slot = (
        config.setdefault("providers", {})
        .setdefault(scope, {})
        .setdefault(status["name"], {})
    )
    slot["install_state"] = status["install_state"]
    slot["last_transition_at"] = status["last_transition_at"]
    slot["last_progress_at"] = status["last_progress_at"]
    slot["install_error"] = status["install_error"]
    slot["progress_bytes_received"] = status["progress_bytes_received"]
    slot["progress_bytes_total"] = status["progress_bytes_total"]
    write_journal_config(config)


__all__ = [
    "InstallState",
    "InstallStatus",
    "INSTALL_STATE_NO_PROGRESS_SECONDS",
    "IN_FLIGHT_STATES",
    "TERMINAL_STATES",
    "now_iso",
    "make_idle_status",
    "transition_state",
    "bump_progress",
    "is_stalled",
    "read_install_status",
    "write_install_status",
]
