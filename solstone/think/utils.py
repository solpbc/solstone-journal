# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""General utilities for solstone.

This module provides core utilities for journal access, date/segment handling,
configuration loading, and CLI setup. Talent-related utilities (prompt loading,
agent configs, etc.) have been moved to think/talent.py.
"""

from __future__ import annotations

import argparse
import copy
import json
import logging
import os
import pwd
import re
import secrets
import socket
import sys
import time
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Optional
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from timefhuman import timefhuman

from solstone.think.media import MIME_TYPES

DATE_RE = re.compile(r"\d{8}")
STREAM_RE = re.compile(r"^[a-z0-9][a-z0-9._-]*$")
CHRONICLE_DIR = "chronicle"
DEFAULT_STREAM = "_default"
EXIT_TEMPFAIL = 75


class SolstoneNotConfigured(RuntimeError):
    def __init__(
        self,
        message: str,
        *,
        path: str | None = None,
        error: OSError | None = None,
    ):
        super().__init__(message)
        self.path = path
        self.error = error


def now_ms() -> int:
    """Return current time as Unix epoch milliseconds."""
    return int(time.time() * 1000)


_rev_cache: str | None = "__unset__"


def get_rev() -> str | None:
    """Return short git commit hash, cached after first call. None if unavailable."""
    global _rev_cache
    if _rev_cache != "__unset__":
        return _rev_cache
    try:
        import subprocess

        result = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        _rev_cache = result.stdout.strip() if result.returncode == 0 else None
    except Exception:
        _rev_cache = None
    return _rev_cache


def truncated_echo(text: str, max_bytes: int = 16384) -> None:
    """Print text to stdout, truncating if it exceeds *max_bytes* UTF-8 bytes.

    When the encoded output exceeds the limit it is cut at a clean UTF-8
    character boundary and a warning is written to stderr reporting the
    original size.  Pass ``max_bytes=0`` to disable the limit.
    """
    encoded = text.encode("utf-8")
    if max_bytes > 0 and len(encoded) > max_bytes:
        truncated = encoded[:max_bytes].decode("utf-8", errors="ignore")
        sys.stdout.write(truncated)
        sys.stdout.write("\n")
        sys.stderr.write(
            f"[truncated: {len(encoded):,} bytes total, --max {max_bytes:,}]\n"
        )
    else:
        sys.stdout.write(text)
        sys.stdout.write("\n")


def get_project_root() -> str:
    """Return the absolute path to the solstone repository root."""
    return str(Path(__file__).resolve().parent.parent.parent)


def is_source_checkout() -> bool:
    """Return True when solstone is running from a source checkout."""
    project_root = Path(get_project_root())
    return (project_root / "pyproject.toml").exists() and (
        project_root / ".git"
    ).exists()


def is_packaged_install() -> bool:
    """Return True when solstone is running from an installed package."""
    return not is_source_checkout()


def get_journal_info() -> tuple[str, str]:
    """Resolve the journal path and its source.

    Returns ``(path, source)`` where source is one of
    ``{"env", "config", "source", "default"}``:

    - ``"env"`` — ``SOLSTONE_JOURNAL`` is set
    - ``"config"`` — ``~/.config/solstone/config.toml`` has a non-empty
      ``journal`` key
    - ``"source"`` — running from a source checkout; journal is
      ``<project_root>/journal``
    - ``"default"`` — built-in default at ``~/journal``

    The wrapper at ``~/.local/bin/sol`` is responsible for setting
    ``SOLSTONE_JOURNAL`` on installed runs; tests set it via the autouse
    fixture.
    """
    env_path = os.environ.get("SOLSTONE_JOURNAL")
    if env_path:
        return env_path, "env"

    from solstone.think.user_config import read_user_config

    user_cfg_path = read_user_config().get("journal", "").strip()
    if user_cfg_path:
        return user_cfg_path, "config"

    if is_source_checkout():
        return str(Path(get_project_root()) / "journal"), "source"

    from solstone.think.user_config import default_journal

    return default_journal(), "default"


def get_journal() -> str:
    """Return the journal path. Auto-creates the directory.

    Resolves the journal from ``SOLSTONE_JOURNAL``, user config, the
    source-tree journal at ``<project_root>/journal``, or the built-in
    ``~/journal`` default. Raises ``SolstoneNotConfigured`` only if
    mkdir fails for the resolved path.

    Trust this function — never bypass it, cache its result, or set
    ``SOLSTONE_JOURNAL`` from application code, agent prompts, subprocess
    environments, or service files. The wrapper at ``~/.local/bin/sol`` is
    the canonical setter; tests use the autouse fixture; everywhere else,
    let it resolve on its own. See ``docs/environment.md``.
    """
    path, source = get_journal_info()
    try:
        os.makedirs(path, exist_ok=True)
    except OSError as exc:
        raise SolstoneNotConfigured(
            f"could not create journal directory ({source}): {path}: {exc}",
            path=path,
            error=exc,
        ) from exc
    return path


def parse_duration_seconds(spec) -> int:
    # D-D: shared parser for scheduler max_runtime values.
    if isinstance(spec, int) and not isinstance(spec, bool):
        if spec > 0:
            return spec
        raise ValueError(f"invalid duration: {spec!r}")

    if isinstance(spec, str):
        match = re.fullmatch(r"(\d+)([smh])", spec)
        if match:
            amount = int(match.group(1))
            if amount <= 0:
                raise ValueError(f"invalid duration: {spec!r}")
            return amount * {"s": 1, "m": 60, "h": 3600}[match.group(2)]

    raise ValueError(f"invalid duration: {spec!r}")


def resolve_journal_path(journal: str | Path, rel: str) -> Path:
    """Resolve a chronicle-free journal-relative path to its on-disk location."""
    if not rel:
        raise ValueError("rel must be non-empty")
    if os.path.isabs(rel):
        raise ValueError("rel must be journal-relative")
    if "\\" in rel:
        raise ValueError("rel must use POSIX separators")
    parts = Path(rel).parts
    if not parts or any(p in ("", ".", "..") for p in parts):
        raise ValueError("rel must not contain empty, '.', or '..' components")
    journal_path = Path(journal)
    if DATE_RE.fullmatch(parts[0]):
        return journal_path / CHRONICLE_DIR / rel
    return journal_path / rel


def journal_relative_path(journal: str | Path, abs_path: str | Path) -> str:
    """Return a chronicle-free journal-relative POSIX path for an absolute path under the journal."""
    journal_path = Path(journal)
    file_path = Path(abs_path)
    chronicle_root = journal_path / CHRONICLE_DIR
    if file_path.is_relative_to(chronicle_root):
        return file_path.relative_to(chronicle_root).as_posix()
    return file_path.relative_to(journal_path).as_posix()


def day_path(day: Optional[str] = None, *, create: bool = True) -> Path:
    """Return absolute path for a day directory within the journal chronicle.

    Parameters
    ----------
    day : str, optional
        Day in YYYYMMDD format. If None, uses today's date.
    create : bool, optional
        Create the day directory if it does not exist. Defaults to True.

    Returns
    -------
    Path
        Absolute path to the day directory in chronicle/. Directory is created if
        it doesn't exist.

    Raises
    ------
    ValueError
        If day format is invalid.
    """
    journal = get_journal()

    # Handle "today" case
    if day is None:
        day = datetime.now().strftime("%Y%m%d")
    elif not DATE_RE.fullmatch(day):
        raise ValueError("day must be in YYYYMMDD format")

    path = Path(journal) / CHRONICLE_DIR / day
    if create:
        path.mkdir(parents=True, exist_ok=True)
    return path


def day_dirs() -> dict[str, str]:
    """Return mapping of YYYYMMDD day names to absolute paths.

    Returns
    -------
    dict[str, str]
        Mapping of day folder names to their full paths.
        Example: {"20250101": "/path/to/journal/chronicle/20250101", ...}
    """
    chronicle_dir = Path(get_journal()) / CHRONICLE_DIR
    if not chronicle_dir.is_dir():
        return {}

    days: dict[str, str] = {}
    for name in os.listdir(chronicle_dir):
        if DATE_RE.fullmatch(name):
            path = os.path.join(chronicle_dir, name)
            if os.path.isdir(path):
                days[name] = path
    return days


def updated_days(exclude: set[str] | None = None) -> list[str]:
    """Return journal days with pending stream data not yet processed daily.

    A day is "updated" when it has a ``health/stream.updated`` marker that is
    newer than its ``health/daily.updated`` marker (or daily.updated is missing).
    Days without ``stream.updated`` are skipped entirely.

    Parameters
    ----------
    exclude : set of str, optional
        Day strings (YYYYMMDD) to skip.

    Returns
    -------
    list of str
        Sorted list of updated day strings.
    """
    days = day_dirs()
    updated: list[str] = []
    for name, path in days.items():
        if exclude and name in exclude:
            continue
        stream = os.path.join(path, "health", "stream.updated")
        if not os.path.isfile(stream):
            continue
        daily = os.path.join(path, "health", "daily.updated")
        if not os.path.isfile(daily):
            updated.append(name)
            continue
        if os.path.getmtime(stream) > os.path.getmtime(daily):
            updated.append(name)
    updated.sort()
    return updated


def segment_path(day: str, segment: str, stream: str, *, create: bool = True) -> Path:
    """Return absolute path for a segment directory within a stream.

    Parameters
    ----------
    day : str
        Day in YYYYMMDD format.
    segment : str
        Segment key in HHMMSS_LEN format.
    stream : str
        Stream name (e.g., "archon", "import.apple").
    create : bool, optional
        Create the segment directory if it does not exist. Defaults to True.

    Returns
    -------
    Path
        Absolute path to the segment directory.
    """
    path = day_path(day, create=create) / stream / segment
    if create:
        path.mkdir(parents=True, exist_ok=True)
    return path


def day_from_path(path: str | Path) -> str | None:
    """Extract the YYYYMMDD day from a journal path.

    Walks up the path's parents and returns the first directory name
    that matches the YYYYMMDD date format.

    Parameters
    ----------
    path : str or Path
        Any path within the journal directory structure.

    Returns
    -------
    str or None
        The YYYYMMDD day string, or None if no date directory is found.
    """
    path = Path(path)
    for parent in (path, *path.parents):
        if DATE_RE.fullmatch(parent.name):
            return parent.name
    return None


def iter_segments(day: str | Path) -> list[tuple[str, str, Path]]:
    """Return all segments in a day, sorted chronologically.

    Traverses the stream directory structure under a day directory and
    returns segment information for all streams.

    Parameters
    ----------
    day : str or Path
        Day in YYYYMMDD format (str) or path to day directory (Path).

    Returns
    -------
    list of (stream_name, segment_key, segment_path) tuples
        Sorted by segment_key across all streams for chronological order.
    """
    if isinstance(day, Path):
        day_dir = day
    else:
        day_dir = day_path(day, create=False)

    if not day_dir.exists():
        return []

    results = []
    for entry in day_dir.iterdir():
        if not entry.is_dir():
            continue
        if segment_key(entry.name) is not None:
            results.append((DEFAULT_STREAM, entry.name, entry))
            continue
        if entry.name == "health":
            continue
        stream_name = entry.name
        for seg_entry in entry.iterdir():
            if seg_entry.is_dir() and segment_key(seg_entry.name):
                results.append((stream_name, seg_entry.name, seg_entry))

    results.sort(key=lambda x: x[1])
    return results


def segment_key(name_or_path: str) -> str | None:
    """Extract segment key (HHMMSS_LEN) from any path/filename.

    Parameters
    ----------
    name_or_path : str
        Segment name, filename, or full path containing segment.

    Returns
    -------
    str or None
        Segment key in HHMMSS_LEN format if valid, None otherwise.

    Examples
    --------
    >>> segment_key("143022_300")
    "143022_300"
    >>> segment_key("143022_300_summary.txt")
    "143022_300"
    >>> segment_key("/journal/20250109/143022_300/audio.jsonl")
    "143022_300"
    >>> segment_key("invalid")
    None
    """
    # Match HHMMSS_LEN format: 6 digits, underscore, 1+ digits
    pattern = r"\b(\d{6})_(\d+)(?:_|\b)"
    match = re.search(pattern, name_or_path)
    if match:
        time_part = match.group(1)
        len_part = match.group(2)
        return f"{time_part}_{len_part}"
    return None


def segment_parse(
    name_or_path: str,
) -> tuple[datetime.time, datetime.time] | tuple[None, None]:
    """Parse segment to extract start and end times as datetime objects.

    Parameters
    ----------
    name_or_path : str
        Segment name (e.g., "143022_300") or full path containing segment.

    Returns
    -------
    tuple of (datetime.time, datetime.time) or (None, None)
        Tuple of (start_time, end_time) where:
        - start_time: datetime.time for HHMMSS
        - end_time: datetime.time computed from start + LEN seconds
        Returns (None, None) if not a valid HHMMSS_LEN segment format.

    Examples
    --------
    >>> segment_parse("143022_300")  # 14:30:22 + 300 seconds = 14:35:22
    (datetime.time(14, 30, 22), datetime.time(14, 35, 22))
    >>> segment_parse("/journal/20250109/143022_300/audio.jsonl")
    (datetime.time(14, 30, 22), datetime.time(14, 35, 22))
    >>> segment_parse("invalid")
    (None, None)
    """
    from datetime import time, timedelta

    # Extract just the segment name if it's a path
    if "/" in name_or_path or "\\" in name_or_path:
        path_parts = Path(name_or_path).parts
        # Look for segment key in path parts after a YYYYMMDD day directory.
        # Layout is YYYYMMDD/stream/HHMMSS_LEN/...
        name = None
        for i, part in enumerate(path_parts):
            if part.isdigit() and len(part) == 8:
                # Scan subsequent parts for a segment key
                for j in range(i + 1, len(path_parts)):
                    if segment_key(path_parts[j]):
                        name = path_parts[j]
                        break
                if name:
                    break
        if name is None:
            return (None, None)
    else:
        name = name_or_path

    # Validate and extract HHMMSS_LEN from segment name
    if "_" not in name:
        return (None, None)

    parts = name.split("_", 1)  # Split on first underscore only
    if (
        len(parts) != 2
        or not parts[0].isdigit()
        or len(parts[0]) != 6
        or not parts[1].isdigit()
    ):
        return (None, None)

    time_str = parts[0]
    length_str = parts[1]

    # Parse HHMMSS to datetime.time
    try:
        hour = int(time_str[0:2])
        minute = int(time_str[2:4])
        second = int(time_str[4:6])

        # Validate ranges
        if not (0 <= hour <= 23 and 0 <= minute <= 59 and 0 <= second <= 59):
            return (None, None)

        start_time = time(hour, minute, second)
    except (ValueError, IndexError):
        return (None, None)

    # Parse LEN and compute end time
    try:
        length_seconds = int(length_str)
        # Compute end time by adding duration
        start_dt = datetime.combine(datetime.today(), start_time)
        end_dt = start_dt + timedelta(seconds=length_seconds)
        if end_dt.date() > start_dt.date():
            end_time = time(23, 59, 59)
        else:
            end_time = end_dt.time()
        return (start_time, end_time)
    except ValueError:
        return (None, None)


def format_day(day: str) -> str:
    """Format a day string (YYYYMMDD) as a human-readable date.

    Parameters
    ----------
    day:
        Day in YYYYMMDD format.

    Returns
    -------
    str
        Formatted date like "Friday, January 24, 2026".
        Returns the original string if parsing fails.

    Examples
    --------
    >>> format_day("20260124")
    "Friday, January 24, 2026"
    """
    try:
        dt = datetime.strptime(day, "%Y%m%d")
        return dt.strftime("%A, %B %d, %Y")
    except ValueError:
        return day


def iso_date(day: str) -> str:
    """Convert a day string (YYYYMMDD) to ISO format (YYYY-MM-DD).

    Parameters
    ----------
    day:
        Day in YYYYMMDD format.

    Returns
    -------
    str
        ISO formatted date like "2026-01-24".
    """
    return f"{day[:4]}-{day[4:6]}-{day[6:8]}"


def get_owner_timezone() -> ZoneInfo:
    """Return the configured owner timezone or fall back to the host timezone."""
    configured = str(get_config().get("identity", {}).get("timezone") or "").strip()
    if configured:
        try:
            return ZoneInfo(configured)
        except ZoneInfoNotFoundError:
            logging.getLogger(__name__).warning(
                "Invalid identity.timezone %r; falling back to host timezone",
                configured,
            )

    local_tz = datetime.now().astimezone().tzinfo
    if isinstance(local_tz, ZoneInfo):
        return local_tz

    local_key = getattr(local_tz, "key", None)
    if isinstance(local_key, str):
        return ZoneInfo(local_key)
    return ZoneInfo("UTC")


def sunday_of_week(dt: datetime, tz: ZoneInfo) -> str:
    """Return the most recent Sunday at or before ``dt`` in ``tz``."""
    if dt.tzinfo is None:
        local_dt = dt.replace(tzinfo=tz)
    else:
        local_dt = dt.astimezone(tz)

    # Why: datetime.weekday() is Monday-first, but weekly_reflection is Sunday-first.
    days_since_sunday = (local_dt.weekday() + 1) % 7
    return (local_dt - timedelta(days=days_since_sunday)).strftime("%Y%m%d")


def format_segment_times(segment: str) -> tuple[str, str] | tuple[None, None]:
    """Format segment start and end times as human-readable strings.

    Parameters
    ----------
    segment:
        Segment key in HHMMSS_LEN format (e.g., "143022_300").

    Returns
    -------
    tuple[str, str] | tuple[None, None]
        Tuple of (start_time, end_time) as formatted strings like "2:30 PM".
        Returns (None, None) if segment format is invalid.

    Examples
    --------
    >>> format_segment_times("143022_300")
    ("2:30 PM", "2:35 PM")
    >>> format_segment_times("090000_3600")
    ("9:00 AM", "10:00 AM")
    """
    start_time, end_time = segment_parse(segment)
    if start_time is None or end_time is None:
        return (None, None)

    return (_format_time(start_time), _format_time(end_time))


def _format_time(t: datetime.time) -> str:
    """Format a time as 12-hour with AM/PM, no leading zero on hour.

    Uses lstrip('0') for cross-platform compatibility (%-I is Unix-only).
    """
    return datetime.combine(datetime.today(), t).strftime("%I:%M %p").lstrip("0")


def _load_default_config() -> dict[str, Any]:
    """Load the default journal configuration from journal_default.json.

    Returns
    -------
    dict
        Default configuration structure.
    """
    default_path = Path(__file__).parent / "journal_default.json"
    with open(default_path, "r", encoding="utf-8") as f:
        return json.load(f)


# Cached default config (loaded once at first use)
_default_config: dict[str, Any] | None = None


def _resolve_os_identity() -> tuple[str, str]:
    """Return (full_name, login_name) from the OS user record, '' on failure."""
    full_name = ""
    login_name = ""
    try:
        entry = pwd.getpwuid(os.getuid())
    except Exception:
        return ("", "")
    try:
        gecos = entry.pw_gecos or ""
        full_name = gecos.split(",", 1)[0].strip()
    except Exception:
        full_name = ""
    try:
        login_name = entry.pw_name or ""
    except Exception:
        login_name = ""
    return (full_name, login_name)


def _zone_from_localtime_path(resolved: str) -> str:
    """Extract the IANA zone name from a resolved /etc/localtime path.

    macOS uses /var/db/timezone/zoneinfo/<Zone>; Linux uses /usr/share/zoneinfo/<Zone>.
    Return everything after the last /zoneinfo/ segment, or '' if absent.
    """
    marker = "/zoneinfo/"
    idx = resolved.rfind(marker)
    return resolved[idx + len(marker) :] if idx != -1 else ""


def _resolve_os_timezone() -> str:
    """Return the system tzdata zone from /etc/localtime, '' on failure."""
    try:
        return _zone_from_localtime_path(str(Path("/etc/localtime").resolve()))
    except Exception:
        return ""


def _write_config_atomic(path: Path, config: dict[str, Any]) -> None:
    from solstone.think.entities.core import atomic_write

    path.parent.mkdir(parents=True, exist_ok=True)
    content = json.dumps(config, indent=2, ensure_ascii=False) + "\n"
    atomic_write(path, content)
    os.chmod(path, 0o600)


def ensure_journal_config() -> dict[str, Any]:
    """Materialize <journal>/config/journal.json and return its contents.

    Idempotent after first creation, with one transitional exception: if the
    file exists but lacks ``convey.secret``, the secret is backfilled. Identity
    fields on an existing file are never modified.
    """
    global _default_config

    journal = Path(get_journal())
    config_path = journal / "config" / "journal.json"

    if config_path.exists():
        with config_path.open(encoding="utf-8") as fh:
            config = json.load(fh)
        if not config.get("convey", {}).get("secret"):
            config.setdefault("convey", {})["secret"] = secrets.token_hex(32)
            _write_config_atomic(config_path, config)
        return config

    if _default_config is None:
        _default_config = _load_default_config()
    config = copy.deepcopy(_default_config)
    try:
        full_name, login_name = _resolve_os_identity()
    except Exception:
        logging.getLogger(__name__).debug(
            "Failed to resolve OS identity", exc_info=True
        )
        full_name = ""
        login_name = ""
    try:
        timezone = _resolve_os_timezone()
    except Exception:
        logging.getLogger(__name__).debug(
            "Failed to resolve OS timezone", exc_info=True
        )
        timezone = ""
    config.setdefault("identity", {})
    config["identity"]["name"] = full_name
    config["identity"]["preferred"] = login_name
    config["identity"]["timezone"] = timezone
    config.setdefault("convey", {})["secret"] = secrets.token_hex(32)
    _write_config_atomic(config_path, config)
    return config


def journal_is_active(path: str | Path) -> bool:
    """Return whether the journal has been onboarded (setup completed)."""
    try:
        journal_path = Path(path)
        if not journal_path.is_dir():
            return False
        config_path = journal_path / "config" / "journal.json"
        with open(config_path, "r", encoding="utf-8") as f:
            config = json.load(f)
        completed = config["setup"]["completed_at"]
        return isinstance(completed, (int, float)) and completed > 0
    except (
        OSError,
        json.JSONDecodeError,
        KeyError,
        TypeError,
        AttributeError,
    ):
        return False


def get_config() -> dict[str, Any]:
    """Return the journal configuration from config/journal.json.

    When no journal.json exists, returns a deep copy of the defaults from
    think/journal_default.json. Once journal.json exists it is the master
    and is returned as-is with no merging of defaults.

    Returns
    -------
    dict
        Journal configuration.
    """
    global _default_config
    if _default_config is None:
        _default_config = _load_default_config()

    journal = get_journal()
    config_path = Path(journal) / "config" / "journal.json"

    # Return defaults when no config file exists yet
    if not config_path.exists():
        return copy.deepcopy(_default_config)

    try:
        with open(config_path, "r", encoding="utf-8") as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError) as exc:
        # Log error but return defaults to avoid breaking callers
        logging.getLogger(__name__).warning(
            "Failed to load config from %s: %s", config_path, exc
        )
        return copy.deepcopy(_default_config)


def _append_task_log(dir_path: str | Path, message: str) -> None:
    """Append ``message`` to ``task_log.txt`` inside ``dir_path``."""
    path = Path(dir_path) / "task_log.txt"
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "a", encoding="utf-8") as f:
            f.write(f"{int(time.time())}\t{message}\n")
    except Exception:
        pass


def day_log(day: str, message: str) -> None:
    """Convenience wrapper to log message for ``day``."""
    _append_task_log(str(day_path(day)), message)


def journal_log(message: str) -> None:
    """Append ``message`` to the journal's ``task_log.txt``."""
    _append_task_log(get_journal(), message)


def day_input_summary(day: str) -> str:
    """Return a human-readable summary of recording data available for a day.

    Uses cluster_segments() to detect recording segments and computes
    total duration from segment keys (HHMMSS_LEN format).

    Parameters
    ----------
    day:
        Day in YYYYMMDD format.

    Returns
    -------
    str
        Human-readable summary like "No recordings", "Light activity: 2 segments,
        ~3 minutes", or "18 segments, ~7.5 hours".
    """
    from solstone.think.cluster import cluster_segments

    segments = cluster_segments(day)

    if not segments:
        return "No recordings"

    # Compute total duration from segment keys (HHMMSS_LEN format)
    total_seconds = 0
    for seg in segments:
        key = seg.get("key", "")
        if "_" in key:
            parts = key.split("_")
            if len(parts) >= 2 and parts[1].isdigit():
                total_seconds += int(parts[1])

    # Format duration
    if total_seconds < 60:
        duration_str = f"~{total_seconds} seconds"
    elif total_seconds < 3600:
        minutes = total_seconds / 60
        duration_str = f"~{minutes:.0f} minutes"
    else:
        hours = total_seconds / 3600
        duration_str = f"~{hours:.1f} hours"

    segment_count = len(segments)

    # Categorize activity level
    if segment_count < 5 or total_seconds < 1800:  # < 5 segments or < 30 min
        return f"Light activity: {segment_count} segment{'s' if segment_count != 1 else ''}, {duration_str}"
    else:
        return f"{segment_count} segments, {duration_str}"


def setup_cli(parser: argparse.ArgumentParser, *, parse_known: bool = False):
    """Parse command line arguments and configure logging.

    The parser will be extended with ``-v``/``--verbose`` and ``-d``/``--debug`` flags.
    The journal path is resolved via get_journal() which auto-creates a default path
    if needed. Environment variables from the journal config's ``env`` section
    (in ``journal.json``) are loaded as fallbacks for any keys not already set.
    The parsed arguments are returned. If ``parse_known`` is ``True`` a tuple of
    ``(args, extra)`` is returned using :func:`argparse.ArgumentParser.parse_known_args`.
    """
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="Enable verbose output"
    )
    parser.add_argument(
        "-d", "--debug", action="store_true", help="Enable debug logging"
    )
    if parse_known:
        args, extra = parser.parse_known_args()
    else:
        args = parser.parse_args()
        extra = None

    if args.debug:
        log_level = logging.DEBUG
    elif args.verbose:
        log_level = logging.INFO
    else:
        log_level = logging.WARNING

    logging.basicConfig(level=log_level)

    # Initialize journal path (auto-creates if needed)
    get_journal()

    # Load config env from journal.json — strict source for API keys
    config = get_config()
    for key, value in config.get("env", {}).items():
        os.environ[key] = str(value)

    return (args, extra) if parse_known else args


def parse_time_range(text: str) -> Optional[tuple[str, str, str]]:
    """Return ``(day, start, end)`` from a natural language time range.

    Parameters
    ----------
    text:
        Natural language description of a time range.

    Returns
    -------
    tuple[str, str, str] | None
        ``(day, start, end)`` if a single range within one day was detected.
        ``day`` is ``YYYYMMDD`` and ``start``/``end`` are ``HHMMSS``. ``None``
        if parsing fails.
    """

    try:
        result = timefhuman(text)
    except Exception as exc:  # pragma: no cover - unexpected library failure
        logging.info("timefhuman failed for %s: %s", text, exc)
        return None

    logging.debug("timefhuman(%s) -> %r", text, result)

    if len(result) != 1:
        logging.info("timefhuman did not return a single expression for %s", text)
        return None

    range_item = result[0]
    if not isinstance(range_item, tuple) or len(range_item) != 2:
        logging.info("Expected a range from %s but got %r", text, range_item)
        return None

    start_dt, end_dt = range_item
    if start_dt.date() != end_dt.date():
        logging.info("Range must be within a single day: %s -> %s", start_dt, end_dt)
        return None

    day = start_dt.strftime("%Y%m%d")
    start = start_dt.strftime("%H%M%S")
    end = end_dt.strftime("%H%M%S")
    return day, start, end


def get_raw_file(day: str, name: str) -> tuple[str, str, Any]:
    """Return raw file path, mime type and metadata for a transcript.

    Parameters
    ----------
    day:
        Day folder in ``YYYYMMDD`` format.
    name:
        Transcript filename such as ``HHMMSS/audio.jsonl``,
        ``HHMMSS/monitor_1_diff.json``, or ``HHMMSS/screen.jsonl``.

    Returns
    -------
    tuple[str, str, Any]
        ``(path, mime_type, metadata)`` where ``path`` is relative to the day
        directory (read from metadata header), ``mime_type`` is determined
        from the raw file extension, and ``metadata`` contains the parsed
        JSON data (empty on failure).
    """

    day_dir = day_path(day)
    transcript_path = day_dir / name

    rel = None
    meta: Any = {}

    try:
        with open(transcript_path, "r", encoding="utf-8") as f:
            if name.endswith(".jsonl"):
                # First line is metadata header with "raw" field
                first_line = f.readline().strip()
                if first_line:
                    header = json.loads(first_line)
                    rel = header.get("raw")

                # Read remaining lines as metadata
                meta = [json.loads(line) for line in f if line.strip()]
            else:
                # Non-JSONL format (e.g., _diff.json)
                meta = json.load(f)
                rel = meta.get("raw")
    except Exception:  # pragma: no cover - optional metadata
        logging.debug("Failed to read %s", transcript_path)

    if not rel:
        raise ValueError(f"No 'raw' field found in metadata for {name}")

    suffix = Path(rel).suffix.lower()
    mime = {**MIME_TYPES, ".png": "image/png"}.get(suffix, "application/octet-stream")

    return rel, mime, meta


# =============================================================================
# SOL_* Environment Variable Helpers
# =============================================================================


def get_sol_day() -> str | None:
    """Read SOL_DAY from the environment."""
    return os.environ.get("SOL_DAY") or None


def get_sol_facet() -> str | None:
    """Read SOL_FACET from the environment."""
    return os.environ.get("SOL_FACET") or None


def get_sol_segment() -> str | None:
    """Read SOL_SEGMENT from the environment."""
    return os.environ.get("SOL_SEGMENT") or None


def get_sol_stream() -> str | None:
    """Read SOL_STREAM from the environment."""
    return os.environ.get("SOL_STREAM") or None


def get_sol_activity() -> str | None:
    """Read SOL_ACTIVITY from the environment."""
    return os.environ.get("SOL_ACTIVITY") or None


def resolve_sol_day(arg: str | None) -> str:
    """Return *arg* if provided, else SOL_DAY from env, else exit with error.

    Intended for CLI commands where ``day`` is required but can be supplied
    via the SOL_DAY environment variable as a convenience.
    """
    if arg:
        return arg
    env = get_sol_day()
    if env:
        return env
    import typer

    typer.echo("Error: day is required (pass as argument or set SOL_DAY).", err=True)
    raise typer.Exit(1)


def resolve_sol_day_or_today(arg: str | None) -> str:
    """Return *arg* if provided, else SOL_DAY from env, else today.

    For read-only ``list`` commands where omitting the day should default to
    today rather than erroring. Do NOT use for write commands — those rely on
    ``resolve_sol_day`` erroring when no day is given.
    """
    if arg:
        return arg
    env = get_sol_day()
    if env:
        return env
    return datetime.now().strftime("%Y%m%d")


def resolve_sol_facet(arg: str | None) -> str:
    """Return *arg* if provided, else SOL_FACET from env, else exit with error.

    Intended for CLI commands where ``facet`` is required but can be supplied
    via the SOL_FACET environment variable as a convenience.
    """
    if arg:
        return arg
    env = get_sol_facet()
    if env:
        return env
    import typer

    typer.echo(
        "Error: facet is required (pass as argument or set SOL_FACET).", err=True
    )
    raise typer.Exit(1)


def resolve_sol_segment(arg: str | None) -> str | None:
    """Return *arg* if provided, else SOL_SEGMENT from env, else None.

    Unlike :func:`resolve_sol_day` this does **not** error when missing
    because segment is typically optional.
    """
    if arg:
        return arg
    return get_sol_segment()


# =============================================================================
# Service Port Discovery
# =============================================================================


def find_available_port(host: str = "127.0.0.1") -> int:
    """Find an available port by binding to port 0.

    Uses the socket bind/getsockname/close pattern to let the OS assign
    an available port.

    Args:
        host: Host address to bind to (default: 127.0.0.1)

    Returns:
        Available port number
    """
    import socket

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind((host, 0))
    _, port = sock.getsockname()
    sock.close()
    return port


def write_service_port(service: str, port: int) -> None:
    """Write a service's port to the health directory.

    Creates journal/health/{service}.port with the port number.

    Args:
        service: Service name (e.g., "convey", "cortex")
        port: Port number to write
    """
    health_dir = Path(get_journal()) / "health"
    health_dir.mkdir(parents=True, exist_ok=True)
    port_file = health_dir / f"{service}.port"
    port_file.write_text(str(port))


def read_service_port(service: str) -> int | None:
    """Read a service's port from the health directory.

    Args:
        service: Service name (e.g., "convey", "cortex")

    Returns:
        Port number if file exists and is valid, None otherwise
    """
    port_file = Path(get_journal()) / "health" / f"{service}.port"
    try:
        return int(port_file.read_text().strip())
    except (FileNotFoundError, ValueError):
        return None


def is_solstone_up(timeout: float = 0.2) -> bool:
    """Return True if convey is accepting TCP connections on its recorded port."""
    port = read_service_port("convey")
    if port is None:
        return False
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=timeout):
            return True
    except OSError:
        return False


def require_solstone() -> None:
    """Exit(1) with a clear message if solstone's stack isn't running."""
    if os.environ.get("SOL_SKIP_SUPERVISOR_CHECK") == "1":
        return
    if is_solstone_up():
        return
    if os.environ.get("SOL_SUPERVISOR_SPAWNED") == "1":
        sys.exit(EXIT_TEMPFAIL)
    print(
        "sol: solstone isn't running. Start it with 'journal up' and retry.",
        file=sys.stderr,
    )
    sys.exit(1)
