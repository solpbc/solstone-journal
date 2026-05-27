# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

from datetime import date
from pathlib import Path
from zoneinfo import ZoneInfo

import solstone.apps.reflections.dates as reflection_dates
from solstone.apps.reflections.dates import next_reflection_sunday


def _seed_chronicle_day(journal: Path, day: str) -> None:
    (journal / "chronicle" / day).mkdir(parents=True)


def test_next_reflection_sunday_empty_chronicle_returns_next_sunday(tmp_path):
    result = next_reflection_sunday(
        tmp_path,
        date(2026, 3, 9),
        ZoneInfo("UTC"),
    )

    assert result == "Sunday, March 15"


def test_next_reflection_sunday_empty_chronicle_returns_today_when_today_is_sunday(
    tmp_path,
):
    result = next_reflection_sunday(
        tmp_path,
        date(2026, 3, 8),
        ZoneInfo("UTC"),
    )

    assert result == "Sunday, March 8"


def test_next_reflection_sunday_old_chronicle_anchors_today(tmp_path):
    _seed_chronicle_day(tmp_path, "20260301")

    result = next_reflection_sunday(
        tmp_path,
        date(2026, 3, 20),
        ZoneInfo("UTC"),
    )

    assert result == "Sunday, March 22"


def test_next_reflection_sunday_recent_chronicle_waits_until_day_seven(tmp_path):
    _seed_chronicle_day(tmp_path, "20260306")

    result = next_reflection_sunday(
        tmp_path,
        date(2026, 3, 8),
        ZoneInfo("UTC"),
    )

    assert result == "Sunday, March 15"


def test_next_reflection_sunday_cross_year_includes_year(tmp_path):
    result = next_reflection_sunday(
        tmp_path,
        date(2026, 12, 28),
        ZoneInfo("UTC"),
    )

    assert result == "Sunday, January 3, 2027"


def test_next_reflection_sunday_returns_none_on_filesystem_error(
    monkeypatch,
    tmp_path,
):
    def fail(_journal):
        raise OSError("no chronicle")

    monkeypatch.setattr(reflection_dates, "_earliest_chronicle_day", fail)

    result = next_reflection_sunday(
        tmp_path,
        date(2026, 3, 8),
        ZoneInfo("UTC"),
    )

    assert result is None
