# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Clock-aligned task scheduler for the supervisor.

Reads schedule definitions from config/schedules.json and submits tasks
via Callosum at hour and day boundaries. State (last-run times) persists
to health/scheduler.json across restarts.

Runtime functions (init, check) are used by the supervisor.
The main() function provides the ``journal schedule`` CLI.
"""

from __future__ import annotations

import argparse
import json
import logging
import tempfile
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

from solstone.think.utils import (
    get_journal,
    now_ms,
    parse_duration_seconds,
    require_solstone,
    setup_cli,
)

logger = logging.getLogger(__name__)

# Valid schedule intervals
INTERVALS = {"hourly", "daily", "weekly"}

# ---------------------------------------------------------------------------
# Module state (populated by init(), used by check())
# ---------------------------------------------------------------------------
_entries: dict[str, dict[str, Any]] = {}
_state: dict[str, dict[str, Any]] = {}
_callosum: Any = None  # CallosumConnection
_last_hour: datetime | None = None
_daily_time: str | None = None
_last_daily_mark: datetime | None = None
_weekly_day: str | None = None
_weekly_time: str | None = None
_last_weekly_mark: datetime | None = None


# ---------------------------------------------------------------------------
# Config + state I/O
# ---------------------------------------------------------------------------


def load_config() -> dict[str, dict[str, Any]]:
    """Read config/schedules.json and return validated entries."""
    global _daily_time, _weekly_day, _weekly_time

    config_path = Path(get_journal()) / "config" / "schedules.json"
    if not config_path.exists():
        _daily_time = None
        _weekly_day = None
        _weekly_time = None
        return {}

    try:
        with open(config_path, "r", encoding="utf-8") as f:
            raw = json.load(f)
    except (json.JSONDecodeError, OSError) as exc:
        logger.warning("Failed to load schedules config: %s", exc)
        _daily_time = None
        _weekly_day = None
        _weekly_time = None
        return {}

    if not isinstance(raw, dict):
        logger.warning(
            "schedules.json must be a JSON object, got %s", type(raw).__name__
        )
        _daily_time = None
        _weekly_day = None
        _weekly_time = None
        return {}

    # Extract daily_time metadata (not a schedule entry)
    _daily_time = raw.pop("daily_time", None)
    if _daily_time is not None and not isinstance(_daily_time, str):
        logger.warning("schedules.json: daily_time must be a string, ignoring")
        _daily_time = None

    # Extract weekly_day metadata
    _weekly_day = raw.pop("weekly_day", None)
    if _weekly_day is not None and not isinstance(_weekly_day, str):
        logger.warning("schedules.json: weekly_day must be a string, ignoring")
        _weekly_day = None
    elif _weekly_day is not None and _parse_weekly_day(_weekly_day) is None:
        logger.warning(
            "schedules.json: unrecognized weekly_day '%s', ignoring", _weekly_day
        )
        _weekly_day = None

    # Extract weekly_time metadata
    _weekly_time = raw.pop("weekly_time", None)
    if _weekly_time is not None and not isinstance(_weekly_time, str):
        logger.warning("schedules.json: weekly_time must be a string, ignoring")
        _weekly_time = None

    entries: dict[str, dict[str, Any]] = {}
    for name, entry in raw.items():
        if not isinstance(entry, dict):
            logger.warning("Schedule '%s': expected object, skipping", name)
            continue

        cmd = entry.get("cmd")
        if not cmd or not isinstance(cmd, list):
            logger.warning("Schedule '%s': missing or invalid 'cmd', skipping", name)
            continue

        every = entry.get("every")
        if every not in INTERVALS:
            logger.warning(
                "Schedule '%s': unknown interval '%s' (expected %s), skipping",
                name,
                every,
                "/".join(sorted(INTERVALS)),
            )
            continue

        if not entry.get("enabled", True):
            continue

        validated = {"cmd": cmd, "every": every}
        max_runtime = entry.get("max_runtime")
        if max_runtime is not None:
            # D-C / design §2: preserve caps for TaskQueue registration,
            # not as extra supervisor.request payload fields.
            try:
                validated["max_runtime"] = parse_duration_seconds(max_runtime)
            except ValueError:
                logger.warning(
                    "Schedule '%s': invalid max_runtime %r, dropping cap",
                    name,
                    max_runtime,
                )

        entries[name] = validated

    return entries


def load_state() -> dict[str, dict[str, Any]]:
    """Read health/scheduler.json."""
    state_path = Path(get_journal()) / "health" / "scheduler.json"
    if not state_path.exists():
        return {}

    try:
        with open(state_path, "r", encoding="utf-8") as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError) as exc:
        logger.warning("Failed to load scheduler state: %s", exc)
        return {}


def save_state() -> None:
    """Persist _state to health/scheduler.json atomically."""
    health_dir = Path(get_journal()) / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    state_path = health_dir / "scheduler.json"

    fd, tmp_path = tempfile.mkstemp(dir=health_dir, suffix=".tmp", prefix=".scheduler_")
    tmp_file = Path(tmp_path)
    try:
        with open(fd, "w", encoding="utf-8") as f:
            json.dump(_state, f, indent=2)
        tmp_file.replace(state_path)
    except BaseException:
        tmp_file.unlink(missing_ok=True)
        raise


# ---------------------------------------------------------------------------
# Boundary helpers
# ---------------------------------------------------------------------------


def _hour_mark(dt: datetime) -> datetime:
    """Truncate datetime to the start of its hour."""
    return dt.replace(minute=0, second=0, microsecond=0)


def _parse_daily_time(raw: str | None) -> tuple[int, int] | None:
    """Parse HH:MM daily time string. Returns (hour, minute) or None."""
    if not raw or not isinstance(raw, str):
        return None
    parts = raw.split(":")
    if len(parts) != 2:
        return None
    try:
        h, m = int(parts[0]), int(parts[1])
        if 0 <= h <= 23 and 0 <= m <= 59:
            return (h, m)
    except ValueError:
        return None
    return None


DAY_NAMES: dict[str, int] = {
    "monday": 0,
    "mon": 0,
    "tuesday": 1,
    "tue": 1,
    "wednesday": 2,
    "wed": 2,
    "thursday": 3,
    "thu": 3,
    "friday": 4,
    "fri": 4,
    "saturday": 5,
    "sat": 5,
    "sunday": 6,
    "sun": 6,
}


def _parse_weekly_day(raw: str | None) -> int | None:
    """Parse day-of-week name. Returns weekday int (0=Monday, 6=Sunday) or None."""
    if not raw or not isinstance(raw, str):
        return None
    return DAY_NAMES.get(raw.strip().lower())


def _compute_daily_mark(now: datetime, daily_time_str: str | None) -> datetime:
    """Compute the most recent daily boundary datetime.

    With a configured daily_time (e.g. "03:00"), the boundary is that time
    today if already passed, otherwise that time yesterday. Without a
    configured time, falls back to midnight (start of today).
    """
    parsed = _parse_daily_time(daily_time_str)
    if parsed is None:
        return now.replace(hour=0, minute=0, second=0, microsecond=0)
    h, m = parsed
    today_mark = now.replace(hour=h, minute=m, second=0, microsecond=0)
    if now >= today_mark:
        return today_mark
    return today_mark - timedelta(days=1)


def _compute_weekly_mark(
    now: datetime, weekly_day: int, weekly_time_str: str | None
) -> datetime:
    """Compute the most recent weekly boundary datetime.

    Returns the most recent occurrence of the target weekday at the target time.
    If now is past this week's boundary, returns this week's. Otherwise last week's.
    """
    parsed = _parse_daily_time(weekly_time_str)
    if parsed is None:
        h, m = 3, 0  # default 03:00
    else:
        h, m = parsed
    days_since = (now.weekday() - weekly_day) % 7
    target_date = now - timedelta(days=days_since)
    target_mark = target_date.replace(hour=h, minute=m, second=0, microsecond=0)
    if now >= target_mark:
        return target_mark
    return target_mark - timedelta(weeks=1)


def _is_due(entry: dict, state_entry: dict | None, now: datetime) -> bool:
    """Check if an entry is due based on its interval and last_run."""
    last_run = (state_entry or {}).get("last_run")
    if last_run is None:
        return True

    try:
        last_dt = datetime.fromtimestamp(last_run)
    except (OSError, ValueError):
        return True

    every = entry["every"]
    if every == "hourly":
        return last_dt < _hour_mark(now)
    if every == "daily":
        return last_dt < _compute_daily_mark(now, _daily_time)
    if every == "weekly":
        weekly_day_val = _parse_weekly_day(_weekly_day)
        if weekly_day_val is None:
            weekly_day_val = 6  # default Sunday
        return last_dt < _compute_weekly_mark(now, weekly_day_val, _weekly_time)
    return False


# ---------------------------------------------------------------------------
# Runtime API (called by supervisor)
# ---------------------------------------------------------------------------


def init(callosum: Any) -> None:
    """Initialize scheduler with a Callosum connection. Load config and state."""
    global _entries, _state, _callosum, _last_hour, _last_daily_mark, _last_weekly_mark

    _callosum = callosum
    _entries = load_config()
    _state = load_state()

    now = datetime.now()
    _last_hour = _hour_mark(now)
    _last_daily_mark = _compute_daily_mark(now, _daily_time)
    weekly_day_val = _parse_weekly_day(_weekly_day)
    if weekly_day_val is None:
        weekly_day_val = 6
    _last_weekly_mark = _compute_weekly_mark(now, weekly_day_val, _weekly_time)

    if _entries:
        logger.info(
            "Scheduler initialized with %d schedule(s): %s",
            len(_entries),
            ", ".join(sorted(_entries)),
        )
    else:
        logger.info("Scheduler initialized (no schedules configured)")


def collect_runtime_caps() -> list[tuple[list[str], int]]:
    """Return configured task runtime caps from loaded schedule entries."""
    caps: list[tuple[list[str], int]] = []
    for entry in _entries.values():
        max_runtime = entry.get("max_runtime")
        if max_runtime is not None:
            caps.append((list(entry["cmd"]), max_runtime))
    return caps


def register_defaults() -> None:
    """Ensure built-in default schedules exist in the config file.

    Called by the supervisor after init(). Writes missing defaults to
    config/schedules.json and reloads entries.
    """
    global _entries

    need_heartbeat = "heartbeat" not in _entries
    need_weekly = "weekly-agents" not in _entries
    need_providers = "providers" not in _entries

    if not need_heartbeat and not need_weekly and not need_providers:
        return

    # Read raw config (preserving daily_time and other entries)
    config_dir = Path(get_journal()) / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    config_path = config_dir / "schedules.json"

    raw: dict[str, Any] = {}
    if config_path.exists():
        try:
            with open(config_path, "r", encoding="utf-8") as f:
                raw = json.load(f)
        except (json.JSONDecodeError, OSError):
            pass

    if not isinstance(raw, dict):
        raw = {}

    changed = False

    if need_heartbeat and "heartbeat" not in raw:
        raw["heartbeat"] = {
            "cmd": ["sol", "heartbeat"],
            "every": "daily",
            "enabled": True,
            "max_runtime": "10m",
        }
        changed = True

    if need_weekly and "weekly-agents" not in raw:
        raw["weekly-agents"] = {
            "cmd": ["sol", "think", "--weekly", "-v"],
            "every": "weekly",
            "enabled": True,
            "max_runtime": "30m",
        }
        changed = True

    if need_providers and "providers" not in raw:
        raw["providers"] = {
            "cmd": ["sol", "providers", "check"],
            "every": "daily",
            "enabled": True,
            "max_runtime": "5m",
        }
        changed = True

    if not changed:
        return

    # Atomic write
    fd, tmp_path = tempfile.mkstemp(dir=config_dir, suffix=".tmp", prefix=".schedules_")
    tmp_file = Path(tmp_path)
    try:
        with open(fd, "w", encoding="utf-8") as f:
            json.dump(raw, f, indent=2)
        tmp_file.replace(config_path)
        logger.info("Auto-registered default schedule(s) in config/schedules.json")
    except BaseException:
        tmp_file.unlink(missing_ok=True)
        raise

    # Reload to pick up the new entry
    _entries = load_config()


def check() -> None:
    """Check for clock boundaries and submit due tasks.

    Called each supervisor tick (~1s). Does nothing unless an hour or day
    boundary has been crossed since the last check.
    """
    global _entries, _state, _last_hour, _last_daily_mark, _last_weekly_mark

    if _last_hour is None:
        return

    now = datetime.now()
    current_hour = _hour_mark(now)
    current_daily_mark = _compute_daily_mark(now, _daily_time)
    weekly_day_val = _parse_weekly_day(_weekly_day)
    if weekly_day_val is None:
        weekly_day_val = 6
    current_weekly_mark = _compute_weekly_mark(now, weekly_day_val, _weekly_time)

    hour_changed = current_hour != _last_hour
    daily_mark_changed = current_daily_mark != _last_daily_mark
    weekly_mark_changed = current_weekly_mark != _last_weekly_mark

    if not hour_changed and not daily_mark_changed and not weekly_mark_changed:
        return

    # Boundary crossed — reload config for freshest definitions
    _entries = load_config()
    _state = load_state()
    _last_hour = current_hour
    # Recompute with potentially updated _daily_time from config reload
    new_daily_mark = _compute_daily_mark(now, _daily_time)
    if new_daily_mark != _last_daily_mark:
        daily_mark_changed = True
    _last_daily_mark = new_daily_mark
    new_weekly_day_val = _parse_weekly_day(_weekly_day)
    if new_weekly_day_val is None:
        new_weekly_day_val = 6
    new_weekly_mark = _compute_weekly_mark(now, new_weekly_day_val, _weekly_time)
    if new_weekly_mark != _last_weekly_mark:
        weekly_mark_changed = True
    _last_weekly_mark = new_weekly_mark

    if not _entries:
        return

    submitted = False
    for name, entry in _entries.items():
        every = entry["every"]

        # Only check entries matching the boundary that changed
        if every == "hourly" and not hour_changed:
            continue
        if every == "daily" and not daily_mark_changed:
            continue
        if every == "weekly" and not weekly_mark_changed:
            continue

        if not _is_due(entry, _state.get(name), now):
            continue

        ref = f"sched:{name}:{now_ms()}"
        cmd = entry["cmd"]

        if _callosum:
            ok = _callosum.emit(
                "supervisor",
                "request",
                cmd=cmd,
                ref=ref,
                scheduler_name=name,
            )
            if ok:
                logger.info(
                    "Scheduled task submitted: %s → %s (ref=%s)",
                    name,
                    " ".join(cmd),
                    ref,
                )
                submitted = True
            else:
                logger.warning(
                    "Failed to emit scheduled task %s (callosum not connected)", name
                )
        else:
            logger.warning("No callosum connection for scheduled task: %s", name)

    if submitted:
        logger.debug("Submitted scheduled task batch")


def collect_status() -> list[dict[str, Any]]:
    """Return schedule status for supervisor.status events."""
    now = datetime.now()
    result = []
    for name, entry in _entries.items():
        state_entry = _state.get(name)
        last_run = (state_entry or {}).get("last_run")
        entry_status = {
            "name": name,
            "every": entry["every"],
            "last_run": last_run,
            "due": _is_due(entry, state_entry, now),
        }
        entry_status["next_run"] = _compute_next_run(entry, state_entry, now)
        if entry["every"] == "daily" and _daily_time:
            entry_status["daily_time"] = _daily_time
        if entry["every"] == "weekly":
            if _weekly_day:
                entry_status["weekly_day"] = _weekly_day
            if _weekly_time:
                entry_status["weekly_time"] = _weekly_time
        result.append(entry_status)
    return result


def _compute_next_run(entry: dict, state_entry: dict | None, now: datetime) -> int:
    """Compute next run time as epoch milliseconds."""
    every = entry["every"]
    if every == "hourly":
        mark = _hour_mark(now)
        nxt = mark if _is_due(entry, state_entry, now) else mark + timedelta(hours=1)
    elif every == "daily":
        mark = _compute_daily_mark(now, _daily_time)
        nxt = mark if _is_due(entry, state_entry, now) else mark + timedelta(days=1)
    elif every == "weekly":
        weekly_day_val = _parse_weekly_day(_weekly_day)
        if weekly_day_val is None:
            weekly_day_val = 6
        mark = _compute_weekly_mark(now, weekly_day_val, _weekly_time)
        nxt = mark if _is_due(entry, state_entry, now) else mark + timedelta(weeks=1)
    else:
        return int(now.timestamp() * 1000)
    return int(nxt.timestamp() * 1000)


# ---------------------------------------------------------------------------
# CLI: journal schedule
# ---------------------------------------------------------------------------


def _format_timestamp(epoch: float | None) -> str:
    """Format an epoch timestamp for display."""
    if epoch is None:
        return "never"
    try:
        return datetime.fromtimestamp(epoch).strftime("%Y-%m-%d %H:%M")
    except (OSError, ValueError):
        return "invalid"


def _format_next_due(entry: dict, state_entry: dict | None, now: datetime) -> str:
    """Format the next due time for display."""
    if _is_due(entry, state_entry, now):
        return "now"

    every = entry["every"]
    if every == "hourly":
        nxt = _hour_mark(now) + timedelta(hours=1)
        return nxt.strftime("%H:%M")
    if every == "daily":
        parsed = _parse_daily_time(_daily_time)
        return f"{parsed[0]:02d}:{parsed[1]:02d}" if parsed else "midnight"
    if every == "weekly":
        weekly_day_val = _parse_weekly_day(_weekly_day)
        if weekly_day_val is None:
            weekly_day_val = 6
        weekly_mark = _compute_weekly_mark(now, weekly_day_val, _weekly_time)
        nxt = weekly_mark + timedelta(weeks=1)
        return f"{nxt.strftime('%A')} {nxt.strftime('%H:%M')}"
    return "?"


def main() -> None:
    """CLI entry point for journal schedule."""
    parser = argparse.ArgumentParser(description="Show scheduled tasks")
    setup_cli(parser)
    require_solstone()

    journal = Path(get_journal())
    config_path = journal / "config" / "schedules.json"
    state_path = journal / "health" / "scheduler.json"

    # Load config (all entries, including disabled for display)
    config: dict[str, Any] = {}
    if config_path.exists():
        try:
            with open(config_path, "r", encoding="utf-8") as f:
                config = json.load(f)
        except (json.JSONDecodeError, OSError) as exc:
            print(f"Error reading {config_path}: {exc}")
            return

    # Extract daily_time metadata before processing entries
    global _daily_time, _weekly_day, _weekly_time
    raw_daily_time = config.pop("daily_time", None)
    _daily_time = raw_daily_time if isinstance(raw_daily_time, str) else None
    raw_weekly_day = config.pop("weekly_day", None)
    raw_weekly_time = config.pop("weekly_time", None)
    _weekly_day = raw_weekly_day if isinstance(raw_weekly_day, str) else None
    _weekly_time = raw_weekly_time if isinstance(raw_weekly_time, str) else None

    if not config:
        print("No schedules configured.")
        print(f"\nAdd schedules to: {config_path}")
        return

    # Load state
    state: dict[str, Any] = {}
    if state_path.exists():
        try:
            with open(state_path, "r", encoding="utf-8") as f:
                state = json.load(f)
        except (json.JSONDecodeError, OSError):
            pass

    now = datetime.now()

    # Compute column widths
    names = list(config.keys())
    name_width = max(max(len(n) for n in names), 4)
    every_width = 8
    last_run_width = 18
    next_due_width = 10

    # Header
    header = (
        f"  {'NAME':<{name_width}}  {'EVERY':<{every_width}}  "
        f"{'LAST RUN':<{last_run_width}}  {'NEXT DUE':<{next_due_width}}  CMD"
    )
    print(header)
    print()

    for name, raw_entry in sorted(config.items()):
        if not isinstance(raw_entry, dict):
            continue

        every = raw_entry.get("every", "?")
        cmd = raw_entry.get("cmd", [])
        enabled = raw_entry.get("enabled", True)
        state_entry = state.get(name)

        last_run_str = _format_timestamp((state_entry or {}).get("last_run"))

        # Build a validated entry for _is_due / _format_next_due
        if every in INTERVALS and enabled:
            entry = {"cmd": cmd, "every": every}
            next_due_str = _format_next_due(entry, state_entry, now)
        else:
            next_due_str = "disabled" if not enabled else "?"

        cmd_str = " ".join(cmd) if isinstance(cmd, list) else str(cmd)

        tags = ""
        if not enabled:
            tags = " [disabled]"

        line = (
            f"  {name:<{name_width}}  {every:<{every_width}}  "
            f"{last_run_str:<{last_run_width}}  {next_due_str:<{next_due_width}}  {cmd_str}{tags}"
        )
        print(line.rstrip())

    print()
    print(f"Config: {config_path}")
