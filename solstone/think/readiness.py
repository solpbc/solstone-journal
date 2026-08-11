# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
import os
import time
from pathlib import Path
from typing import Any

import psutil

from solstone.think.utils import get_journal

MARKER_RELATIVE_PATH = Path("health") / "supervisor.ready"
_PID_RELATIVE_PATH = Path("health") / "supervisor.pid"
_START_TIME_RELATIVE_PATH = Path("health") / "supervisor.start_time"
START_TIME_TOLERANCE_S = 1.5
_POLL_INTERVAL_S = 0.5

logger = logging.getLogger(__name__)


def _journal_path() -> Path:
    return Path(get_journal())


def _marker_path() -> Path:
    return _journal_path() / MARKER_RELATIVE_PATH


def clear_ready() -> None:
    try:
        _marker_path().unlink(missing_ok=True)
    except OSError as exc:
        logger.warning("Could not clear readiness marker: %s", exc)


def _read_json_marker(path: Path) -> dict[str, Any] | None:
    try:
        payload = json.loads(path.read_text())
    except FileNotFoundError:
        logger.debug("Readiness marker is not present yet")
        return None
    except (OSError, json.JSONDecodeError) as exc:
        logger.warning("Readiness marker is malformed or unreadable: %s", exc)
        return None

    if not isinstance(payload, dict):
        logger.warning("Readiness marker is malformed: expected JSON object")
        return None
    for key in ("pid", "ready_at", "start_time"):
        if key not in payload:
            logger.warning("Readiness marker is malformed: missing %s", key)
            return None
    return payload


def _read_supervisor_pid(path: Path) -> int | None:
    try:
        return int(path.read_text().strip())
    except FileNotFoundError:
        logger.debug("Supervisor pid file is not present yet")
    except (OSError, ValueError) as exc:
        logger.warning("Could not read supervisor pid for readiness check: %s", exc)
    return None


def _read_recorded_start_time(path: Path) -> float | None:
    try:
        return float(path.read_text().strip())
    except FileNotFoundError:
        logger.debug("Supervisor start time file is not present yet")
    except (OSError, ValueError) as exc:
        logger.warning(
            "Could not read supervisor start time for readiness check: %s", exc
        )
    return None


# This cut leaves no dual-path shim. is_supervisor_up(), wait_ready(), and
# clear_ready() were never part of the deleted Python supervisor writer domain;
# service.py uses them for launch-and-wait orchestration around the native
# supervisor, now the sole writer of health/supervisor.{pid,start_time,ready}.
# No future deletion of this module is planned.
def is_supervisor_up() -> bool:
    """Return True when pid and start time identify a live supervisor process."""
    journal = _journal_path()
    pid = _read_supervisor_pid(journal / _PID_RELATIVE_PATH)
    if pid is None:
        return False

    try:
        os.kill(pid, 0)
    except OSError:
        return False

    recorded_start = _read_recorded_start_time(
        journal / _START_TIME_RELATIVE_PATH
    )
    if recorded_start is None:
        return False

    try:
        create_time = psutil.Process(pid).create_time()
    except psutil.Error:
        return False

    return abs(recorded_start - create_time) <= START_TIME_TOLERANCE_S


def _valid_marker() -> dict[str, Any] | None:
    journal = _journal_path()
    payload = _read_json_marker(journal / MARKER_RELATIVE_PATH)
    if payload is None:
        return None

    try:
        marker_pid = int(payload["pid"])
        float(payload["ready_at"])
        float(payload["start_time"])
    except (TypeError, ValueError) as exc:
        logger.warning("Readiness marker is malformed: %s", exc)
        return None

    recorded_pid = _read_supervisor_pid(journal / _PID_RELATIVE_PATH)
    if recorded_pid is None:
        return None
    if marker_pid != recorded_pid:
        logger.debug(
            "Readiness marker pid %s does not match supervisor pid %s",
            marker_pid,
            recorded_pid,
        )
        return None

    recorded_start = _read_recorded_start_time(journal / _START_TIME_RELATIVE_PATH)
    if recorded_start is None:
        return None

    try:
        create_time = psutil.Process(recorded_pid).create_time()
    except (psutil.NoSuchProcess, psutil.Error) as exc:
        logger.warning("Could not validate supervisor process readiness: %s", exc)
        return None

    if abs(recorded_start - create_time) > START_TIME_TOLERANCE_S:
        logger.debug(
            "Supervisor start time mismatch for readiness marker: recorded=%s process=%s",
            recorded_start,
            create_time,
        )
        return None

    return payload


def wait_ready(timeout: float) -> dict[str, Any] | None:
    deadline = time.monotonic() + timeout
    while True:
        payload = _valid_marker()
        if payload is not None:
            return payload

        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        time.sleep(min(_POLL_INTERVAL_S, remaining))
