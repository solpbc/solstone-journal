# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for scheduler — clock-aligned task scheduler."""

import json
import time
from contextlib import contextmanager
from datetime import datetime
from pathlib import Path
from unittest.mock import Mock

import pytest

from solstone.think import scheduler


@contextmanager
def _fake_now(dt: datetime):
    """Temporarily replace scheduler.datetime with a fake that returns dt."""

    class _FakeDatetime:
        min = datetime.min

        @staticmethod
        def now():
            return dt

        @staticmethod
        def fromtimestamp(ts):
            return datetime.fromtimestamp(ts)

        @staticmethod
        def combine(*a, **k):
            return datetime.combine(*a, **k)

    scheduler.datetime = _FakeDatetime
    try:
        yield
    finally:
        scheduler.datetime = datetime


@pytest.fixture(autouse=True)
def reset_scheduler_state():
    """Reset scheduler module state between tests."""
    import solstone.think.scheduler as mod

    mod._entries = {}
    mod._state = {}
    mod._callosum = None
    mod._last_hour = None
    mod._daily_time = None
    mod._last_daily_mark = None
    mod._weekly_day = None
    mod._weekly_time = None
    mod._last_weekly_mark = None
    yield
    mod._entries = {}
    mod._state = {}
    mod._callosum = None
    mod._last_hour = None
    mod._daily_time = None
    mod._last_daily_mark = None
    mod._weekly_day = None
    mod._weekly_time = None
    mod._last_weekly_mark = None


@pytest.fixture
def journal_path(tmp_path, monkeypatch):
    """Create a temp journal with config/ and health/ dirs."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "config").mkdir()
    (tmp_path / "health").mkdir()
    return tmp_path


def _write_config(journal: Path, config: dict) -> None:
    with open(journal / "config" / "schedules.json", "w") as f:
        json.dump(config, f)


def _write_state(journal: Path, state: dict) -> None:
    with open(journal / "health" / "scheduler.json", "w") as f:
        json.dump(state, f)


def _read_state(journal: Path) -> dict:
    with open(journal / "health" / "scheduler.json") as f:
        return json.load(f)


# ---------------------------------------------------------------------------
# load_config
# ---------------------------------------------------------------------------


class TestLoadConfig:
    def test_valid_config(self, journal_path):
        _write_config(
            journal_path,
            {
                "sync:plaud": {
                    "cmd": ["sol", "import", "--sync", "plaud"],
                    "every": "hourly",
                },
            },
        )
        from solstone.think.scheduler import load_config

        entries = load_config()
        assert "sync:plaud" in entries
        assert entries["sync:plaud"]["every"] == "hourly"
        assert entries["sync:plaud"]["cmd"] == ["sol", "import", "--sync", "plaud"]

    def test_missing_file_returns_empty(self, journal_path):
        from solstone.think.scheduler import load_config

        assert load_config() == {}

    def test_invalid_json_returns_empty(self, journal_path):
        (journal_path / "config" / "schedules.json").write_text("not json{")
        from solstone.think.scheduler import load_config

        assert load_config() == {}

    def test_unknown_every_skipped(self, journal_path):
        _write_config(
            journal_path,
            {
                "bad": {"cmd": ["sol", "noop"], "every": "biweekly"},
            },
        )
        from solstone.think.scheduler import load_config

        assert load_config() == {}

    def test_missing_cmd_skipped(self, journal_path):
        _write_config(
            journal_path,
            {
                "bad": {"every": "hourly"},
            },
        )
        from solstone.think.scheduler import load_config

        assert load_config() == {}

    def test_disabled_entry_excluded(self, journal_path):
        _write_config(
            journal_path,
            {
                "off": {"cmd": ["sol", "noop"], "every": "hourly", "enabled": False},
            },
        )
        from solstone.think.scheduler import load_config

        assert load_config() == {}

    def test_max_runtime_valid_string_round_trips(self, journal_path):
        # D-E/D-F: assert the accepted Plaud cap via test-local config,
        # leaving the synthetic fixture schedule minimal.
        _write_config(
            journal_path,
            {
                "sync:plaud": {
                    "cmd": ["sol", "import", "--sync", "plaud"],
                    "every": "hourly",
                    "max_runtime": "30m",
                },
            },
        )
        from solstone.think.scheduler import load_config

        entries = load_config()
        assert entries["sync:plaud"]["max_runtime"] == 1800

    def test_max_runtime_valid_int_round_trips(self, journal_path):
        _write_config(
            journal_path,
            {
                "sync:plaud": {
                    "cmd": ["sol", "import", "--sync", "plaud"],
                    "every": "hourly",
                    "max_runtime": 1800,
                },
            },
        )
        from solstone.think.scheduler import load_config

        entries = load_config()
        assert entries["sync:plaud"]["max_runtime"] == 1800

    def test_max_runtime_invalid_negative_logged_and_dropped(
        self, journal_path, caplog
    ):
        _write_config(
            journal_path,
            {
                "sync:plaud": {
                    "cmd": ["sol", "import", "--sync", "plaud"],
                    "every": "hourly",
                    "max_runtime": -5,
                },
            },
        )
        from solstone.think.scheduler import load_config

        entries = load_config()
        assert "max_runtime" not in entries["sync:plaud"]
        assert "Schedule 'sync:plaud': invalid max_runtime -5" in caplog.text

    def test_max_runtime_invalid_garbage_logged_and_dropped(self, journal_path, caplog):
        _write_config(
            journal_path,
            {
                "sync:plaud": {
                    "cmd": ["sol", "import", "--sync", "plaud"],
                    "every": "hourly",
                    "max_runtime": "garbage",
                },
            },
        )
        from solstone.think.scheduler import load_config

        entries = load_config()
        assert "max_runtime" not in entries["sync:plaud"]
        assert "Schedule 'sync:plaud': invalid max_runtime 'garbage'" in caplog.text

    def test_max_runtime_invalid_type_logged_and_dropped(self, journal_path, caplog):
        _write_config(
            journal_path,
            {
                "sync:plaud": {
                    "cmd": ["sol", "import", "--sync", "plaud"],
                    "every": "hourly",
                    "max_runtime": [1, 2],
                },
            },
        )
        from solstone.think.scheduler import load_config

        entries = load_config()
        assert "max_runtime" not in entries["sync:plaud"]
        assert "Schedule 'sync:plaud': invalid max_runtime [1, 2]" in caplog.text

    def test_collect_runtime_caps_returns_only_capped_entries(self, journal_path):
        _write_config(
            journal_path,
            {
                "sync:plaud": {
                    "cmd": ["sol", "import", "--sync", "plaud"],
                    "every": "hourly",
                    "max_runtime": "30m",
                },
                "heartbeat": {
                    "cmd": ["journal", "heartbeat"],
                    "every": "daily",
                },
            },
        )
        import solstone.think.scheduler as mod

        mod.init(Mock())

        assert mod.collect_runtime_caps() == [
            (["sol", "import", "--sync", "plaud"], 1800)
        ]


# ---------------------------------------------------------------------------
# load_state / save_state
# ---------------------------------------------------------------------------


class TestState:
    def test_round_trip(self, journal_path):
        import solstone.think.scheduler as mod

        mod._state = {"sync:plaud": {"last_run": 1700000000.0}}
        mod.save_state()

        loaded = mod.load_state()
        assert loaded["sync:plaud"]["last_run"] == 1700000000.0

    def test_missing_file_returns_empty(self, journal_path):
        from solstone.think.scheduler import load_state

        assert load_state() == {}

    def test_atomic_write_no_partial(self, journal_path):
        """State file shouldn't have leftover tmp files on success."""
        import solstone.think.scheduler as mod

        mod._state = {"a": {"last_run": 1.0}}
        mod.save_state()

        tmps = list((journal_path / "health").glob(".scheduler_*"))
        assert tmps == []


# ---------------------------------------------------------------------------
# _is_due
# ---------------------------------------------------------------------------


class TestIsDue:
    def test_no_state_is_due(self):
        from solstone.think.scheduler import _is_due

        entry = {"cmd": ["sol", "x"], "every": "hourly"}
        assert _is_due(entry, None, datetime(2026, 2, 17, 14, 30)) is True

    def test_hourly_same_hour_not_due(self):
        from solstone.think.scheduler import _is_due

        entry = {"cmd": ["sol", "x"], "every": "hourly"}
        # Last run at 14:05, now is 14:30 — same hour
        state = {"last_run": datetime(2026, 2, 17, 14, 5).timestamp()}
        assert _is_due(entry, state, datetime(2026, 2, 17, 14, 30)) is False

    def test_hourly_new_hour_is_due(self):
        from solstone.think.scheduler import _is_due

        entry = {"cmd": ["sol", "x"], "every": "hourly"}
        # Last run at 13:45, now is 14:01 — new hour
        state = {"last_run": datetime(2026, 2, 17, 13, 45).timestamp()}
        assert _is_due(entry, state, datetime(2026, 2, 17, 14, 1)) is True

    def test_daily_same_day_not_due(self):
        from solstone.think.scheduler import _is_due

        entry = {"cmd": ["sol", "x"], "every": "daily"}
        # Last run today at 00:05, now is 14:00
        state = {"last_run": datetime(2026, 2, 17, 0, 5).timestamp()}
        assert _is_due(entry, state, datetime(2026, 2, 17, 14, 0)) is False

    def test_daily_new_day_is_due(self):
        from solstone.think.scheduler import _is_due

        entry = {"cmd": ["sol", "x"], "every": "daily"}
        # Last run yesterday at 23:50, now is 00:01
        state = {"last_run": datetime(2026, 2, 16, 23, 50).timestamp()}
        assert _is_due(entry, state, datetime(2026, 2, 17, 0, 1)) is True


class TestDailyTime:
    """Tests for daily_time-aware scheduling."""

    def test_load_config_extracts_daily_time(self, journal_path):
        """load_config extracts daily_time from schedules.json."""
        import solstone.think.scheduler as mod

        _write_config(
            journal_path,
            {
                "daily_time": "03:00",
                "a": {"cmd": ["sol", "x"], "every": "daily"},
            },
        )
        entries = mod.load_config()
        assert "a" in entries
        assert "daily_time" not in entries  # Not a schedule entry
        assert mod._daily_time == "03:00"

    def test_load_config_no_daily_time(self, journal_path):
        """When daily_time is absent, _daily_time is None."""
        import solstone.think.scheduler as mod

        _write_config(journal_path, {"a": {"cmd": ["sol", "x"], "every": "hourly"}})
        mod.load_config()
        assert mod._daily_time is None

    def test_load_config_invalid_daily_time(self, journal_path):
        """Non-string daily_time is ignored."""
        import solstone.think.scheduler as mod

        _write_config(
            journal_path,
            {
                "daily_time": 300,
                "a": {"cmd": ["sol", "x"], "every": "hourly"},
            },
        )
        mod.load_config()
        assert mod._daily_time is None

    def test_is_due_with_daily_time(self):
        """Daily task is due when last_run is before the daily_time boundary."""
        import solstone.think.scheduler as mod

        mod._daily_time = "03:00"
        entry = {"cmd": ["sol", "x"], "every": "daily"}
        # Last run at 02:00 today, now is 04:00 — past the 03:00 boundary
        state = {"last_run": datetime(2026, 2, 17, 2, 0).timestamp()}
        assert mod._is_due(entry, state, datetime(2026, 2, 17, 4, 0)) is True

    def test_not_due_after_daily_time_boundary(self):
        """Daily task is not due when last_run is after the daily_time boundary."""
        import solstone.think.scheduler as mod

        mod._daily_time = "03:00"
        entry = {"cmd": ["sol", "x"], "every": "daily"}
        # Last run at 03:30 today, now is 04:00 — already ran after boundary
        state = {"last_run": datetime(2026, 2, 17, 3, 30).timestamp()}
        assert mod._is_due(entry, state, datetime(2026, 2, 17, 4, 0)) is False

    def test_not_due_before_daily_time_boundary(self):
        """Before the daily_time, yesterday's boundary applies."""
        import solstone.think.scheduler as mod

        mod._daily_time = "03:00"
        entry = {"cmd": ["sol", "x"], "every": "daily"}
        # Last run at 04:00 yesterday, now is 02:00 today
        # Yesterday's boundary was yesterday 03:00, last_run > that → not due
        state = {"last_run": datetime(2026, 2, 16, 4, 0).timestamp()}
        assert mod._is_due(entry, state, datetime(2026, 2, 17, 2, 0)) is False

    def test_check_fires_at_daily_time_not_midnight(self, journal_path):
        """check() fires daily tasks at the configured daily_time, not midnight."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "daily_time": "03:00",
                "d": {"cmd": ["sol", "daily-thing"], "every": "daily"},
            },
        )

        mod.init(callosum)

        # Set boundaries: last check at 02:59 (before 03:00 boundary)
        # _last_daily_mark should be yesterday's 03:00 since 02:59 < 03:00
        mod._last_hour = datetime(2026, 2, 17, 2, 0)
        mod._last_daily_mark = datetime(2026, 2, 16, 3, 0)

        # Now cross to 03:01 — daily mark changes from yesterday's to today's
        with _fake_now(datetime(2026, 2, 17, 3, 1)):
            mod.check()

        callosum.emit.assert_called_once()
        assert callosum.emit.call_args[1]["cmd"] == ["sol", "daily-thing"]

    def test_check_no_fire_at_midnight_with_daily_time(self, journal_path):
        """Midnight does not trigger daily tasks when daily_time is set."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "daily_time": "03:00",
                "d": {"cmd": ["sol", "daily-thing"], "every": "daily"},
            },
        )

        # State: ran at 03:30 yesterday (after yesterday's boundary)
        _write_state(
            journal_path,
            {"d": {"last_run": datetime(2026, 2, 16, 3, 30).timestamp()}},
        )

        mod.init(callosum)

        # Set boundaries at 23:59 Feb 16
        mod._last_hour = datetime(2026, 2, 16, 23, 0)
        mod._last_daily_mark = datetime(2026, 2, 16, 3, 0)

        # Cross midnight to 00:01 Feb 17 — hour changes but daily mark stays
        # at Feb 16 03:00 (since 00:01 < 03:00, yesterday's mark applies)
        with _fake_now(datetime(2026, 2, 17, 0, 1)):
            mod.check()

        # Only hourly tasks would fire, not daily (no daily_mark_changed)
        # Since we only have a daily task and daily mark didn't change, nothing fires
        callosum.emit.assert_not_called()

    def test_format_next_due_with_daily_time(self):
        """_format_next_due shows configured time instead of midnight."""
        import solstone.think.scheduler as mod

        mod._daily_time = "03:00"
        entry = {"cmd": ["sol", "x"], "every": "daily"}
        # Last ran after the boundary — not currently due
        state = {"last_run": datetime(2026, 2, 17, 3, 30).timestamp()}
        now = datetime(2026, 2, 17, 14, 0)

        result = mod._format_next_due(entry, state, now)
        assert result == "03:00"

    def test_format_next_due_no_daily_time(self):
        """_format_next_due shows midnight when no daily_time configured."""
        import solstone.think.scheduler as mod

        mod._daily_time = None
        entry = {"cmd": ["sol", "x"], "every": "daily"}
        state = {"last_run": datetime(2026, 2, 17, 0, 5).timestamp()}
        now = datetime(2026, 2, 17, 14, 0)

        result = mod._format_next_due(entry, state, now)
        assert result == "midnight"

    def test_invalid_daily_time_falls_back_to_midnight(self, journal_path):
        """Invalid daily_time string falls back to midnight behavior."""
        import solstone.think.scheduler as mod

        _write_config(
            journal_path,
            {
                "daily_time": "not-a-time",
                "d": {"cmd": ["sol", "daily-thing"], "every": "daily"},
            },
        )
        mod.load_config()
        assert mod._daily_time == "not-a-time"  # Stored as-is

        entry = {"cmd": ["sol", "x"], "every": "daily"}
        # _parse_daily_time returns None for invalid → midnight fallback
        # Last run yesterday, now is today → due (midnight boundary)
        state = {"last_run": datetime(2026, 2, 16, 23, 50).timestamp()}
        assert mod._is_due(entry, state, datetime(2026, 2, 17, 0, 1)) is True

    def test_collect_status_includes_daily_time(self, journal_path):
        """collect_status includes daily_time for daily entries."""
        import solstone.think.scheduler as mod

        mod._daily_time = "03:00"
        mod._entries = {"d": {"cmd": ["sol", "x"], "every": "daily"}}
        mod._state = {}

        status = mod.collect_status()
        assert len(status) == 1
        assert status[0]["daily_time"] == "03:00"

    def test_collect_status_no_daily_time_for_hourly(self, journal_path):
        """collect_status does not include daily_time for hourly entries."""
        import solstone.think.scheduler as mod

        mod._daily_time = "03:00"
        mod._entries = {"h": {"cmd": ["sol", "x"], "every": "hourly"}}
        mod._state = {}

        status = mod.collect_status()
        assert "daily_time" not in status[0]


class TestWeeklyTime:
    """Tests for weekly scheduling — boundary computation and config parsing."""

    def test_load_config_extracts_weekly_day_and_time(self, journal_path):
        """load_config extracts weekly_day and weekly_time from schedules.json."""
        import solstone.think.scheduler as mod

        _write_config(
            journal_path,
            {
                "weekly_day": "sunday",
                "weekly_time": "04:00",
                "w": {"cmd": ["journal", "think", "--weekly"], "every": "weekly"},
            },
        )
        entries = mod.load_config()
        assert "w" in entries
        assert "weekly_day" not in entries
        assert "weekly_time" not in entries
        assert mod._weekly_day == "sunday"
        assert mod._weekly_time == "04:00"

    def test_load_config_no_weekly_config(self, journal_path):
        """When weekly_day/weekly_time are absent, globals are None."""
        import solstone.think.scheduler as mod

        _write_config(journal_path, {"a": {"cmd": ["sol", "x"], "every": "hourly"}})
        mod.load_config()
        assert mod._weekly_day is None
        assert mod._weekly_time is None

    def test_load_config_invalid_weekly_day(self, journal_path):
        """Invalid weekly_day string is ignored."""
        import solstone.think.scheduler as mod

        _write_config(
            journal_path,
            {
                "weekly_day": "notaday",
                "w": {"cmd": ["sol", "x"], "every": "weekly"},
            },
        )
        mod.load_config()
        assert mod._weekly_day is None

    def test_load_config_non_string_weekly_day(self, journal_path):
        """Non-string weekly_day is ignored."""
        import solstone.think.scheduler as mod

        _write_config(
            journal_path,
            {
                "weekly_day": 0,
                "w": {"cmd": ["sol", "x"], "every": "weekly"},
            },
        )
        mod.load_config()
        assert mod._weekly_day is None

    def test_weekly_day_case_insensitive(self, journal_path):
        """Day name parsing is case-insensitive and accepts abbreviations."""
        import solstone.think.scheduler as mod

        for name in ["Sunday", "SUNDAY", "sun", "Sun"]:
            _write_config(
                journal_path,
                {
                    "weekly_day": name,
                    "w": {"cmd": ["sol", "x"], "every": "weekly"},
                },
            )
            mod.load_config()
            assert mod._weekly_day == name

    def test_compute_weekly_mark_past_boundary(self):
        """When now is past this week's target, returns this week's boundary."""
        import solstone.think.scheduler as mod

        now = datetime(2026, 3, 22, 4, 0)
        mark = mod._compute_weekly_mark(now, 6, "03:00")
        assert mark == datetime(2026, 3, 22, 3, 0)

    def test_compute_weekly_mark_before_boundary(self):
        """When now is before this week's target, returns last week's boundary."""
        import solstone.think.scheduler as mod

        now = datetime(2026, 3, 22, 2, 0)
        mark = mod._compute_weekly_mark(now, 6, "03:00")
        assert mark == datetime(2026, 3, 15, 3, 0)

    def test_compute_weekly_mark_midweek(self):
        """Midweek, returns the most recent target day occurrence."""
        import solstone.think.scheduler as mod

        now = datetime(2026, 3, 25, 10, 0)
        mark = mod._compute_weekly_mark(now, 6, "03:00")
        assert mark == datetime(2026, 3, 22, 3, 0)

    def test_compute_weekly_mark_no_time_defaults_to_0300(self):
        """When weekly_time is None, boundary defaults to 03:00."""
        import solstone.think.scheduler as mod

        now = datetime(2026, 3, 22, 4, 0)
        mark = mod._compute_weekly_mark(now, 6, None)
        assert mark == datetime(2026, 3, 22, 3, 0)

    def test_is_due_weekly_due(self):
        """Weekly task is due when last_run is before the weekly boundary."""
        import solstone.think.scheduler as mod

        mod._weekly_day = "sunday"
        mod._weekly_time = "03:00"
        entry = {"cmd": ["sol", "x"], "every": "weekly"}
        state = {"last_run": datetime(2026, 3, 21, 10, 0).timestamp()}
        assert mod._is_due(entry, state, datetime(2026, 3, 22, 4, 0)) is True

    def test_is_due_weekly_not_due(self):
        """Weekly task is not due when last_run is after the weekly boundary."""
        import solstone.think.scheduler as mod

        mod._weekly_day = "sunday"
        mod._weekly_time = "03:00"
        entry = {"cmd": ["sol", "x"], "every": "weekly"}
        state = {"last_run": datetime(2026, 3, 22, 4, 0).timestamp()}
        assert mod._is_due(entry, state, datetime(2026, 3, 25, 10, 0)) is False

    def test_is_due_weekly_no_state(self):
        """Weekly task with no prior run is always due."""
        import solstone.think.scheduler as mod

        mod._weekly_day = "sunday"
        entry = {"cmd": ["sol", "x"], "every": "weekly"}
        assert mod._is_due(entry, None, datetime(2026, 3, 25, 10, 0)) is True

    def test_check_fires_at_weekly_boundary(self, journal_path):
        """check() fires weekly tasks when the weekly boundary is crossed."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "weekly_day": "sunday",
                "weekly_time": "03:00",
                "w": {"cmd": ["journal", "think", "--weekly"], "every": "weekly"},
            },
        )

        mod.init(callosum)

        mod._last_hour = datetime(2026, 3, 21, 23, 0)
        mod._last_daily_mark = datetime(2026, 3, 21, 0, 0)
        mod._last_weekly_mark = datetime(2026, 3, 15, 3, 0)

        with _fake_now(datetime(2026, 3, 22, 3, 1)):
            mod.check()

        callosum.emit.assert_called_once()
        assert callosum.emit.call_args[1]["cmd"] == [
            "journal",
            "think",
            "--weekly",
        ]

    def test_check_no_fire_before_weekly_boundary(self, journal_path):
        """check() does not fire weekly tasks before the weekly boundary."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "weekly_day": "sunday",
                "weekly_time": "03:00",
                "w": {"cmd": ["journal", "think", "--weekly"], "every": "weekly"},
            },
        )

        _write_state(
            journal_path,
            {"w": {"last_run": datetime(2026, 3, 15, 4, 0).timestamp()}},
        )

        mod.init(callosum)

        mod._last_hour = datetime(2026, 3, 21, 22, 0)
        mod._last_daily_mark = datetime(2026, 3, 21, 0, 0)
        mod._last_weekly_mark = datetime(2026, 3, 15, 3, 0)

        with _fake_now(datetime(2026, 3, 21, 23, 1)):
            mod.check()

        callosum.emit.assert_not_called()

    def test_missed_weeks_runs_once(self, journal_path):
        """If supervisor was down for 3 weeks, weekly agent runs once on restart."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "weekly_day": "sunday",
                "weekly_time": "03:00",
                "w": {"cmd": ["journal", "think", "--weekly"], "every": "weekly"},
            },
        )

        _write_state(
            journal_path,
            {"w": {"last_run": datetime(2026, 3, 1, 4, 0).timestamp()}},
        )

        mod.init(callosum)

        mod._last_hour = datetime(2026, 3, 22, 2, 0)
        mod._last_daily_mark = datetime(2026, 3, 22, 0, 0)
        mod._last_weekly_mark = datetime(2026, 3, 15, 3, 0)

        with _fake_now(datetime(2026, 3, 22, 3, 1)):
            mod.check()

        callosum.emit.assert_called_once()

    def test_dedup_same_week_not_due(self):
        """After running this week, weekly agent is not due again."""
        import solstone.think.scheduler as mod

        mod._weekly_day = "sunday"
        mod._weekly_time = "03:00"
        entry = {"cmd": ["sol", "x"], "every": "weekly"}
        state = {"last_run": datetime(2026, 3, 22, 3, 30).timestamp()}
        assert mod._is_due(entry, state, datetime(2026, 3, 26, 10, 0)) is False

    def test_format_next_due_weekly(self):
        """_format_next_due shows next weekday and time."""
        import solstone.think.scheduler as mod

        mod._weekly_day = "sunday"
        mod._weekly_time = "03:00"
        entry = {"cmd": ["sol", "x"], "every": "weekly"}
        state = {"last_run": datetime(2026, 3, 22, 4, 0).timestamp()}
        now = datetime(2026, 3, 25, 10, 0)

        result = mod._format_next_due(entry, state, now)
        assert "Sunday" in result
        assert "03:00" in result

    def test_collect_status_includes_weekly_fields(self, journal_path):
        """collect_status includes weekly_day and weekly_time for weekly entries."""
        import solstone.think.scheduler as mod

        mod._weekly_day = "sunday"
        mod._weekly_time = "04:00"
        mod._entries = {"w": {"cmd": ["sol", "x"], "every": "weekly"}}
        mod._state = {}

        status = mod.collect_status()
        assert len(status) == 1
        assert status[0]["weekly_day"] == "sunday"
        assert status[0]["weekly_time"] == "04:00"


# ---------------------------------------------------------------------------
# init
# ---------------------------------------------------------------------------


class TestInit:
    def test_loads_config_and_state(self, journal_path):
        _write_config(
            journal_path,
            {
                "a": {"cmd": ["sol", "x"], "every": "hourly"},
            },
        )
        _write_state(journal_path, {"a": {"last_run": 1700000000.0}})

        import solstone.think.scheduler as mod

        callosum = Mock()
        mod.init(callosum)

        assert "a" in mod._entries
        assert mod._state["a"]["last_run"] == 1700000000.0
        assert mod._callosum is callosum
        assert mod._last_hour is not None
        assert mod._last_daily_mark is not None

    def test_no_config_file(self, journal_path):
        import solstone.think.scheduler as mod

        mod.init(Mock())
        assert mod._entries == {}


# ---------------------------------------------------------------------------
# check
# ---------------------------------------------------------------------------


class TestCheck:
    def test_pre_init_returns_immediately(self, journal_path):
        """check() does nothing when init() hasn't been called."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)
        mod._callosum = callosum

        mod.check()
        callosum.emit.assert_not_called()

    def test_no_boundary_no_io(self, journal_path):
        """When no boundary has crossed, check() does nothing."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)
        now = datetime(2026, 2, 17, 14, 30)

        _write_config(
            journal_path,
            {
                "a": {"cmd": ["sol", "x"], "every": "hourly"},
            },
        )

        mod.init(callosum)
        # Set boundaries to current — no crossing
        mod._last_hour = mod._hour_mark(now)
        mod._last_daily_mark = datetime(2026, 2, 17, 0, 0)

        with _fake_now(now):
            mod.check()

        callosum.emit.assert_not_called()

    def test_hourly_boundary_submits(self, journal_path):
        """Crossing an hour boundary submits due hourly tasks."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "a": {"cmd": ["sol", "test-task", "-v"], "every": "hourly"},
            },
        )

        mod.init(callosum)

        # Simulate: last check was at 13:59, now it's 14:01
        mod._last_hour = datetime(2026, 2, 17, 13, 0)
        mod._last_daily_mark = datetime(2026, 2, 17, 0, 0)
        # No prior state → task is due

        with _fake_now(datetime(2026, 2, 17, 14, 1)):
            mod.check()

        callosum.emit.assert_called_once()
        call_kwargs = callosum.emit.call_args
        assert call_kwargs[0][0] == "supervisor"
        assert call_kwargs[0][1] == "request"
        assert call_kwargs[1]["cmd"] == ["sol", "test-task", "-v"]
        assert call_kwargs[1]["ref"].startswith("sched:a:")
        assert call_kwargs[1]["scheduler_name"] == "a"

        assert "a" not in mod._state
        assert not (journal_path / "health" / "scheduler.json").exists()

    def test_daily_boundary_submits(self, journal_path):
        """Crossing a day boundary submits due daily tasks."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "d": {"cmd": ["sol", "daily-thing"], "every": "daily"},
            },
        )

        mod.init(callosum)

        # Simulate: last check was yesterday 23:59, now it's 00:01
        mod._last_hour = datetime(2026, 2, 16, 23, 0)
        mod._last_daily_mark = datetime(2026, 2, 16, 0, 0)

        with _fake_now(datetime(2026, 2, 17, 0, 1)):
            mod.check()

        callosum.emit.assert_called_once()
        assert callosum.emit.call_args[1]["cmd"] == ["sol", "daily-thing"]

    def test_submits_on_new_hour_after_previous_run(self, journal_path):
        """Task ran in hour 14; crossing to hour 15 triggers resubmission."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "a": {"cmd": ["sol", "x"], "every": "hourly"},
            },
        )
        # Already ran at 14:02
        _write_state(
            journal_path,
            {
                "a": {"last_run": datetime(2026, 2, 17, 14, 2).timestamp()},
            },
        )

        mod.init(callosum)
        mod._last_hour = datetime(2026, 2, 17, 14, 0)
        mod._last_daily_mark = datetime(2026, 2, 17, 0, 0)

        # Cross to hour 15
        with _fake_now(datetime(2026, 2, 17, 15, 0, 1)):
            mod.check()

        # Should submit because we crossed to hour 15 and last_run was in hour 14
        callosum.emit.assert_called_once()

    def test_config_reloaded_on_boundary(self, journal_path):
        """Config file changes are picked up when a boundary is crossed."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        # Start with empty config
        _write_config(journal_path, {})
        mod.init(callosum)
        mod._last_hour = datetime(2026, 2, 17, 13, 0)
        mod._last_daily_mark = datetime(2026, 2, 17, 0, 0)

        # Now write a real config
        _write_config(
            journal_path,
            {
                "new": {"cmd": ["sol", "new-task"], "every": "hourly"},
            },
        )

        with _fake_now(datetime(2026, 2, 17, 14, 1)):
            mod.check()

        callosum.emit.assert_called_once()
        assert callosum.emit.call_args[1]["cmd"] == ["sol", "new-task"]

    def test_check_reloads_state_before_due_checks(self, journal_path):
        """State written by supervisor is reloaded before due checks."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=True)

        _write_config(
            journal_path,
            {
                "a": {"cmd": ["sol", "x"], "every": "hourly"},
            },
        )
        mod.init(callosum)
        mod._state = {}
        mod._last_hour = datetime(2026, 2, 17, 13, 0)
        mod._last_daily_mark = datetime(2026, 2, 17, 0, 0)
        _write_state(
            journal_path,
            {
                "a": {"last_run": datetime(2026, 2, 17, 14, 0, 30).timestamp()},
            },
        )

        with _fake_now(datetime(2026, 2, 17, 14, 1)):
            mod.check()

        callosum.emit.assert_not_called()
        assert "a" in mod._state

    def test_emit_failure_no_state_update(self, journal_path):
        """If emit fails, last_run should not be updated."""
        import solstone.think.scheduler as mod

        callosum = Mock()
        callosum.emit = Mock(return_value=False)

        _write_config(
            journal_path,
            {
                "a": {"cmd": ["sol", "x"], "every": "hourly"},
            },
        )

        mod.init(callosum)
        mod._last_hour = datetime(2026, 2, 17, 13, 0)
        mod._last_daily_mark = datetime(2026, 2, 17, 0, 0)

        with _fake_now(datetime(2026, 2, 17, 14, 1)):
            mod.check()

        assert mod._state.get("a") is None


# ---------------------------------------------------------------------------
# collect_status
# ---------------------------------------------------------------------------


class TestCollectStatus:
    def test_returns_entries(self, journal_path):
        import solstone.think.scheduler as mod

        mod._entries = {
            "a": {"cmd": ["sol", "x"], "every": "hourly"},
        }
        mod._state = {"a": {"last_run": time.time()}}

        status = mod.collect_status()
        assert len(status) == 1
        assert status[0]["name"] == "a"
        assert status[0]["every"] == "hourly"
        assert "last_run" in status[0]
        assert "due" in status[0]

    def test_next_run_hourly(self, journal_path):
        import solstone.think.scheduler as mod

        mod._entries = {"a": {"cmd": ["sol", "x"], "every": "hourly"}}
        mod._state = {"a": {"last_run": datetime(2026, 2, 17, 14, 5).timestamp()}}

        with _fake_now(datetime(2026, 2, 17, 14, 30)):
            status = mod.collect_status()

        expected = int(datetime(2026, 2, 17, 15, 0).timestamp() * 1000)
        assert status[0]["next_run"] == expected

    def test_next_run_daily(self, journal_path):
        import solstone.think.scheduler as mod

        mod._daily_time = "03:00"
        mod._entries = {"a": {"cmd": ["sol", "x"], "every": "daily"}}
        mod._state = {"a": {"last_run": datetime(2026, 2, 17, 3, 30).timestamp()}}

        with _fake_now(datetime(2026, 2, 17, 4, 0)):
            status = mod.collect_status()

        expected = int(datetime(2026, 2, 18, 3, 0).timestamp() * 1000)
        assert status[0]["next_run"] == expected

    def test_next_run_weekly(self, journal_path):
        import solstone.think.scheduler as mod

        mod._weekly_day = "sunday"
        mod._weekly_time = "03:00"
        mod._entries = {"a": {"cmd": ["sol", "x"], "every": "weekly"}}
        mod._state = {"a": {"last_run": datetime(2026, 3, 22, 3, 30).timestamp()}}

        with _fake_now(datetime(2026, 3, 22, 4, 0)):
            status = mod.collect_status()

        expected = int(datetime(2026, 3, 29, 3, 0).timestamp() * 1000)
        assert status[0]["next_run"] == expected

    def test_next_run_when_due(self, journal_path):
        import solstone.think.scheduler as mod

        mod._entries = {"a": {"cmd": ["sol", "x"], "every": "hourly"}}
        mod._state = {}

        with _fake_now(datetime(2026, 2, 17, 14, 30)):
            status = mod.collect_status()

        expected = int(datetime(2026, 2, 17, 14, 0).timestamp() * 1000)
        assert status[0]["next_run"] == expected


class TestHeartbeatSchedule:
    """Tests for heartbeat schedule registration and daily firing."""

    def test_register_defaults_creates_heartbeat(self, journal_path):
        """register_defaults() creates a heartbeat entry in the config file."""
        import solstone.think.scheduler as mod

        mock_cal = Mock()
        mod.init(mock_cal)
        mod.register_defaults()

        assert "heartbeat" in mod._entries
        assert mod._entries["heartbeat"]["cmd"] == ["journal", "heartbeat"]
        assert mod._entries["heartbeat"]["every"] == "daily"
        assert mod._entries["heartbeat"]["max_runtime"] == 600

        config_path = journal_path / "config" / "schedules.json"
        assert config_path.exists()
        with open(config_path) as f:
            raw = json.load(f)
        assert "heartbeat" in raw
        assert raw["heartbeat"]["cmd"] == ["journal", "heartbeat"]
        assert raw["heartbeat"]["max_runtime"] == "10m"
        assert raw["weekly-agents"]["max_runtime"] == "30m"

    def test_register_defaults_creates_providers(self, journal_path):
        """register_defaults() creates a providers health check entry."""
        import solstone.think.scheduler as mod

        mock_cal = Mock()
        mod.init(mock_cal)
        mod.register_defaults()

        config_path = journal_path / "config" / "schedules.json"
        with open(config_path) as f:
            raw = json.load(f)

        assert raw["providers"] == {
            "cmd": ["journal", "providers", "check"],
            "every": "daily",
            "enabled": True,
            "max_runtime": "5m",
        }
        assert mod._entries["providers"]["max_runtime"] == 300

    def test_register_defaults_idempotent(self, journal_path):
        """register_defaults() does not overwrite existing heartbeat config."""
        import solstone.think.scheduler as mod

        _write_config(
            journal_path,
            {
                "heartbeat": {
                    "cmd": ["journal", "heartbeat", "--custom"],
                    "every": "daily",
                    "enabled": True,
                }
            },
        )

        mock_cal = Mock()
        mod.init(mock_cal)
        mod.register_defaults()

        assert mod._entries["heartbeat"]["cmd"] == [
            "journal",
            "heartbeat",
            "--custom",
        ]
        config_path = journal_path / "config" / "schedules.json"
        with open(config_path) as f:
            raw = json.load(f)
        assert "max_runtime" not in raw["heartbeat"]

    def test_register_defaults_preserves_disabled_providers(self, journal_path):
        """register_defaults() does not overwrite disabled providers config."""
        import solstone.think.scheduler as mod

        existing = {
            "cmd": ["journal", "providers", "check", "--custom"],
            "every": "daily",
            "enabled": False,
        }
        _write_config(journal_path, {"providers": existing})

        mock_cal = Mock()
        mod.init(mock_cal)
        mod.register_defaults()

        config_path = journal_path / "config" / "schedules.json"
        with open(config_path) as f:
            raw = json.load(f)
        assert raw["providers"] == existing

    def test_register_defaults_second_call_writes_nothing(
        self, journal_path, monkeypatch
    ):
        """register_defaults() is idempotent once defaults are present."""
        import solstone.think.scheduler as mod

        mock_cal = Mock()
        mod.init(mock_cal)
        mod.register_defaults()

        monkeypatch.setattr(
            mod.tempfile,
            "mkstemp",
            lambda *args, **kwargs: pytest.fail("register_defaults rewrote config"),
        )

        mod.register_defaults()

    def test_heartbeat_is_due_when_never_run(self, journal_path):
        """_is_due returns True for heartbeat entry with no prior run."""
        import solstone.think.scheduler as mod

        entry = {"cmd": ["journal", "heartbeat"], "every": "daily", "enabled": True}
        now = datetime(2026, 3, 19, 10, 0, 0)
        assert mod._is_due(entry, None, now) is True

    def test_heartbeat_not_due_when_recently_run(self, journal_path):
        """_is_due returns False for heartbeat entry that ran after the daily mark."""
        import solstone.think.scheduler as mod

        entry = {"cmd": ["journal", "heartbeat"], "every": "daily", "enabled": True}
        now = datetime(2026, 3, 19, 10, 0, 0)
        last_run_ts = datetime(2026, 3, 19, 1, 0, 0).timestamp()
        state_entry = {"last_run": last_run_ts}
        assert mod._is_due(entry, state_entry, now) is False


# ---------------------------------------------------------------------------
# CLI main()
# ---------------------------------------------------------------------------


class TestCLI:
    def test_no_config_prints_message(self, journal_path, capsys, monkeypatch):
        monkeypatch.setattr("sys.argv", ["sol schedule"])
        from solstone.think.scheduler import main

        main()
        out = capsys.readouterr().out
        assert "No schedules configured" in out

    def test_with_config_prints_table(self, journal_path, capsys, monkeypatch):
        monkeypatch.setattr("sys.argv", ["sol schedule"])
        _write_config(
            journal_path,
            {
                "sync:plaud": {
                    "cmd": ["sol", "import", "--sync", "plaud"],
                    "every": "hourly",
                },
            },
        )

        from solstone.think.scheduler import main

        main()
        out = capsys.readouterr().out
        assert "sync:plaud" in out
        assert "hourly" in out
        assert "NAME" in out
