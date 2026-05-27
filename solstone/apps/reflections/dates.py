# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Compute the next reflection Sunday for the reflections empty-state copy."""

from datetime import date, datetime, timedelta
from pathlib import Path
from zoneinfo import ZoneInfo


def next_reflection_sunday(journal: Path, today: date, tz: ZoneInfo) -> str | None:
    """Return a formatted Sunday string for the empty-state copy, or None."""
    _ = tz
    # Owner-facing copy — degrade to fallback on any filesystem/parse error.
    try:
        earliest = _earliest_chronicle_day(journal)
        if earliest is None:
            anchor = today
        else:
            anchor = max(today, earliest + timedelta(days=7))
        sunday = _next_sunday_on_or_after(anchor)
        return _format_sunday(sunday, today.year)
    except Exception:
        return None


def _earliest_chronicle_day(journal: Path) -> date | None:
    chronicle_dir = journal / "chronicle"
    if not chronicle_dir.exists():
        return None
    earliest: date | None = None
    for child in chronicle_dir.iterdir():
        if not child.is_dir():
            continue
        name = child.name
        if len(name) != 8 or not name.isdigit():
            continue
        try:
            parsed = datetime.strptime(name, "%Y%m%d").date()
        except ValueError:
            continue
        if earliest is None or parsed < earliest:
            earliest = parsed
    return earliest


def _next_sunday_on_or_after(d: date) -> date:
    # Python weekday: Monday=0 ... Sunday=6.
    days_until_sunday = (6 - d.weekday()) % 7
    return d + timedelta(days=days_until_sunday)


def _format_sunday(sunday: date, today_year: int) -> str:
    month = sunday.strftime("%B")
    day = str(sunday.day)
    if sunday.year == today_year:
        return f"Sunday, {month} {day}"
    return f"Sunday, {month} {day}, {sunday.year}"
