# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Test supervisor daily scheduling functionality."""

import logging
import os
from datetime import date
from unittest.mock import MagicMock, Mock, call

import pytest

import solstone.think.supervisor as mod


def _daily_think_calls(days):
    return [call(["journal", "think", "-v", "--day", day], day=day) for day in days]


@pytest.fixture
def submit_mock(monkeypatch):
    mock = Mock()
    monkeypatch.setattr(mod._task_queue, "submit", mock)
    return mock


@pytest.fixture
def set_today(monkeypatch):
    def _set_today(today):
        fake_datetime = Mock()
        fake_datetime.now.return_value.date.return_value = today
        monkeypatch.setattr(mod, "datetime", fake_datetime)

    return _set_today


def daily_complete_message(**overrides):
    message = {
        "tract": "think",
        "event": "daily_complete",
        "day": "20260318",
        "success": 3,
        "failed": 0,
        "duration_ms": 5000,
    }
    message.update(overrides)
    return message


@pytest.mark.parametrize(
    ("last_day", "today", "updated_days_return", "expected_days"),
    [
        pytest.param(
            date(2025, 1, 1),
            date(2025, 1, 2),
            ["20250101"],
            ["20250101"],
            id="one-updated-day",
        ),
        pytest.param(
            date(2025, 1, 5),
            date(2025, 1, 6),
            ["20250103", "20250104", "20250105"],
            ["20250103", "20250104", "20250105"],
            id="multiple-updated-days",
        ),
        pytest.param(
            date(2025, 1, 10),
            date(2025, 1, 11),
            [
                "20250104",
                "20250105",
                "20250106",
                "20250107",
                "20250108",
                "20250109",
                "20250110",
            ],
            ["20250107", "20250108", "20250109", "20250110"],
            id="max-updated-catchup",
        ),
    ],
)
def test_handle_daily_tasks_submits_think_runs_on_day_change(
    mock_callosum,
    monkeypatch,
    submit_mock,
    set_today,
    last_day,
    today,
    updated_days_return,
    expected_days,
):
    mod._daily_state["last_day"] = last_day
    set_today(today)
    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: updated_days_return)

    mod.handle_daily_tasks()

    assert submit_mock.call_args_list == [
        call(["journal", "think", "-v", "--day", day], day=day) for day in expected_days
    ]
    assert mod._daily_state["last_day"] == today


def test_no_spawn_same_day(mock_callosum, submit_mock, set_today):
    today = date(2025, 1, 2)
    mod._daily_state["last_day"] = today
    set_today(today)

    mod.handle_daily_tasks()

    submit_mock.assert_not_called()


def test_skipped_in_remote_mode(mock_callosum, submit_mock, set_today):
    mod._daily_state["last_day"] = date(2025, 1, 1)
    mod._is_remote_mode = True
    set_today(date(2025, 1, 2))

    mod.handle_daily_tasks()

    submit_mock.assert_not_called()
    assert mod._daily_state["last_day"] == date(2025, 1, 1)


def test_advances_state_with_no_updated_days(
    mock_callosum, monkeypatch, submit_mock, set_today
):
    mod._daily_state["last_day"] = date(2025, 1, 1)
    set_today(date(2025, 1, 2))
    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: [])

    mod.handle_daily_tasks()

    submit_mock.assert_not_called()
    assert mod._daily_state["last_day"] == date(2025, 1, 2)


def test_excludes_today(mock_callosum, monkeypatch, submit_mock, set_today):
    mod._daily_state["last_day"] = date(2025, 1, 1)
    set_today(date(2025, 1, 2))
    updated_days = MagicMock(return_value=["20250101"])
    monkeypatch.setattr(mod, "updated_days", updated_days)

    mod.handle_daily_tasks()

    assert updated_days.call_args.kwargs["exclude"] == {"20250102"}


def test_run_catchup_drain_excludes_stuck_days(mock_callosum, monkeypatch, submit_mock):
    monkeypatch.setattr(
        mod,
        "updated_days",
        lambda **kwargs: ["20250101", "20250102", "20250103"],
    )
    monkeypatch.setattr(mod, "read_day_stuck", lambda day: day == "20250102")

    submitted = mod.run_catchup_drain()

    assert submitted == ["20250101", "20250103"]
    assert submit_mock.call_args_list == _daily_think_calls(submitted)


def test_run_catchup_drain_force_day_bypasses_stuck_filter(
    mock_callosum, monkeypatch, submit_mock
):
    monkeypatch.setattr(
        mod,
        "updated_days",
        lambda **kwargs: ["20250101", "20250102", "20250103"],
    )
    monkeypatch.setattr(mod, "read_day_stuck", lambda day: day == "20250102")

    submitted = mod.run_catchup_drain(force_days={"20250102"})

    assert submitted == ["20250101", "20250102", "20250103"]
    assert submit_mock.call_args_list == _daily_think_calls(submitted)


def test_run_catchup_drain_limits_to_freshest_without_skip_warning(
    mock_callosum, monkeypatch, submit_mock, caplog
):
    caplog.set_level(logging.WARNING)
    pending = [
        "20250101",
        "20250102",
        "20250103",
        "20250104",
        "20250105",
        "20250106",
        "20250107",
    ]
    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: pending)
    monkeypatch.setattr(mod, "read_day_stuck", lambda day: False)

    submitted = mod.run_catchup_drain()

    assert submitted == ["20250104", "20250105", "20250106", "20250107"]
    assert submit_mock.call_args_list == _daily_think_calls(submitted)
    warning_messages = [
        record.getMessage().lower()
        for record in caplog.records
        if record.levelno >= logging.WARNING
    ]
    assert all("skip" not in message for message in warning_messages)
    assert all("dropping" not in message for message in warning_messages)


def test_run_catchup_drain_submits_next_freshest_on_reinvocation(
    mock_callosum, monkeypatch, submit_mock
):
    pending = [
        "20250101",
        "20250102",
        "20250103",
        "20250104",
        "20250105",
        "20250106",
        "20250107",
    ]

    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: list(pending))
    monkeypatch.setattr(mod, "read_day_stuck", lambda day: False)

    first = mod.run_catchup_drain()
    for day in first:
        pending.remove(day)
    submit_mock.reset_mock()

    second = mod.run_catchup_drain()

    assert first == ["20250104", "20250105", "20250106", "20250107"]
    assert second == ["20250101", "20250102", "20250103"]
    assert submit_mock.call_args_list == _daily_think_calls(second)


def test_run_catchup_drain_force_day_union_once(
    mock_callosum, monkeypatch, submit_mock
):
    pending = [
        "20250101",
        "20250102",
        "20250103",
        "20250104",
        "20250105",
        "20250106",
        "20250107",
    ]
    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: pending)
    monkeypatch.setattr(mod, "read_day_stuck", lambda day: False)

    submitted = mod.run_catchup_drain(force_days={"20250106"})

    assert submitted.count("20250106") == 1
    assert submitted == ["20250104", "20250105", "20250106", "20250107"]

    submit_mock.reset_mock()
    submitted = mod.run_catchup_drain(force_days={"20241230", "20241231"})

    assert len(submitted) == mod.MAX_UPDATED_CATCHUP + 2
    assert submit_mock.call_args_list == _daily_think_calls(submitted)


def test_run_catchup_drain_degrades_to_unfiltered_on_predicate_error(
    mock_callosum, monkeypatch, submit_mock, caplog
):
    caplog.set_level(logging.WARNING)
    pending = [
        "20250101",
        "20250102",
        "20250103",
        "20250104",
        "20250105",
        "20250106",
        "20250107",
    ]
    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: pending)

    def raise_stuck(day):
        raise RuntimeError(f"boom {day}")

    monkeypatch.setattr(mod, "read_day_stuck", raise_stuck)

    submitted = mod.run_catchup_drain()

    assert submitted == ["20250104", "20250105", "20250106", "20250107"]
    assert submit_mock.call_args_list == _daily_think_calls(submitted)
    warnings = [
        record
        for record in caplog.records
        if record.getMessage()
        == "Stuck-day filter unavailable; draining unfiltered catchup set"
    ]
    assert len(warnings) == 1


def test_run_catchup_drain_emits_no_followon_drain(
    mock_callosum, monkeypatch, submit_mock
):
    callosum = Mock()
    monkeypatch.setattr(mod, "_supervisor_callosum", callosum)
    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: ["20250101"])
    monkeypatch.setattr(mod, "read_day_stuck", lambda day: False)

    mod.run_catchup_drain()

    assert submit_mock.call_args_list == _daily_think_calls(["20250101"])
    callosum.emit.assert_not_called()


def test_handle_callosum_drain_runs_drain(mock_callosum, monkeypatch, submit_mock):
    pending = [
        "20250101",
        "20250102",
        "20250103",
        "20250104",
        "20250105",
        "20250106",
        "20250107",
    ]
    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: pending)
    monkeypatch.setattr(mod, "read_day_stuck", lambda day: day == "20250105")

    mod._handle_callosum_message({"tract": "supervisor", "event": "drain"})

    expected = ["20250103", "20250104", "20250106", "20250107"]
    assert submit_mock.call_args_list == _daily_think_calls(expected)


def test_handle_callosum_drain_with_day_forces_day(
    mock_callosum, monkeypatch, submit_mock
):
    pending = [
        "20250101",
        "20250102",
        "20250103",
        "20250104",
        "20250105",
        "20250106",
        "20250107",
    ]
    monkeypatch.setattr(mod, "updated_days", lambda **kwargs: pending)
    monkeypatch.setattr(mod, "read_day_stuck", lambda day: day == "20250102")

    mod._handle_callosum_message(
        {"tract": "supervisor", "event": "drain", "day": "20250102"}
    )

    expected = ["20250102", "20250104", "20250105", "20250106", "20250107"]
    assert submit_mock.call_args_list == _daily_think_calls(expected)


def test_handle_callosum_drain_ignored_in_remote_mode(
    mock_callosum, monkeypatch, submit_mock
):
    mod._is_remote_mode = True
    monkeypatch.setattr(
        mod,
        "updated_days",
        Mock(side_effect=AssertionError("should not drain in remote mode")),
    )

    mod._handle_callosum_message({"tract": "supervisor", "event": "drain"})

    submit_mock.assert_not_called()


def test_handle_think_daily_complete_submits_heartbeat(
    mock_callosum, tmp_path, monkeypatch, submit_mock
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(exist_ok=True)

    mod._handle_think_daily_complete(daily_complete_message())

    submit_mock.assert_called_once_with(["journal", "heartbeat"])


@pytest.mark.parametrize(
    "message",
    [
        pytest.param(
            {"tract": "supervisor", "event": "daily_complete"}, id="wrong-tract"
        ),
        pytest.param({"tract": "think", "event": "started"}, id="wrong-event"),
        pytest.param({}, id="empty-message"),
    ],
)
def test_ignores_non_think_daily_complete(
    mock_callosum, tmp_path, monkeypatch, submit_mock, message
):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    (tmp_path / "health").mkdir(exist_ok=True)

    mod._handle_think_daily_complete(message)

    submit_mock.assert_not_called()


def test_skips_when_pid_alive(mock_callosum, tmp_path, monkeypatch, submit_mock):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health = tmp_path / "health"
    health.mkdir(exist_ok=True)
    (health / "heartbeat.pid").write_text(str(os.getpid()))

    mod._handle_think_daily_complete(daily_complete_message())

    submit_mock.assert_not_called()


def test_proceeds_on_dead_pid(mock_callosum, tmp_path, monkeypatch, submit_mock):
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    health = tmp_path / "health"
    health.mkdir(exist_ok=True)
    (health / "heartbeat.pid").write_text("99999999")

    mod._handle_think_daily_complete(daily_complete_message())

    submit_mock.assert_called_once_with(["journal", "heartbeat"])
