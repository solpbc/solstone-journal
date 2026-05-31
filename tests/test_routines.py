# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for routines — user-defined routines engine."""

import importlib
import importlib.util
from contextlib import contextmanager
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from unittest.mock import patch

import frontmatter
import pytest
import typer
from typer.testing import CliRunner

from solstone.think import routines
from solstone.think.routines import cron_matches, get_config, save_config
from solstone.think.tools.routines import app as _routines_app

runner = CliRunner()
call_app = typer.Typer()
call_app.add_typer(_routines_app, name="routines")


def _load_chat_context_module():
    """Load talent.chat_context from this worktree explicitly for tests."""
    path = (
        Path(__file__).resolve().parents[1] / "solstone" / "talent" / "chat_context.py"
    )
    spec = importlib.util.spec_from_file_location("test_chat_context", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _load_routine_context_module():
    """Load talent._routine_context from this worktree explicitly for tests."""
    module = importlib.import_module("solstone.talent._routine_context")
    return importlib.reload(module)


def _load_routines_cli_module():
    """Load think.tools.routines from this worktree explicitly for tests."""
    path = (
        Path(__file__).resolve().parents[1]
        / "solstone"
        / "think"
        / "tools"
        / "routines.py"
    )
    spec = importlib.util.spec_from_file_location("test_routines_cli", path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@contextmanager
def _fake_now(dt: datetime):
    """Temporarily replace routines.datetime with a fake that returns dt."""

    class _FakeDatetime(datetime):
        @classmethod
        def now(cls, tz=None):
            if tz is None:
                return dt
            if dt.tzinfo is None:
                return dt.replace(tzinfo=tz)
            return dt.astimezone(tz)

    routines.datetime = _FakeDatetime
    try:
        yield
    finally:
        routines.datetime = datetime


@pytest.fixture(autouse=True)
def reset_routines_state():
    """Reset routines module state between tests."""
    import solstone.think.routines as mod

    mod._config = {}
    mod._callosum = None
    mod._last_fired = {}
    mod._fired_triggers = {}
    mod._logged_unknown_cadence = set()
    yield
    mod._config = {}
    mod._callosum = None
    mod._last_fired = {}
    mod._fired_triggers = {}
    mod._logged_unknown_cadence = set()


@pytest.fixture
def journal_path(tmp_path, monkeypatch):
    """Create a temp journal with routines/ and health/ dirs."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "routines").mkdir()
    (tmp_path / "health").mkdir()
    return tmp_path


class TestCronMatches:
    def test_wildcard_all(self):
        dt = datetime(2026, 3, 15, 9, 30)
        assert cron_matches("* * * * *", dt) is True

    def test_specific_values(self):
        assert cron_matches("30 9 15 3 *", datetime(2026, 3, 15, 9, 30)) is True
        assert cron_matches("30 9 15 3 *", datetime(2026, 3, 15, 9, 31)) is False

    def test_comma_list(self):
        assert cron_matches("0,15,30,45 * * * *", datetime(2026, 3, 15, 9, 15)) is True
        assert cron_matches("0,15,30,45 * * * *", datetime(2026, 3, 15, 9, 10)) is False

    def test_range(self):
        assert cron_matches("0 9-17 * * *", datetime(2026, 3, 15, 9, 0)) is True
        assert cron_matches("0 9-17 * * *", datetime(2026, 3, 15, 18, 0)) is False

    def test_step(self):
        assert cron_matches("*/15 * * * *", datetime(2026, 3, 15, 9, 45)) is True
        assert cron_matches("*/15 * * * *", datetime(2026, 3, 15, 9, 44)) is False

    def test_range_with_step(self):
        assert cron_matches("0 1-23/2 * * *", datetime(2026, 3, 15, 9, 0)) is True
        assert cron_matches("0 1-23/2 * * *", datetime(2026, 3, 15, 10, 0)) is False

    def test_dow_sunday_zero(self):
        sunday = datetime(2026, 3, 29, 0, 0)
        assert sunday.isoweekday() == 7
        assert cron_matches("0 0 * * 0", sunday) is True

    def test_dow_sunday_seven(self):
        sunday = datetime(2026, 3, 29, 0, 0)
        assert cron_matches("0 0 * * 7", sunday) is True

    def test_dow_monday(self):
        monday = datetime(2026, 3, 30, 0, 0)
        assert monday.isoweekday() == 1
        assert cron_matches("0 0 * * 1", monday) is True

    def test_invalid_field_count(self):
        with pytest.raises(ValueError):
            cron_matches("* * * *", datetime(2026, 3, 15, 9, 0))

    def test_step_zero(self):
        with pytest.raises(ValueError):
            cron_matches("*/0 * * * *", datetime(2026, 3, 15, 9, 0))

    def test_out_of_range(self):
        with pytest.raises(ValueError):
            cron_matches("60 * * * *", datetime(2026, 3, 15, 9, 0))


class TestConfigIO:
    def test_get_config_empty(self, journal_path):
        assert get_config() == {}

    def test_save_and_get_config(self, journal_path):
        routine = {
            "abc123": {
                "id": "abc123",
                "name": "Morning",
                "instruction": "Summarize today",
                "cadence": "0 9 * * *",
                "timezone": "UTC",
                "facets": ["work"],
                "enabled": True,
                "created": "2026-03-27T00:00:00+00:00",
                "last_run": None,
                "template": None,
                "notify": False,
            }
        }
        save_config(routine)
        loaded = get_config()
        assert loaded == routine

    def test_save_config_creates_directory(self, journal_path):
        (journal_path / "routines").rmdir()
        save_config({"abc123": {"id": "abc123"}})
        assert (journal_path / "routines").exists()
        assert (journal_path / "routines" / "config.json").exists()

    def test_get_config_corrupt_json(self, journal_path):
        (journal_path / "routines" / "config.json").write_text("not json{")
        assert get_config() == {}


class TestCheck:
    def test_fires_due_routine(self, journal_path):
        import solstone.think.routines as mod

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning",
                    "instruction": "Do the thing",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )

        dt = datetime(2026, 3, 27, 9, 0, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        mock_req.assert_called_once()

    def test_skips_disabled_routine(self, journal_path):
        import solstone.think.routines as mod

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning",
                    "instruction": "Do the thing",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": False,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )

        dt = datetime(2026, 3, 27, 9, 0, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        mock_req.assert_not_called()

    def test_idempotent_same_minute(self, journal_path):
        import solstone.think.routines as mod

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning",
                    "instruction": "Do the thing",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )

        dt = datetime(2026, 3, 27, 9, 0, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()
            mod.check()

        assert mock_req.call_count == 1

    def test_fires_again_next_minute(self, journal_path):
        import solstone.think.routines as mod

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Hourly",
                    "instruction": "Do the thing",
                    "cadence": "0 * * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )

        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
        ):
            with _fake_now(datetime(2026, 3, 27, 9, 0, tzinfo=timezone.utc)):
                mod.check()
            with _fake_now(datetime(2026, 3, 27, 10, 0, tzinfo=timezone.utc)):
                mod.check()

        assert mock_req.call_count == 2


class TestCLI:
    def test_create_routine(self, journal_path):
        result = runner.invoke(
            call_app,
            [
                "routines",
                "create",
                "--name",
                "Morning review",
                "--instruction",
                "Review the day",
                "--cadence",
                "0 9 * * *",
            ],
        )
        assert result.exit_code == 0
        config = get_config()
        assert len(config) == 1
        routine = next(iter(config.values()))
        assert routine["name"] == "Morning review"

    def test_list_routines(self, journal_path):
        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning review",
                    "instruction": "Review the day",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(call_app, ["routines", "list"])
        assert result.exit_code == 0
        assert "Morning review" in result.stdout

    def test_list_empty(self, journal_path):
        result = runner.invoke(call_app, ["routines", "list"])
        assert result.exit_code == 0
        assert "No routines configured." in result.stdout


class TestTemplates:
    def test_templates_command_lists_all(self):
        result = runner.invoke(call_app, ["routines", "templates"])
        assert result.exit_code == 0
        for template_name in (
            "morning-briefing",
            "weekly-review",
            "domain-watch",
            "relationship-pulse",
            "commitment-audit",
            "monthly-patterns",
            "meeting-prep",
        ):
            assert template_name in result.stdout

    def test_template_frontmatter_valid(self):
        templates_dir = Path(__file__).resolve().parents[1] / "routines" / "templates"
        for path in sorted(templates_dir.glob("*.md")):
            post = frontmatter.load(path)
            assert post.metadata["name"]
            assert post.metadata["description"]
            assert "default_cadence" in post.metadata
            assert post.content.strip()


class TestTemplateCreate:
    def test_create_from_template(self, journal_path):
        result = runner.invoke(
            call_app,
            ["routines", "create", "--template", "morning-briefing"],
        )
        assert result.exit_code == 0
        config = get_config()
        assert len(config) == 1
        routine = next(iter(config.values()))
        assert routine["name"] == "morning-briefing"
        assert routine["cadence"] == "0 7 * * *"
        assert routine["template"] == "morning-briefing"
        assert "daily morning briefing" in routine["instruction"].lower()

    def test_create_template_with_overrides(self, journal_path):
        result = runner.invoke(
            call_app,
            [
                "routines",
                "create",
                "--template",
                "morning-briefing",
                "--cadence",
                "0 8 * * *",
                "--name",
                "My Briefing",
            ],
        )
        assert result.exit_code == 0
        config = get_config()
        routine = next(iter(config.values()))
        assert routine["name"] == "My Briefing"
        assert routine["cadence"] == "0 8 * * *"
        assert routine["template"] == "morning-briefing"

    def test_create_template_not_found(self, journal_path):
        result = runner.invoke(
            call_app,
            ["routines", "create", "--template", "nonexistent"],
        )
        assert result.exit_code == 1
        assert "template 'nonexistent' not found" in result.stderr

    def test_create_template_dict_cadence_persisted(self, journal_path):
        result = runner.invoke(
            call_app,
            ["routines", "create", "--template", "meeting-prep"],
        )
        assert result.exit_code == 0
        config = get_config()
        assert len(config) == 1
        routine = next(iter(config.values()))
        assert routine["cadence"] == {
            "type": "activity-anticipation",
            "offset_minutes": -30,
        }

    def test_create_template_dict_cadence_overridden_by_string(self, journal_path):
        result = runner.invoke(
            call_app,
            [
                "routines",
                "create",
                "--template",
                "meeting-prep",
                "--cadence",
                "0 9 * * *",
            ],
        )
        assert result.exit_code == 0
        config = get_config()
        assert len(config) == 1
        routine = next(iter(config.values()))
        assert routine["cadence"] == "0 9 * * *"

    def test_create_invalid_template_cadence_type(self, journal_path, monkeypatch):
        import solstone.think.tools.routines as routines_cli

        def _fake_template(name: str):
            return (
                {
                    "name": name,
                    "description": "bad template",
                    "default_cadence": {
                        "type": "event",
                        "trigger": "wrong",
                        "offset_minutes": -30,
                    },
                    "default_timezone": "UTC",
                    "default_facets": [],
                },
                "Instruction body",
            )

        monkeypatch.setattr(routines_cli, "_load_template", _fake_template)
        result = runner.invoke(
            call_app,
            ["routines", "create", "--template", "bad-template"],
        )
        assert result.exit_code == 1
        assert "unsupported cadence type" in result.stderr

    def test_create_template_dict_cadence_missing_type(self, journal_path, monkeypatch):
        import solstone.think.tools.routines as routines_cli

        def _fake_template(name: str):
            return (
                {
                    "name": name,
                    "description": "missing cadence type",
                    "default_cadence": {"offset_minutes": -30},
                    "default_timezone": "UTC",
                    "default_facets": [],
                },
                "Instruction body",
            )

        monkeypatch.setattr(routines_cli, "_load_template", _fake_template)
        result = runner.invoke(
            call_app,
            ["routines", "create", "--template", "bad-template"],
        )
        assert result.exit_code == 1
        assert "type" in result.stderr
        assert "missing" in result.stderr

    def test_create_template_dict_cadence_bad_offset_minutes(
        self, journal_path, monkeypatch
    ):
        import solstone.think.tools.routines as routines_cli

        def _fake_template(name: str):
            return (
                {
                    "name": name,
                    "description": "bad cadence offset",
                    "default_cadence": {
                        "type": "activity-anticipation",
                        "offset_minutes": "not-a-number",
                    },
                    "default_timezone": "UTC",
                    "default_facets": [],
                },
                "Instruction body",
            )

        monkeypatch.setattr(routines_cli, "_load_template", _fake_template)
        result = runner.invoke(
            call_app,
            ["routines", "create", "--template", "bad-template"],
        )
        assert result.exit_code == 1
        assert "offset_minutes" in result.stderr


class TestNameResolution:
    def test_resolve_by_name(self, journal_path):
        save_config(
            {
                "abc-123-def": {
                    "id": "abc-123-def",
                    "name": "Morning Briefing",
                    "instruction": "Brief me",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(
            call_app, ["routines", "edit", "Morning Briefing", "--name", "Updated"]
        )
        assert result.exit_code == 0
        config = get_config()
        assert config["abc-123-def"]["name"] == "Updated"

    def test_resolve_by_name_case_insensitive(self, journal_path):
        save_config(
            {
                "abc-123-def": {
                    "id": "abc-123-def",
                    "name": "Morning Briefing",
                    "instruction": "Brief me",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(
            call_app, ["routines", "edit", "morning briefing", "--name", "Updated"]
        )
        assert result.exit_code == 0
        config = get_config()
        assert config["abc-123-def"]["name"] == "Updated"

    def test_resolve_name_ambiguous(self, journal_path):
        save_config(
            {
                "abc-123": {
                    "id": "abc-123",
                    "name": "Daily",
                    "instruction": "a",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                },
                "def-456": {
                    "id": "def-456",
                    "name": "Daily",
                    "instruction": "b",
                    "cadence": "0 10 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                },
            }
        )
        result = runner.invoke(call_app, ["routines", "edit", "Daily", "--name", "X"])
        assert result.exit_code == 1
        assert "ambiguous" in result.stderr.lower()

    def test_meta_excluded_from_resolve(self, journal_path):
        save_config(
            {
                "_meta": {"suggestions_enabled": True},
                "abc-123": {
                    "id": "abc-123",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                },
            }
        )
        result = runner.invoke(
            call_app, ["routines", "edit", "abc", "--name", "Updated"]
        )
        assert result.exit_code == 0


class TestResumeDate:
    def test_edit_resume_date(self, journal_path):
        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": False,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(
            call_app,
            [
                "routines",
                "edit",
                "routine-1",
                "--enabled",
                "false",
                "--resume-date",
                "2026-04-01",
            ],
        )
        assert result.exit_code == 0
        config = get_config()
        assert config["routine-1"]["resume_date"] == "2026-04-01"

    def test_enable_clears_resume_date(self, journal_path):
        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": False,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                    "resume_date": "2026-04-01",
                }
            }
        )
        result = runner.invoke(
            call_app, ["routines", "edit", "routine-1", "--enabled", "true"]
        )
        assert result.exit_code == 0
        config = get_config()
        assert config["routine-1"]["enabled"] is True
        assert "resume_date" not in config["routine-1"]

    def test_auto_resume(self, journal_path):
        import solstone.think.routines as mod

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": False,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                    "resume_date": "2026-03-27",
                }
            }
        )

        dt = datetime(2026, 3, 27, 10, 0, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ),
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        config = get_config()
        assert config["routine-1"]["enabled"] is True
        assert "resume_date" not in config["routine-1"]
        health_log = (journal_path / "health" / "routines.log").read_text()
        assert "auto-resumed" in health_log

    def test_auto_resume_future_date_not_resumed(self, journal_path):
        import solstone.think.routines as mod

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": False,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                    "resume_date": "2026-04-01",
                }
            }
        )

        dt = datetime(2026, 3, 27, 10, 0, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ),
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        config = get_config()
        assert config["routine-1"]["enabled"] is False
        assert config["routine-1"]["resume_date"] == "2026-04-01"

    def test_resume_date_invalid_format(self, journal_path):
        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": False,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(
            call_app, ["routines", "edit", "routine-1", "--resume-date", "not-a-date"]
        )
        assert result.exit_code == 1
        assert "YYYY-MM-DD" in result.stderr


class TestOutputByDate:
    def test_output_specific_date(self, journal_path):
        output_dir = journal_path / "routines" / "routine-1"
        output_dir.mkdir(parents=True)
        (output_dir / "20260325.md").write_text("March 25 output", encoding="utf-8")
        (output_dir / "20260326.md").write_text("March 26 output", encoding="utf-8")

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(
            call_app, ["routines", "output", "routine-1", "--date", "2026-03-25"]
        )
        assert result.exit_code == 0
        assert "March 25 output" in result.stdout

    def test_output_date_missing(self, journal_path):
        output_dir = journal_path / "routines" / "routine-1"
        output_dir.mkdir(parents=True)
        (output_dir / "20260325.md").write_text("content", encoding="utf-8")

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(
            call_app, ["routines", "output", "routine-1", "--date", "2026-03-27"]
        )
        assert result.exit_code == 0
        assert "No output for that date" in result.stdout

    def test_output_date_collision_file(self, journal_path):
        output_dir = journal_path / "routines" / "routine-1"
        output_dir.mkdir(parents=True)
        (output_dir / "20260325.md").write_text("first run", encoding="utf-8")
        (output_dir / "20260325-093000.md").write_text("second run", encoding="utf-8")

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(
            call_app, ["routines", "output", "routine-1", "--date", "2026-03-25"]
        )
        assert result.exit_code == 0
        assert "second run" in result.stdout

    def test_output_default_no_date(self, journal_path):
        output_dir = journal_path / "routines" / "routine-1"
        output_dir.mkdir(parents=True)
        (output_dir / "20260325.md").write_text("old", encoding="utf-8")
        (output_dir / "20260326.md").write_text("latest", encoding="utf-8")

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                }
            }
        )
        result = runner.invoke(call_app, ["routines", "output", "routine-1"])
        assert result.exit_code == 0
        assert "latest" in result.stdout


class TestSuggestions:
    def test_suggestions_read_default(self, journal_path):
        save_config({})
        result = runner.invoke(call_app, ["routines", "suggestions"])
        assert result.exit_code == 0
        assert "enabled" in result.stdout

    def test_suggestions_disable(self, journal_path):
        save_config({})
        result = runner.invoke(call_app, ["routines", "suggestions", "--disable"])
        assert result.exit_code == 0
        assert "disabled" in result.stdout
        config = get_config()
        assert config["_meta"]["suggestions_enabled"] is False

    def test_suggestions_enable(self, journal_path):
        save_config({"_meta": {"suggestions_enabled": False}})
        result = runner.invoke(call_app, ["routines", "suggestions", "--enable"])
        assert result.exit_code == 0
        assert "enabled" in result.stdout
        config = get_config()
        assert config["_meta"]["suggestions_enabled"] is True


class TestTriggerCounting:
    """Test trigger counting for progressive discovery."""

    def test_morning_briefing_triggers(self, journal_path):
        """Calendar queries increment morning-briefing trigger count."""
        module = _load_chat_context_module()
        config = {"_meta": {"suggestions": {}}}

        module._count_triggers("what's on my calendar today", None, config)
        module._count_triggers("show me my schedule", None, config)
        module._count_triggers("what's my agenda", None, config)

        entry = config["_meta"]["suggestions"]["morning-briefing"]
        assert entry["trigger_count"] == 3
        assert entry["first_trigger"] is not None
        assert entry["last_trigger"] is not None

    def test_relationship_pulse_triggers(self, journal_path):
        """Relationship queries increment relationship-pulse trigger count."""
        module = _load_chat_context_module()
        config = {"_meta": {"suggestions": {}}}

        module._count_triggers("who haven't i talked to recently", None, config)
        module._count_triggers("when did i last talk to Sarah", None, config)

        entry = config["_meta"]["suggestions"]["relationship-pulse"]
        assert entry["trigger_count"] == 2

    def test_commitment_audit_triggers(self, journal_path):
        """Commitment queries increment commitment-audit trigger count."""
        module = _load_chat_context_module()
        config = {"_meta": {"suggestions": {}}}

        module._count_triggers("do I have any overdue follow-ups", None, config)
        module._count_triggers("what commitments have I dropped", None, config)

        entry = config["_meta"]["suggestions"]["commitment-audit"]
        assert entry["trigger_count"] == 2

    def test_domain_watch_requires_facet(self, journal_path):
        """domain-watch triggers only count when facet is present."""
        module = _load_chat_context_module()
        config = {"_meta": {"suggestions": {}}}

        module._count_triggers("track this trend over time", None, config)
        assert "domain-watch" not in config["_meta"]["suggestions"]

        module._count_triggers("track this trend over time", "work", config)
        entry = config["_meta"]["suggestions"]["domain-watch"]
        assert entry["trigger_count"] == 1
        assert entry["trigger_data"]["topics"]["work"] == [date.today().isoformat()]

    def test_domain_watch_dedupes_same_day(self, journal_path):
        """domain-watch only counts distinct dates per topic."""
        module = _load_chat_context_module()
        config = {"_meta": {"suggestions": {}}}

        module._count_triggers("track trends lately", "work", config)
        changed = module._count_triggers("watch these trends", "work", config)
        assert not changed

        entry = config["_meta"]["suggestions"]["domain-watch"]
        assert entry["trigger_count"] == 1

    def test_no_match_no_mutation(self, journal_path):
        """Messages that don't match any pattern don't mutate config."""
        module = _load_chat_context_module()
        config = {"_meta": {"suggestions": {}}}

        changed = module._count_triggers("hello how are you", None, config)
        assert not changed
        assert config["_meta"]["suggestions"] == {}

    def test_write_avoidance(self, journal_path):
        """_count_triggers returns False when no triggers matched."""
        module = _load_chat_context_module()
        config = {"_meta": {"suggestions": {}}}

        assert module._count_triggers("just chatting", None, config) is False
        assert module._count_triggers("what's on my calendar", None, config) is True


class TestEligibilityGates:
    """Test the 5-gate eligibility chain for routine suggestions."""

    def test_suggestions_disabled_blocks(self, journal_path):
        """Gate 1: suggestions_enabled=False blocks all suggestions."""
        module = _load_routine_context_module()
        routines_config = {
            "_meta": {
                "suggestions_enabled": False,
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 5,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    }
                },
            }
        }
        journal_config = {"agent": {"name_status": "chosen"}}
        assert module.get_eligible_suggestion(routines_config, journal_config) is None

    def test_naming_default_blocks(self, journal_path):
        """Gate 2: name_status='default' blocks all suggestions."""
        module = _load_routine_context_module()
        routines_config = {
            "_meta": {
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 5,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    }
                }
            }
        }
        journal_config = {"agent": {"name_status": "default"}}
        assert module.get_eligible_suggestion(routines_config, journal_config) is None

    def test_active_routine_blocks(self, journal_path):
        """Gate 3: existing routine with same template blocks suggestion."""
        module = _load_routine_context_module()
        routines_config = {
            "routine-1": {
                "id": "routine-1",
                "name": "Morning Briefing",
                "template": "morning-briefing",
            },
            "_meta": {
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 5,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    }
                }
            },
        }
        journal_config = {"agent": {"name_status": "chosen"}}
        assert module.get_eligible_suggestion(routines_config, journal_config) is None

    def test_declined_blocks(self, journal_path):
        """Gate 4: declined response blocks suggestion for that template."""
        module = _load_routine_context_module()
        routines_config = {
            "_meta": {
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 5,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": "declined",
                        "suggested": True,
                    }
                }
            }
        }
        journal_config = {"agent": {"name_status": "chosen"}}
        assert module.get_eligible_suggestion(routines_config, journal_config) is None

    def test_cooldown_blocks(self, journal_path):
        """Gate 5: suggestion within last 7 days blocks all."""
        module = _load_routine_context_module()
        yesterday = (date.today() - timedelta(days=1)).isoformat()
        routines_config = {
            "_meta": {
                "last_suggestion_date": yesterday,
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 5,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    }
                },
            }
        }
        journal_config = {"agent": {"name_status": "chosen"}}
        assert module.get_eligible_suggestion(routines_config, journal_config) is None

    def test_all_gates_pass(self, journal_path):
        """When all gates pass and threshold met, returns suggestion."""
        module = _load_routine_context_module()
        routines_config = {
            "_meta": {
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 3,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    }
                }
            }
        }
        journal_config = {"agent": {"name_status": "chosen"}}
        result = module.get_eligible_suggestion(routines_config, journal_config)
        assert result is not None
        assert result["template_name"] == "morning-briefing"
        assert result["trigger_count"] == 3

    def test_below_threshold_no_suggestion(self, journal_path):
        """Trigger count below threshold does not produce a suggestion."""
        module = _load_routine_context_module()
        routines_config = {
            "_meta": {
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 2,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    }
                }
            }
        }
        journal_config = {"agent": {"name_status": "chosen"}}
        assert module.get_eligible_suggestion(routines_config, journal_config) is None

    def test_highest_trigger_count_wins(self, journal_path):
        """When multiple templates eligible, highest trigger_count wins."""
        module = _load_routine_context_module()
        routines_config = {
            "_meta": {
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 3,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    },
                    "meeting-prep": {
                        "trigger_count": 5,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    },
                }
            }
        }
        journal_config = {"agent": {"name_status": "chosen"}}
        result = module.get_eligible_suggestion(routines_config, journal_config)
        assert result["template_name"] == "meeting-prep"
        assert result["trigger_count"] == 5

    def test_cooldown_expired_allows(self, journal_path):
        """Cooldown older than 7 days allows suggestions."""
        module = _load_routine_context_module()
        old_date = (date.today() - timedelta(days=8)).isoformat()
        routines_config = {
            "_meta": {
                "last_suggestion_date": old_date,
                "suggestions": {
                    "morning-briefing": {
                        "trigger_count": 3,
                        "first_trigger": "2026-03-01",
                        "last_trigger": "2026-03-27",
                        "trigger_data": {},
                        "response": None,
                        "suggested": False,
                    }
                },
            }
        }
        journal_config = {"agent": {"name_status": "chosen"}}
        result = module.get_eligible_suggestion(routines_config, journal_config)
        assert result is not None


class TestSuggestRespond:
    """Test suggest-respond and suggest-state CLI commands."""

    def test_suggest_respond_accepted(self, journal_path):
        module = _load_routines_cli_module()
        save_config(
            {
                "_meta": {
                    "suggestions": {
                        "morning-briefing": {
                            "trigger_count": 3,
                            "first_trigger": "2026-03-01",
                            "last_trigger": "2026-03-27",
                            "trigger_data": {},
                            "response": None,
                            "suggested": False,
                        }
                    }
                }
            }
        )
        result = runner.invoke(
            module.app,
            ["suggest-respond", "morning-briefing", "--accepted"],
        )
        assert result.exit_code == 0
        assert "accepted" in result.output

        config = get_config()
        entry = config["_meta"]["suggestions"]["morning-briefing"]
        assert entry["response"] == "accepted"
        assert entry["suggested"] is True
        assert config["_meta"]["last_suggestion_date"] is not None

    def test_suggest_respond_declined(self, journal_path):
        module = _load_routines_cli_module()
        save_config(
            {
                "_meta": {
                    "suggestions": {
                        "morning-briefing": {
                            "trigger_count": 3,
                            "first_trigger": "2026-03-01",
                            "last_trigger": "2026-03-27",
                            "trigger_data": {},
                            "response": None,
                            "suggested": False,
                        }
                    }
                }
            }
        )
        result = runner.invoke(
            module.app,
            ["suggest-respond", "morning-briefing", "--declined"],
        )
        assert result.exit_code == 0
        assert "declined" in result.output

        config = get_config()
        entry = config["_meta"]["suggestions"]["morning-briefing"]
        assert entry["response"] == "declined"

    def test_suggest_respond_no_flags_fails(self, journal_path):
        module = _load_routines_cli_module()
        save_config(
            {
                "_meta": {
                    "suggestions": {
                        "morning-briefing": {
                            "trigger_count": 3,
                            "response": None,
                            "suggested": False,
                        }
                    }
                }
            }
        )
        result = runner.invoke(
            module.app,
            ["suggest-respond", "morning-briefing"],
        )
        assert result.exit_code == 1

    def test_suggest_respond_both_flags_fails(self, journal_path):
        module = _load_routines_cli_module()
        save_config(
            {
                "_meta": {
                    "suggestions": {
                        "morning-briefing": {
                            "trigger_count": 3,
                            "response": None,
                            "suggested": False,
                        }
                    }
                }
            }
        )
        result = runner.invoke(
            module.app,
            [
                "suggest-respond",
                "morning-briefing",
                "--accepted",
                "--declined",
            ],
        )
        assert result.exit_code == 1

    def test_suggest_respond_unknown_template_fails(self, journal_path):
        module = _load_routines_cli_module()
        save_config({"_meta": {"suggestions": {}}})
        result = runner.invoke(
            module.app,
            ["suggest-respond", "nonexistent", "--accepted"],
        )
        assert result.exit_code == 1

    def test_suggest_state(self, journal_path):
        module = _load_routines_cli_module()
        save_config(
            {
                "_meta": {
                    "suggestions": {
                        "morning-briefing": {
                            "trigger_count": 3,
                            "response": "accepted",
                        }
                    }
                }
            }
        )
        result = runner.invoke(module.app, ["suggest-state"])
        assert result.exit_code == 0
        data = __import__("json").loads(result.output)
        assert "morning-briefing" in data
        assert data["morning-briefing"]["response"] == "accepted"


class TestGetRoutineState:
    def test_basic_structure(self, journal_path):
        from solstone.think.routines import get_routine_state

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "last_run": None,
                }
            }
        )
        state = get_routine_state()
        assert len(state) == 1
        assert state[0]["name"] == "Morning"
        assert state[0]["cadence"] == "0 9 * * *"
        assert state[0]["enabled"] is True
        assert state[0]["output_summary"] is None

    def test_recent_output_summary(self, journal_path):
        from solstone.think.routines import get_routine_state

        last_run = datetime(2026, 3, 27, 9, 0, tzinfo=timezone.utc).isoformat()
        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "last_run": last_run,
                }
            }
        )
        output_dir = journal_path / "routines" / "routine-1"
        output_dir.mkdir(parents=True)
        (output_dir / "20260327.md").write_text(
            "Here is the morning briefing summary for today.", encoding="utf-8"
        )

        dt = datetime(2026, 3, 27, 10, 0, tzinfo=timezone.utc)
        with _fake_now(dt):
            state = get_routine_state()

        assert len(state) == 1
        assert state[0]["output_summary"] is not None
        assert "morning briefing" in state[0]["output_summary"]

    def test_meta_excluded(self, journal_path):
        from solstone.think.routines import get_routine_state

        save_config(
            {
                "_meta": {"suggestions_enabled": True},
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "last_run": None,
                },
            }
        )
        state = get_routine_state()
        assert len(state) == 1
        assert state[0]["name"] == "Morning"

    def test_paused_until(self, journal_path):
        from solstone.think.routines import get_routine_state

        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": False,
                    "facets": [],
                    "last_run": None,
                    "resume_date": "2026-04-01",
                }
            }
        )
        state = get_routine_state()
        assert state[0]["paused_until"] == "2026-04-01"
        assert state[0]["enabled"] is False


class TestMetaFiltering:
    def test_list_excludes_meta(self, journal_path):
        save_config(
            {
                "_meta": {"suggestions_enabled": True},
                "routine-1": {
                    "id": "routine-1",
                    "name": "Test",
                    "instruction": "test",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                },
            }
        )
        result = runner.invoke(call_app, ["routines", "list"])
        assert result.exit_code == 0
        assert "Test" in result.stdout
        assert "_meta" not in result.stdout
        assert "suggestions" not in result.stdout

    def test_check_skips_meta(self, journal_path):
        import solstone.think.routines as mod

        save_config(
            {
                "_meta": {"suggestions_enabled": True},
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning",
                    "instruction": "Do the thing",
                    "cadence": "0 9 * * *",
                    "timezone": "UTC",
                    "enabled": True,
                    "facets": [],
                    "template": None,
                    "notify": False,
                    "last_run": None,
                },
            }
        )

        dt = datetime(2026, 3, 27, 9, 0, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        mock_req.assert_called_once()


class TestDeleteSuggestionReset:
    """Test that delete resets accepted suggestion state but preserves declined."""

    def test_delete_resets_accepted_suggestion(self, journal_path):
        module = _load_routines_cli_module()
        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning Briefing",
                    "template": "morning-briefing",
                    "cadence": "0 7 * * *",
                    "enabled": True,
                },
                "_meta": {
                    "suggestions": {
                        "morning-briefing": {
                            "trigger_count": 5,
                            "first_trigger": "2026-03-01",
                            "last_trigger": "2026-03-27",
                            "trigger_data": {},
                            "response": "accepted",
                            "suggested": True,
                        }
                    }
                },
            }
        )
        result = runner.invoke(module.app, ["delete", "routine-1"])
        assert result.exit_code == 0

        config = get_config()
        entry = config["_meta"]["suggestions"]["morning-briefing"]
        assert entry["trigger_count"] == 0
        assert entry["response"] is None
        assert entry["suggested"] is False
        assert entry["first_trigger"] is None

    def test_delete_preserves_declined_suggestion(self, journal_path):
        module = _load_routines_cli_module()
        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Morning Briefing",
                    "template": "morning-briefing",
                    "cadence": "0 7 * * *",
                    "enabled": True,
                },
                "_meta": {
                    "suggestions": {
                        "morning-briefing": {
                            "trigger_count": 5,
                            "first_trigger": "2026-03-01",
                            "last_trigger": "2026-03-27",
                            "trigger_data": {},
                            "response": "declined",
                            "suggested": True,
                        }
                    }
                },
            }
        )
        result = runner.invoke(module.app, ["delete", "routine-1"])
        assert result.exit_code == 0

        config = get_config()
        entry = config["_meta"]["suggestions"]["morning-briefing"]
        assert entry["response"] == "declined"
        assert entry["trigger_count"] == 5

    def test_delete_no_template_no_reset(self, journal_path):
        """Routines without a template field don't touch suggestion state."""
        module = _load_routines_cli_module()
        save_config(
            {
                "routine-1": {
                    "id": "routine-1",
                    "name": "Custom Routine",
                    "cadence": "0 7 * * *",
                    "enabled": True,
                },
                "_meta": {
                    "suggestions": {
                        "morning-briefing": {
                            "trigger_count": 5,
                            "response": "accepted",
                            "suggested": True,
                        }
                    }
                },
            }
        )
        result = runner.invoke(module.app, ["delete", "routine-1"])
        assert result.exit_code == 0

        config = get_config()
        entry = config["_meta"]["suggestions"]["morning-briefing"]
        assert entry["response"] == "accepted"
        assert entry["trigger_count"] == 5


class TestActivityAnticipation:
    @staticmethod
    def _make_routine(routine_id: str, offset_minutes: int) -> dict:
        return {
            "id": routine_id,
            "name": "Meeting prep",
            "instruction": "Prepare for the upcoming activity.",
            "cadence": {
                "type": "activity-anticipation",
                "offset_minutes": offset_minutes,
            },
            "timezone": "UTC",
            "enabled": True,
            "facets": [],
            "template": None,
            "notify": False,
            "last_run": None,
        }

    @staticmethod
    def _make_anticipated_record(
        activity_id: str,
        start: str,
        title: str = "Sync",
        description: str = "Discuss current status.",
        participation=None,
        *,
        facet: str = "work",
    ) -> dict:
        return {
            "id": activity_id,
            "activity": "meeting",
            "target_date": "2026-04-18",
            "start": start,
            "end": "10:30:00",
            "title": title,
            "description": description,
            "details": "Review open items.",
            "facet": facet,
            "source": "anticipated",
            "participation": participation or [],
            "hidden": False,
        }

    @staticmethod
    def _seed_activity_record(facet: str, day: str, record: dict) -> None:
        from solstone.think.activities import append_activity_record
        from solstone.think.facets import create_facet

        title = " ".join(part.capitalize() for part in facet.split("-"))
        slug = create_facet(title)
        assert slug == facet
        written = append_activity_record(facet, day, record)
        assert written is True

    def test_dispatch_fires_and_injects_prompt(self, journal_path):
        import solstone.think.routines as mod

        save_config({"routine-1": self._make_routine("routine-1", -30)})
        self._seed_activity_record(
            "work",
            "20260418",
            self._make_anticipated_record(
                "anticipated_meeting_100000_0418",
                "10:00:00",
                title="Roadmap Sync",
                description="Discuss Q2 roadmap.",
                participation=[
                    {"role": "attendee", "name": "Alex Rivera"},
                    {"role": "attendee", "name": "Jordan Lee"},
                    {"role": "organizer", "name": "Morgan Shaw"},
                ],
            ),
        )

        dt = datetime(2026, 4, 18, 9, 30, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        mock_req.assert_called_once()
        prompt = mock_req.call_args.kwargs["prompt"]
        assert prompt.index("## Upcoming Activity") < prompt.index(
            "Execute this routine now."
        )
        assert "Roadmap Sync" in prompt
        assert "10:00:00" in prompt
        assert "Discuss Q2 roadmap." in prompt
        assert "Alex Rivera" in prompt
        assert "Jordan Lee" in prompt
        assert "Morgan Shaw" not in prompt

    def test_same_minute_fires_only_once(self, journal_path):
        import solstone.think.routines as mod

        save_config({"routine-1": self._make_routine("routine-1", -30)})
        self._seed_activity_record(
            "work",
            "20260418",
            self._make_anticipated_record(
                "anticipated_meeting_100000_0418",
                "10:00:00",
            ),
        )

        dt = datetime(2026, 4, 18, 9, 30, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()
            mod.check()

        assert mock_req.call_count == 1

    def test_hidden_records_are_skipped(self, journal_path):
        import solstone.think.routines as mod

        save_config({"routine-1": self._make_routine("routine-1", -30)})
        record = self._make_anticipated_record(
            "anticipated_meeting_100000_0418",
            "10:00:00",
        )
        record["hidden"] = True
        self._seed_activity_record("work", "20260418", record)

        dt = datetime(2026, 4, 18, 9, 30, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        mock_req.assert_not_called()

    def test_non_anticipated_records_are_skipped(self, journal_path):
        import solstone.think.routines as mod

        save_config({"routine-1": self._make_routine("routine-1", -30)})
        record = self._make_anticipated_record(
            "anticipated_meeting_100000_0418",
            "10:00:00",
        )
        record["source"] = "completed"
        self._seed_activity_record("work", "20260418", record)

        dt = datetime(2026, 4, 18, 9, 30, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        mock_req.assert_not_called()

    def test_late_evening_fires_for_next_day_activity(self, journal_path):
        """Pre-alert for a 00:15 activity on D+1 must fire at 23:45 on D."""
        import solstone.think.routines as mod

        save_config({"routine-1": self._make_routine("routine-1", -30)})
        record = self._make_anticipated_record(
            "anticipated_meeting_001500_0419",
            "00:15",
        )
        record["target_date"] = "2026-04-19"
        self._seed_activity_record("work", "20260419", record)

        dt = datetime(2026, 4, 18, 23, 45, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()
            mod.check()

        assert mock_req.call_count == 1

    def test_early_morning_fires_for_previous_day_activity(self, journal_path):
        """Post-start anticipation for a 23:45 activity on D-1 fires at 00:15 on D."""
        import solstone.think.routines as mod

        save_config({"routine-1": self._make_routine("routine-1", 30)})
        record = self._make_anticipated_record(
            "anticipated_meeting_234500_0417",
            "23:45",
        )
        record["target_date"] = "2026-04-17"
        self._seed_activity_record("work", "20260417", record)

        dt = datetime(2026, 4, 18, 0, 15, tzinfo=timezone.utc)
        with (
            patch(
                "solstone.think.routines.cortex_request", return_value="fake_agent_id"
            ) as mock_req,
            patch(
                "solstone.think.routines.wait_for_uses",
                return_value=({"fake_agent_id": "finish"}, []),
            ),
            patch("solstone.think.routines.callosum_send", return_value=True),
            _fake_now(dt),
        ):
            mod.check()

        assert mock_req.call_count == 1
