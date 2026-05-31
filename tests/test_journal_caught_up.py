# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import pytest

from solstone.think.pipeline_health import (
    BACKLOG_STATE_UNKNOWN,
    BacklogDay,
    BacklogError,
    BacklogView,
)


@pytest.fixture
def doctor():
    from solstone.think import doctor as doctor_module

    return doctor_module


def args(doctor):
    return doctor.Args(verbose=False, json=False, jsonl=False, port=5015)


def clean_view() -> BacklogView:
    return BacklogView(
        window=30,
        days=(),
        pending_days=0,
        stuck_days=0,
        oldest_pending_day=None,
        errors=(),
    )


def unknown_day(day: str = "20200228") -> tuple[BacklogDay, BacklogError]:
    error = BacklogError(day=day, stage="terminal_states", message="boom")
    backlog_day = BacklogDay(
        day=day,
        state=BACKLOG_STATE_UNKNOWN,
        segments=0,
        units=0,
        not_sensed=0,
        why=(),
        reason=None,
        error=error,
    )
    return backlog_day, error


def backlog_day(day: str, state: str) -> BacklogDay:
    return BacklogDay(
        day=day,
        state=state,
        segments=0,
        units=0,
        not_sensed=0,
        why=(),
        reason=None,
        error=None,
    )


def test_journal_caught_up_ok_only_when_fully_clean(doctor, monkeypatch):
    monkeypatch.setattr(doctor, "read_backlog_view", clean_view)

    result = doctor.journal_caught_up_check(args(doctor))

    assert result.status == "ok"
    assert result.detail == "caught up"
    assert result.severity == "advisory"


def test_journal_caught_up_unknown_day_warns_before_false_green(
    doctor,
    monkeypatch,
):
    day, error = unknown_day()
    view = BacklogView(
        window=30,
        days=(day,),
        pending_days=0,
        stuck_days=0,
        oldest_pending_day=None,
        errors=(error,),
    )
    monkeypatch.setattr(doctor, "read_backlog_view", lambda: view)

    result = doctor.journal_caught_up_check(args(doctor))

    assert result.status == "warn"
    assert "couldn't fully determine" in result.detail
    assert result.status != "ok"


def test_journal_caught_up_pending_and_stuck_warn_with_distinct_counts(
    doctor,
    monkeypatch,
):
    view = BacklogView(
        window=30,
        days=(
            backlog_day("20200229", "pending"),
            backlog_day("20200301", "pending"),
            backlog_day("20200302", "stuck"),
        ),
        pending_days=2,
        stuck_days=1,
        oldest_pending_day="20200229",
        errors=(),
    )
    monkeypatch.setattr(doctor, "read_backlog_view", lambda: view)

    result = doctor.journal_caught_up_check(args(doctor))

    assert result.status == "warn"
    assert result.severity == "advisory"
    assert "2 day(s) pending" in result.detail
    assert "1 day(s) stuck" in result.detail
    assert "oldest outstanding 20200229" in result.detail
    assert str(2 + 1) not in result.detail


def test_journal_caught_up_warn_is_visible_but_non_blocking(doctor):
    results = [
        doctor.make_result(doctor.JOURNAL_CAUGHT_UP_CHECK, "warn", "..."),
        doctor.make_result(doctor.SERVICE_RUNNING_CHECK, "ok", "..."),
    ]

    assert doctor.jsonl_summary_status(results) == "warning"
    assert not any(
        result.severity == "blocker" and result.status == "fail" for result in results
    )


def test_journal_caught_up_is_only_in_journal_battery(doctor):
    assert "journal_caught_up" in {check.name for check, _ in doctor.JOURNAL_CHECKS}
    assert "journal_caught_up" not in {
        check.name for check, _ in doctor.UNIVERSAL_CHECKS
    }
    assert "journal_caught_up" not in {
        check.name for check, _ in doctor.READINESS_CHECKS
    }


def test_journal_caught_up_rides_existing_emission_paths(doctor, capsys):
    warn_result = doctor.make_result(doctor.JOURNAL_CAUGHT_UP_CHECK, "warn", "...")
    ok_result = doctor.make_result(doctor.JOURNAL_CAUGHT_UP_CHECK, "ok", "caught up")

    doctor.emit_json([warn_result])
    assert "journal_caught_up" in capsys.readouterr().out

    doctor.emit_jsonl(
        [warn_result],
        started_at_iso="x",
        duration_ms=0,
        summary_status="warning",
    )
    assert "journal_caught_up" in capsys.readouterr().out

    doctor.emit_text([ok_result], verbose=False)
    assert "journal_caught_up" not in capsys.readouterr().out

    doctor.emit_text([ok_result], verbose=True)
    assert "journal_caught_up" in capsys.readouterr().out
