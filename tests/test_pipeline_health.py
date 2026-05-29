# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.pipeline_health."""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path

import pytest

from solstone.think.pipeline_health import (
    pipeline_status_message,
    read_completed_units,
    summarize_pipeline_day,
)


def _write_jsonl(path: Path, events: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for event in events:
            handle.write(json.dumps(event) + "\n")


@pytest.fixture
def pipeline_journal(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    return journal


def test_read_completed_units_missing_health_dir(pipeline_journal):
    (pipeline_journal / "chronicle" / "20990201").mkdir(parents=True)

    assert read_completed_units("20990201") == set()


def test_read_completed_units_terminal_presence(pipeline_journal):
    day = "20990202"
    base = pipeline_journal / "chronicle" / day / "health"
    _write_jsonl(
        base / "001_daily.jsonl",
        [
            {"event": "talent.complete", "ts": 1, "mode": "daily", "name": "done"},
            {"event": "talent.fail", "ts": 1, "mode": "daily", "name": "failed"},
            {"event": "talent.dispatch", "ts": 1, "mode": "daily", "name": "sent"},
            {"event": "talent.skip", "ts": 1, "mode": "daily", "name": "skipped"},
        ],
    )

    assert read_completed_units(day) == {("daily", "done", None)}


def test_read_completed_units_latest_terminal_wins(pipeline_journal):
    day = "20990203"
    base = pipeline_journal / "chronicle" / day / "health"
    _write_jsonl(
        base / "001_daily.jsonl",
        [{"event": "talent.complete", "ts": 1, "mode": "daily", "name": "alpha"}],
    )
    _write_jsonl(
        base / "002_daily.jsonl",
        [{"event": "talent.fail", "ts": 2, "mode": "daily", "name": "alpha"}],
    )
    _write_jsonl(
        base / "003_daily.jsonl",
        [{"event": "talent.fail", "ts": 1, "mode": "daily", "name": "beta"}],
    )
    _write_jsonl(
        base / "004_daily.jsonl",
        [{"event": "talent.complete", "ts": 2, "mode": "daily", "name": "beta"}],
    )

    assert read_completed_units(day) == {("daily", "beta", None)}


def test_read_completed_units_skip_is_non_terminal(pipeline_journal):
    day = "20990204"
    base = pipeline_journal / "chronicle" / day / "health"
    _write_jsonl(
        base / "001_daily.jsonl",
        [{"event": "talent.complete", "ts": 1, "mode": "daily", "name": "alpha"}],
    )
    _write_jsonl(
        base / "002_daily.jsonl",
        [{"event": "talent.skip", "ts": 2, "mode": "daily", "name": "alpha"}],
    )

    assert read_completed_units(day) == {("daily", "alpha", None)}


def test_read_completed_units_equal_ts_later_record_wins(pipeline_journal):
    day = "20990205"
    base = pipeline_journal / "chronicle" / day / "health"
    _write_jsonl(
        base / "001_daily.jsonl",
        [{"event": "talent.complete", "ts": 1, "mode": "daily", "name": "alpha"}],
    )
    _write_jsonl(
        base / "002_daily.jsonl",
        [{"event": "talent.fail", "ts": 1, "mode": "daily", "name": "alpha"}],
    )

    assert read_completed_units(day) == set()


def test_read_completed_units_keys_include_facet(pipeline_journal):
    day = "20990206"
    base = pipeline_journal / "chronicle" / day / "health"
    _write_jsonl(
        base / "001_daily.jsonl",
        [
            {
                "event": "talent.complete",
                "ts": 1,
                "mode": "daily",
                "name": "facet_newsletter",
                "facet": "work",
            },
            {
                "event": "talent.fail",
                "ts": 1,
                "mode": "daily",
                "name": "facet_newsletter",
                "facet": "personal",
            },
        ],
    )

    assert read_completed_units(day) == {("daily", "facet_newsletter", "work")}


def test_read_completed_units_skips_malformed_records(pipeline_journal):
    day = "20990207"
    path = pipeline_journal / "chronicle" / day / "health" / "001_daily.jsonl"
    path.parent.mkdir(parents=True)
    with path.open("w", encoding="utf-8") as handle:
        handle.write("{bad json\n")
        handle.write(json.dumps({"event": "talent.complete", "ts": 1, "name": "alpha"}))
        handle.write("\n")
        handle.write(json.dumps({"event": "talent.complete", "ts": 1, "mode": "daily"}))
        handle.write("\n")
        handle.write(
            json.dumps(
                {
                    "event": "talent.complete",
                    "ts": "not-int",
                    "mode": "daily",
                    "name": "beta",
                }
            )
        )
        handle.write("\n")
        handle.write(
            json.dumps(
                {
                    "event": "talent.complete",
                    "ts": 2,
                    "mode": "daily",
                    "name": "gamma",
                }
            )
        )
        handle.write("\n")

    assert read_completed_units(day) == {("daily", "gamma", None)}


def test_empty_day_is_healthy(pipeline_journal):
    summary = summarize_pipeline_day("20260101")

    assert summary["status"] == "healthy"
    assert summary["anomalies"] == []
    assert summary["talents"] == {
        "dispatched": 0,
        "completed": 0,
        "failed": 0,
        "skipped": 0,
        "failed_list": [],
        "failed_list_truncated": False,
    }
    assert summary["activities"] == {
        "detected": 0,
        "persisted": 0,
        "talents_fired": False,
    }
    assert all(
        run == {"count": 0, "duration_ms_total": 0} for run in summary["runs"].values()
    )


def test_missing_health_dir(pipeline_journal):
    (pipeline_journal / "chronicle" / "20260101").mkdir(parents=True)

    summary = summarize_pipeline_day("20260101")

    assert summary["status"] == "healthy"
    assert summary["anomalies"] == []
    assert summary["runs"]["daily"]["count"] == 0


def test_healthy_day_with_all_modes(pipeline_journal):
    day = "20990101"
    base = pipeline_journal / "chronicle" / day / "health"
    _write_jsonl(
        base / "1_segment.jsonl",
        [
            {"event": "run.start", "mode": "segment"},
            {"event": "talent.dispatch", "mode": "segment"},
            {"event": "talent.complete", "mode": "segment"},
            {"event": "run.complete", "mode": "segment", "duration_ms": 10},
        ],
    )
    _write_jsonl(
        base / "2_daily.jsonl",
        [
            {"event": "run.start", "mode": "daily"},
            {"event": "talent.dispatch", "mode": "daily"},
            {"event": "talent.complete", "mode": "daily"},
            {"event": "run.complete", "mode": "daily", "duration_ms": 20},
        ],
    )
    _write_jsonl(
        base / "3_activity.jsonl",
        [
            {"event": "run.start", "mode": "activity"},
            {"event": "talent.dispatch", "mode": "activity"},
            {"event": "talent.complete", "mode": "activity"},
            {"event": "run.complete", "mode": "activity", "duration_ms": 30},
        ],
    )

    summary = summarize_pipeline_day(day)

    assert summary["status"] == "healthy"
    assert summary["talents"]["dispatched"] == 3
    assert summary["talents"]["completed"] == 3
    assert summary["runs"]["segment"] == {"count": 1, "duration_ms_total": 10}
    assert summary["runs"]["daily"] == {"count": 1, "duration_ms_total": 20}
    assert summary["runs"]["activity"] == {"count": 1, "duration_ms_total": 30}
    assert summary["activities"]["talents_fired"] is True


def test_agent_failure_promotes_warning(pipeline_journal):
    day = "20990102"
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "1_segment.jsonl",
        [
            {
                "event": "talent.fail",
                "mode": "segment",
                "name": "screen",
                "use_id": "a-1",
                "state": "timeout",
            }
        ],
    )

    summary = summarize_pipeline_day(day)

    assert summary["status"] == "warning"
    assert summary["talents"]["failed"] == 1
    assert summary["talents"]["failed_list"] == [
        {"mode": "segment", "name": "screen", "use_id": "a-1", "state": "timeout"}
    ]
    assert summary["anomalies"] == [
        {
            "kind": "talent_failure",
            "mode": "segment",
            "name": "screen",
            "use_id": "a-1",
            "state": "timeout",
        }
    ]


def test_failed_list_truncates_at_20(pipeline_journal):
    day = "20990103"
    events = [
        {
            "event": "talent.fail",
            "mode": "daily",
            "name": f"agent-{idx}",
            "use_id": f"id-{idx}",
            "state": "error",
        }
        for idx in range(25)
    ]
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "1_daily.jsonl", events
    )

    summary = summarize_pipeline_day(day)

    assert summary["talents"]["failed"] == 25
    assert len(summary["talents"]["failed_list"]) == 20
    assert summary["talents"]["failed_list_truncated"] is True
    assert sum(1 for a in summary["anomalies"] if a["kind"] == "talent_failure") == 20


def test_activity_detected_without_run_is_stale(pipeline_journal):
    day = "20990104"
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "1_segment.jsonl",
        [{"event": "activity.detected", "mode": "segment"}],
    )

    summary = summarize_pipeline_day(day)

    assert summary["status"] == "stale"
    assert {"kind": "activity_agents_missing"} in summary["anomalies"]


def test_past_day_without_daily_run_is_stale(pipeline_journal, monkeypatch):
    day = "20200101"
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "1_segment.jsonl",
        [{"event": "run.start", "mode": "segment"}],
    )
    monkeypatch.setattr(
        "solstone.think.pipeline_health._now", lambda: datetime(2020, 1, 2, 12, 0, 0)
    )

    summary = summarize_pipeline_day(day)

    assert summary["status"] == "stale"
    assert {"kind": "daily_agents_missing"} in summary["anomalies"]


def test_today_before_23h_no_daily_run_is_healthy(pipeline_journal, monkeypatch):
    current = datetime(2026, 4, 16, 12, 0, 0)
    monkeypatch.setattr("solstone.think.pipeline_health._now", lambda: current)
    (pipeline_journal / "chronicle" / current.strftime("%Y%m%d") / "health").mkdir(
        parents=True
    )

    summary = summarize_pipeline_day(current.strftime("%Y%m%d"))

    assert summary["status"] == "healthy"
    assert {"kind": "daily_agents_missing"} not in summary["anomalies"]


def test_today_after_23h_no_daily_run_is_stale(pipeline_journal, monkeypatch):
    current = datetime(2026, 4, 16, 23, 30, 0)
    monkeypatch.setattr("solstone.think.pipeline_health._now", lambda: current)
    (pipeline_journal / "chronicle" / current.strftime("%Y%m%d") / "health").mkdir(
        parents=True
    )

    summary = summarize_pipeline_day(current.strftime("%Y%m%d"))

    assert summary["status"] == "stale"
    assert {"kind": "daily_agents_missing"} in summary["anomalies"]


def test_segment_runs_missing_is_soft(pipeline_journal, monkeypatch):
    day = "20990105"
    (pipeline_journal / "chronicle" / day / "health").mkdir(parents=True)
    monkeypatch.setattr(
        "solstone.think.pipeline_health.iter_segments",
        lambda value: [("default", "120000_300", Path("/tmp/fake"))],
    )

    summary = summarize_pipeline_day(day)

    assert summary["status"] == "healthy"
    assert {"kind": "segment_runs_missing"} in summary["anomalies"]


def test_invalid_day_returns_healthy_empty(pipeline_journal):
    summary = summarize_pipeline_day("not-a-date")

    assert summary["status"] == "healthy"
    assert summary["anomalies"] == []
    assert summary["talents"] == {
        "dispatched": 0,
        "completed": 0,
        "failed": 0,
        "skipped": 0,
        "failed_list": [],
        "failed_list_truncated": False,
    }


def test_malformed_json_lines_skipped(pipeline_journal):
    day = "20990106"
    path = pipeline_journal / "chronicle" / day / "health" / "1_segment.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps({"event": "run.start", "mode": "segment"})
        + "\nnot json at all\n"
        + json.dumps({"event": "talent.dispatch", "mode": "segment"})
        + "\n",
        encoding="utf-8",
    )

    summary = summarize_pipeline_day(day)

    assert summary["runs"]["segment"]["count"] == 1
    assert summary["talents"]["dispatched"] == 1


@pytest.mark.parametrize(
    ("summary", "expected"),
    [
        (
            {
                "status": "healthy",
                "anomalies": [],
                "talents": {"failed": 0},
                "day": "20260101",
            },
            None,
        ),
        (
            {
                "status": "stale",
                "anomalies": [
                    {"kind": "activity_agents_missing"},
                    {"kind": "daily_agents_missing"},
                    {"kind": "talent_failure"},
                ],
                "talents": {"failed": 3},
                "day": "20260101",
            },
            {
                "status": "stale",
                "message": "Activity processing gap — meeting notes may be delayed",
            },
        ),
        (
            {
                "status": "stale",
                "anomalies": [
                    {"kind": "daily_agents_missing"},
                    {"kind": "talent_failure"},
                ],
                "talents": {"failed": 2},
                "day": "20260102",
            },
            {
                "status": "stale",
                "message": "Daily processing hasn't run yet",
            },
        ),
        (
            {
                "status": "warning",
                "anomalies": [{"kind": "talent_failure"}],
                "talents": {"failed": 1},
                "day": "20260101",
            },
            {"status": "warning", "message": "1 talent error today"},
        ),
        (
            {
                "status": "warning",
                "anomalies": [{"kind": "talent_failure"}] * 3,
                "talents": {"failed": 3},
                "day": "20260101",
            },
            {"status": "warning", "message": "3 talent errors today"},
        ),
        (
            {
                "status": "healthy",
                "anomalies": [{"kind": "segment_runs_missing"}],
                "talents": {"failed": 0},
                "day": "20260101",
            },
            None,
        ),
    ],
)
def test_status_message_priorities(summary, expected):
    assert pipeline_status_message(summary) == expected
