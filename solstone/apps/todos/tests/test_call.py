# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for todos CLI commands (sol call todos ...)."""

import json
from datetime import datetime

from typer.testing import CliRunner

import solstone.apps.todos.call as todos_call
from solstone.think.call import call_app

runner = CliRunner()


class TestTodosList:
    """Tests for 'sol call todos list' command."""

    def test_list_with_facet(self, todo_env):
        """List todos for a single day with --facet."""
        todo_env(
            [{"text": "Buy milk"}, {"text": "Walk dog", "completed": True}],
            day="20240101",
        )
        result = runner.invoke(
            call_app, ["todos", "list", "20240101", "--facet", "personal"]
        )
        assert result.exit_code == 0
        assert "Buy milk" in result.output
        assert "Walk dog" in result.output

    def test_list_all_facets(self, todo_env):
        """List todos across all facets when --facet is omitted."""
        todo_env([{"text": "Work task"}], day="20240101", facet="work")
        # Add a second facet's todos in the same journal
        todo_env([{"text": "Home task"}], day="20240101", facet="home")
        result = runner.invoke(call_app, ["todos", "list", "20240101"])
        assert result.exit_code == 0
        assert "Work task" in result.output
        assert "Home task" in result.output

    def test_list_empty_day(self, todo_env):
        """Empty day shows no-todos message."""
        todo_env([], day="20240101")
        result = runner.invoke(
            call_app, ["todos", "list", "20240101", "--facet", "personal"]
        )
        assert result.exit_code == 0
        assert "No todos" in result.output

    def test_list_invalid_range(self, todo_env):
        """--to before day produces an error."""
        todo_env([], day="20240101")
        result = runner.invoke(
            call_app,
            ["todos", "list", "20240201", "--facet", "personal", "--to", "20240101"],
        )
        assert result.exit_code == 1
        assert "Error" in result.output

    def test_list_defaults_to_today_without_day_or_env(self, todo_env, monkeypatch):
        today = datetime.now().strftime("%Y%m%d")
        todo_env([{"text": "Today task"}], day=today)
        monkeypatch.delenv("SOL_DAY", raising=False)

        result = runner.invoke(call_app, ["todos", "list", "--facet", "personal"])

        assert result.exit_code == 0
        assert "Today task" in result.output


class TestTodosAdd:
    """Tests for 'sol call todos add' command."""

    def test_add_todo(self, todo_env):
        """Add a todo to a future day."""
        todo_env([], day="29991231")
        result = runner.invoke(
            call_app,
            [
                "todos",
                "add",
                "Ship feature",
                "--day",
                "29991231",
                "--facet",
                "personal",
            ],
        )
        assert result.exit_code == 0
        assert "Ship feature" in result.output

    def test_add_appends_to_existing(self, todo_env):
        """Add appends after existing items."""
        todo_env([{"text": "First"}], day="29991231")
        result = runner.invoke(
            call_app,
            ["todos", "add", "Second", "--day", "29991231", "--facet", "personal"],
        )
        assert result.exit_code == 0
        assert "First" in result.output
        assert "Second" in result.output

    def test_add_past_date_allowed(self, todo_env):
        """Adding to a past date succeeds."""
        todo_env([], day="20200101")
        result = runner.invoke(
            call_app,
            ["todos", "add", "Nope", "--day", "20200101", "--facet", "personal"],
        )
        assert result.exit_code == 0
        assert "Nope" in result.output

    def test_add_empty_text_rejected(self, todo_env):
        """Adding empty text fails."""
        todo_env([], day="29991231")
        result = runner.invoke(
            call_app,
            ["todos", "add", "   ", "--day", "29991231", "--facet", "personal"],
        )
        assert result.exit_code == 1

    def test_add_with_nudge(self, todo_env):
        """Add a todo with --nudge flag."""
        todo_env(day="20260301", facet="personal")
        result = runner.invoke(
            call_app,
            [
                "todos",
                "add",
                "Test nudge",
                "--nudge",
                "15:00",
                "-f",
                "personal",
                "-d",
                "20260301",
            ],
        )
        assert result.exit_code == 0
        assert "Test nudge" in result.output


class TestTodosDone:
    """Tests for 'sol call todos done' command."""

    def test_done_marks_complete(self, todo_env):
        """Mark a todo as done."""
        todo_env([{"text": "Buy milk"}], day="20240101")
        result = runner.invoke(
            call_app, ["todos", "done", "1", "--day", "20240101", "--facet", "personal"]
        )
        assert result.exit_code == 0
        assert "[x]" in result.output

    def test_done_invalid_line_number(self, todo_env):
        """Invalid line number fails."""
        todo_env([{"text": "Only one"}], day="20240101")
        result = runner.invoke(
            call_app, ["todos", "done", "5", "--day", "20240101", "--facet", "personal"]
        )
        assert result.exit_code == 1


class TestTodosCancel:
    """Tests for 'sol call todos cancel' command."""

    def test_cancel_entry(self, todo_env):
        """Cancel a todo."""
        todo_env([{"text": "Buy milk"}], day="20240101")
        result = runner.invoke(
            call_app,
            ["todos", "cancel", "1", "--day", "20240101", "--facet", "personal"],
        )
        assert result.exit_code == 0
        assert "cancelled" in result.output

    def test_cancel_invalid_line_number(self, todo_env):
        """Invalid line number fails."""
        todo_env([{"text": "Only one"}], day="20240101")
        result = runner.invoke(
            call_app,
            ["todos", "cancel", "5", "--day", "20240101", "--facet", "personal"],
        )
        assert result.exit_code == 1


class TestTodosUpcoming:
    """Tests for 'sol call todos upcoming' command."""

    def test_upcoming_shows_future(self, todo_env):
        """Upcoming shows future todos."""
        todo_env([{"text": "Future task"}], day="29991231")
        result = runner.invoke(call_app, ["todos", "upcoming"])
        assert result.exit_code == 0
        assert "Future task" in result.output

    def test_upcoming_with_facet_filter(self, todo_env):
        """Upcoming filters by facet."""
        todo_env([{"text": "Work task"}], day="29991231", facet="work")
        result = runner.invoke(call_app, ["todos", "upcoming", "--facet", "work"])
        assert result.exit_code == 0
        assert "Work task" in result.output

    def test_upcoming_no_future_todos(self, todo_env):
        """No future todos shows appropriate message."""
        todo_env([], day="20200101")
        result = runner.invoke(call_app, ["todos", "upcoming"])
        assert result.exit_code == 0
        assert "No upcoming todos" in result.output


class TestTodosNudges:
    class _FixedDateTime:
        @classmethod
        def now(cls):
            return datetime(2026, 3, 10, 12, 0)

    def test_list_nudges_due_is_readonly(self, todo_env, monkeypatch):
        day, facet, todo_path = todo_env(
            [{"text": "Follow up", "nudge": "20260310T09:00"}],
            day="20260310",
        )
        before = todo_path.read_text(encoding="utf-8")
        monkeypatch.setattr(todos_call, "datetime", self._FixedDateTime)

        result = runner.invoke(call_app, ["todos", "list-nudges-due", "--facet", facet])

        assert result.exit_code == 0
        assert "Follow up" in result.output
        assert todo_path.read_text(encoding="utf-8") == before

    def test_list_nudges_due_json_all_facets(self, todo_env, monkeypatch):
        todo_env(
            [{"text": "Work ping", "nudge": "20260310T08:00"}],
            day="20260310",
            facet="work",
        )
        todo_env(
            [{"text": "Home ping", "nudge": "20260310T09:00"}],
            day="20260310",
            facet="home",
        )
        monkeypatch.setattr(todos_call, "datetime", self._FixedDateTime)

        result = runner.invoke(call_app, ["todos", "list-nudges-due", "--json"])

        assert result.exit_code == 0
        payload = json.loads(result.output)
        assert payload == [
            {
                "day": "20260310",
                "facet": "work",
                "index": 1,
                "text": "Work ping",
                "nudge": "20260310T08:00",
                "nudge_display": "4h ago",
            },
            {
                "day": "20260310",
                "facet": "home",
                "index": 1,
                "text": "Home ping",
                "nudge": "20260310T09:00",
                "nudge_display": "3h ago",
            },
        ]

    def test_list_nudges_due_empty(self, todo_env, monkeypatch):
        todo_env([], day="20260310")
        monkeypatch.setattr(todos_call, "datetime", self._FixedDateTime)

        human = runner.invoke(call_app, ["todos", "list-nudges-due"])
        json_result = runner.invoke(call_app, ["todos", "list-nudges-due", "--json"])

        assert human.exit_code == 0
        assert human.output.strip() == "No nudges due."
        assert json_result.exit_code == 0
        assert json.loads(json_result.output) == []

    def test_dispatch_nudges_notifies_and_marks(self, todo_env, monkeypatch):
        _day, facet, todo_path = todo_env(
            [{"text": "Follow up", "nudge": "20260310T09:00"}],
            day="20260310",
        )
        calls: list[tuple[list[str], dict]] = []

        def fake_run(argv, **kwargs):
            calls.append((argv, kwargs))
            return None

        monkeypatch.setattr(todos_call, "datetime", self._FixedDateTime)
        monkeypatch.setattr(todos_call.subprocess, "run", fake_run)

        result = runner.invoke(call_app, ["todos", "dispatch-nudges", "--facet", facet])

        assert result.exit_code == 0
        assert result.output.strip() == "dispatched 1 nudge(s)"
        assert calls == [
            (
                [
                    "sol",
                    "notify",
                    "Follow up",
                    "--title",
                    "Todo Reminder",
                    "--icon",
                    "✅",
                    "--app",
                    "todos",
                    "--facet",
                    facet,
                    "--action",
                    "/app/todos/20260310",
                ],
                {"check": False, "capture_output": True},
            )
        ]
        saved = [
            json.loads(line)
            for line in todo_path.read_text(encoding="utf-8").splitlines()
        ]
        assert saved == [
            {
                "text": "Follow up",
                "nudge": "20260310T09:00",
                "notified": True,
            }
        ]

    def test_dispatch_nudges_noop_when_nothing_due(self, todo_env, monkeypatch):
        todo_env([], day="20260310")
        calls: list[tuple[list[str], dict]] = []

        def fake_run(argv, **kwargs):
            calls.append((argv, kwargs))
            return None

        monkeypatch.setattr(todos_call, "datetime", self._FixedDateTime)
        monkeypatch.setattr(todos_call.subprocess, "run", fake_run)

        result = runner.invoke(call_app, ["todos", "dispatch-nudges"])

        assert result.exit_code == 0
        assert result.output.strip() == "dispatched 0 nudge(s)"
        assert calls == []

    def test_legacy_nudge_command_removed(self, todo_env):
        todo_env([], day="20260310")

        result = runner.invoke(call_app, ["todos", "check" + "-nudges"])

        assert result.exit_code != 0


class TestTodosMove:
    """Tests for 'sol call todos move' command."""

    def test_move_todo(self, move_env):
        journal, src_facet, dst_facet = move_env([{"text": "Ship feature"}])

        result = runner.invoke(
            call_app,
            [
                "todos",
                "move",
                "1",
                "--day",
                "20240101",
                "--from",
                src_facet,
                "--to",
                dst_facet,
            ],
        )

        assert result.exit_code == 0
        source_items = [
            json.loads(line)
            for line in (journal / "facets" / src_facet / "todos" / "20240101.jsonl")
            .read_text(encoding="utf-8")
            .splitlines()
        ]
        dest_items = [
            json.loads(line)
            for line in (journal / "facets" / dst_facet / "todos" / "20240101.jsonl")
            .read_text(encoding="utf-8")
            .splitlines()
        ]
        assert source_items[0]["cancelled"] is True
        assert source_items[0]["cancelled_reason"] == "moved_to_facet"
        assert source_items[0]["moved_to"] == dst_facet
        assert dest_items[0]["text"] == "Ship feature"
        assert dest_items[0]["created_at"] == source_items[0]["created_at"]

    def test_move_todo_with_nudge(self, move_env):
        journal, src_facet, dst_facet = move_env(
            [{"text": "Call Alice", "nudge": "20240101T09:00"}]
        )

        result = runner.invoke(
            call_app,
            [
                "todos",
                "move",
                "1",
                "--day",
                "20240101",
                "--from",
                src_facet,
                "--to",
                dst_facet,
            ],
        )

        assert result.exit_code == 0
        dest_items = [
            json.loads(line)
            for line in (journal / "facets" / dst_facet / "todos" / "20240101.jsonl")
            .read_text(encoding="utf-8")
            .splitlines()
        ]
        assert dest_items[0]["nudge"] == "20240101T09:00"

    def test_move_already_cancelled(self, move_env):
        _, src_facet, dst_facet = move_env(
            [{"text": "Ship feature", "cancelled": True}]
        )

        result = runner.invoke(
            call_app,
            [
                "todos",
                "move",
                "1",
                "--day",
                "20240101",
                "--from",
                src_facet,
                "--to",
                dst_facet,
            ],
        )

        assert result.exit_code == 1
        assert "already cancelled" in result.output

    def test_move_already_completed(self, move_env):
        _, src_facet, dst_facet = move_env(
            [{"text": "Ship feature", "completed": True}]
        )

        result = runner.invoke(
            call_app,
            [
                "todos",
                "move",
                "1",
                "--day",
                "20240101",
                "--from",
                src_facet,
                "--to",
                dst_facet,
            ],
        )

        assert result.exit_code == 1
        assert "completed todo" in result.output

    def test_move_invalid_line_number(self, move_env):
        _, src_facet, dst_facet = move_env([{"text": "Ship feature"}])

        result = runner.invoke(
            call_app,
            [
                "todos",
                "move",
                "5",
                "--day",
                "20240101",
                "--from",
                src_facet,
                "--to",
                dst_facet,
            ],
        )

        assert result.exit_code == 1
        assert "out of range" in result.output

    def test_move_missing_facet(self, move_env):
        move_env([{"text": "Ship feature"}], dst_facet="personal")

        result = runner.invoke(
            call_app,
            [
                "todos",
                "move",
                "1",
                "--day",
                "20240101",
                "--from",
                "work",
                "--to",
                "missing",
            ],
        )

        assert result.exit_code == 1
        assert "does not exist" in result.output


class TestSolEnvResolution:
    """Tests for SOL_* env var resolution in todos commands."""

    def test_list_from_sol_day(self, todo_env, monkeypatch):
        """list with SOL_DAY env and no day arg works."""
        todo_env([{"text": "Env task"}], day="20240101")
        monkeypatch.setenv("SOL_DAY", "20240101")
        result = runner.invoke(call_app, ["todos", "list", "--facet", "personal"])
        assert result.exit_code == 0
        assert "Env task" in result.output

    def test_add_from_sol_day_and_facet(self, todo_env, monkeypatch):
        """add with SOL_DAY + SOL_FACET env works."""
        todo_env([], day="29991231")
        monkeypatch.setenv("SOL_DAY", "29991231")
        monkeypatch.setenv("SOL_FACET", "personal")
        result = runner.invoke(call_app, ["todos", "add", "Env todo"])
        assert result.exit_code == 0
        assert "Env todo" in result.output

    def test_done_from_sol_day_and_facet(self, todo_env, monkeypatch):
        """done with SOL_DAY + SOL_FACET env works."""
        todo_env([{"text": "Buy milk"}], day="20240101")
        monkeypatch.setenv("SOL_DAY", "20240101")
        monkeypatch.setenv("SOL_FACET", "personal")
        result = runner.invoke(call_app, ["todos", "done", "1"])
        assert result.exit_code == 0
        assert "[x]" in result.output


class TestTodosAddDedup:
    """Tests for cross-facet duplicate detection in 'sol call todos add'."""

    def test_add_rejects_duplicate_in_other_facet(self, move_env):
        """Adding a duplicate todo in another facet is rejected with exit code 1."""
        _, src_facet, dst_facet = move_env([{"text": "Draft Q1 plan"}], day="20240102")
        result = runner.invoke(
            call_app,
            [
                "todos",
                "add",
                "Draft Q1 plan",
                "--day",
                "20240102",
                "--facet",
                dst_facet,
            ],
        )
        assert result.exit_code == 1
        assert "Duplicate detected" in result.output

    def test_add_force_bypasses_dedup(self, move_env):
        """--force flag allows adding despite duplicate detection."""
        _, src_facet, dst_facet = move_env([{"text": "Draft Q1 plan"}], day="20240102")
        result = runner.invoke(
            call_app,
            [
                "todos",
                "add",
                "Draft Q1 plan",
                "--day",
                "20240102",
                "--facet",
                dst_facet,
                "--force",
            ],
        )
        assert result.exit_code == 0
        assert "Draft Q1 plan" in result.output

    def test_add_succeeds_when_no_matches(self, move_env):
        """Adding a unique todo succeeds normally."""
        _, src_facet, dst_facet = move_env([{"text": "Buy groceries"}], day="20240102")
        result = runner.invoke(
            call_app,
            [
                "todos",
                "add",
                "Draft Q1 plan",
                "--day",
                "20240102",
                "--facet",
                dst_facet,
            ],
        )
        assert result.exit_code == 0
        assert "Draft Q1 plan" in result.output

    def test_add_dedup_stderr_format(self, move_env):
        """Rejection message includes score, facet, day, line, and text."""
        _, src_facet, dst_facet = move_env([{"text": "Draft Q1 plan"}], day="20240102")
        result = runner.invoke(
            call_app,
            [
                "todos",
                "add",
                "Draft Q1 plan",
                "--day",
                "20240102",
                "--facet",
                dst_facet,
            ],
        )
        assert result.exit_code == 1
        assert "100%" in result.output
        assert src_facet in result.output
        assert "20240102" in result.output
        assert "line 1" in result.output
        assert "--force" in result.output
