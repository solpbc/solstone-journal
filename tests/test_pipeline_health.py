# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for think.pipeline_health."""

from __future__ import annotations

import ast
import inspect
import json
import os
from datetime import datetime
from pathlib import Path

import pytest

from solstone.think.pipeline_health import (
    STUCK_FAIL_THRESHOLD,
    TerminalUnit,
    pipeline_status_message,
    read_backlog_view,
    read_completed_units,
    read_day_stuck,
    read_terminal_states,
    summarize_pipeline_day,
)


def _write_jsonl(path: Path, events: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for event in events:
            handle.write(json.dumps(event) + "\n")


def _segment_event(
    event: str,
    segment: str,
    name: str | None = None,
    ts: int = 1,
    **extra,
) -> dict:
    record = {"event": event, "ts": ts, "mode": "segment", "segment": segment}
    if name is not None:
        record["name"] = name
    record.update(extra)
    return record


def _dispatch(segment: str, name: str, ts: int = 1, **extra) -> dict:
    return _segment_event("talent.dispatch", segment, name, ts, **extra)


def _complete(segment: str, name: str, ts: int = 1, **extra) -> dict:
    return _segment_event("talent.complete", segment, name, ts, state="finish", **extra)


def _fail(segment: str, name: str, ts: int = 1, **extra) -> dict:
    return _segment_event("talent.fail", segment, name, ts, state="error", **extra)


def _sense_complete(
    segment: str,
    density: str = "active",
    ts: int = 1,
    **extra,
) -> dict:
    return _segment_event("sense.complete", segment, ts=ts, density=density, **extra)


def _complete_segment_events(segment: str, density: str = "active") -> list[dict]:
    events = [
        _dispatch(segment, "sense", 10),
        _complete(segment, "sense", 11),
        _sense_complete(segment, density, 12),
    ]
    if density != "idle":
        events.extend(
            [
                _dispatch(segment, "entities", 13),
                _complete(segment, "entities", 14),
                _dispatch(segment, "documents", 15),
                _complete(segment, "documents", 16),
            ]
        )
    return events


def _seed_screen_segment(
    journal: Path,
    day: str,
    segment: str,
) -> Path:
    segment_dir = journal / "chronicle" / day / "default" / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    (segment_dir / "screen.webm").write_bytes(b"raw")
    (segment_dir / "screen.jsonl").write_text(
        json.dumps({"raw": "screen.webm", "type": "screencast"})
        + "\n"
        + json.dumps({"timestamp": 0, "content": {}})
        + "\n",
        encoding="utf-8",
    )
    return segment_dir


def _seed_pending_segment(
    journal: Path,
    day: str,
    segment: str,
    *,
    stream: str = "default",
) -> Path:
    segment_dir = journal / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    (segment_dir / "screen.webm").write_bytes(b"raw")
    return segment_dir


def _write_failed_marker(
    segment_dir: Path,
    *,
    modality: str = "screen",
    reason: str = "marker_corrupt",
    failed_at: str = "1970-01-01T00:00:03Z",
) -> Path:
    marker = segment_dir / f".analyze_failed_{modality}"
    marker.write_text(
        json.dumps(
            {
                "modality": modality,
                "reason": reason,
                "failed_at": failed_at,
            }
        )
        + "\n",
        encoding="utf-8",
    )
    return marker


def _touch_marker(
    journal: Path,
    day: str,
    name: str,
    *,
    mtime_ms: int | None = None,
) -> Path:
    path = journal / "chronicle" / day / "health" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.touch()
    if mtime_ms is not None:
        seconds = mtime_ms / 1000
        os.utime(path, (seconds, seconds))
    return path


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


def test_read_terminal_states_latest_wins_and_preserves_expanded_keys(
    pipeline_journal,
):
    day = "20990208"
    base = pipeline_journal / "chronicle" / day / "health"
    _write_jsonl(
        base / "001_daily.jsonl",
        [
            {
                "event": "talent.fail",
                "ts": 1,
                "mode": "daily",
                "name": "alpha",
                "provider": "openai",
                "model": "gpt-4",
            },
            {"event": "talent.complete", "ts": 2, "mode": "daily", "name": "alpha"},
            {
                "event": "talent.fail",
                "ts": 3,
                "mode": "daily",
                "name": "alpha",
                "provider": "google",
                "model": "gemini-2.5-pro",
            },
            {
                "event": "talent.fail",
                "ts": 4,
                "mode": "daily",
                "name": "alpha",
                "provider": "anthropic",
                "model": "claude-opus-4-1",
            },
            {"event": "talent.fail", "ts": 10, "mode": "daily", "name": "beta"},
            {
                "event": "talent.complete",
                "ts": 10,
                "mode": "daily",
                "name": "beta",
            },
            _fail("090000_300", "entities", 5, stream="alpha"),
            _complete("091000_300", "entities", 6, stream="alpha"),
        ],
    )

    states = read_terminal_states(day)
    alpha = states[
        TerminalUnit(
            mode="daily",
            name="alpha",
            facet=None,
            stream=None,
            segment=None,
            activity=None,
        )
    ]
    beta = states[
        TerminalUnit(
            mode="daily",
            name="beta",
            facet=None,
            stream=None,
            segment=None,
            activity=None,
        )
    ]

    assert alpha.latest_event == "fail"
    assert alpha.latest_ts == 4
    assert alpha.trailing_fail_count == 2
    assert alpha.last_fail_ts == 4
    assert alpha.provider == "anthropic"
    assert alpha.model == "claude-opus-4-1"
    assert beta.latest_event == "complete"
    assert beta.trailing_fail_count == 0
    assert read_completed_units(day) == {("daily", "beta", None)}
    assert (
        states[
            TerminalUnit("segment", "entities", None, "alpha", "090000_300", None)
        ].latest_event
        == "fail"
    )
    assert (
        states[
            TerminalUnit("segment", "entities", None, "alpha", "091000_300", None)
        ].latest_event
        == "complete"
    )


def test_read_completed_units_returns_old_daily_tuple_shape_and_filters_scoped_units(
    pipeline_journal,
):
    day = "20990209"
    base = pipeline_journal / "chronicle" / day / "health"
    _write_jsonl(
        base / "001_daily.jsonl",
        [
            {
                "event": "talent.fail",
                "ts": 1,
                "mode": "daily",
                "name": "alpha",
            },
            {
                "event": "talent.complete",
                "ts": 1,
                "mode": "daily",
                "name": "alpha",
            },
            {
                "event": "talent.complete",
                "ts": 2,
                "mode": "daily",
                "name": "facet_newsletter",
                "facet": "work",
            },
            {
                "event": "talent.complete",
                "ts": 3,
                "mode": "segment",
                "name": "entities",
                "stream": "default",
                "segment": "090000_300",
            },
            {
                "event": "talent.complete",
                "ts": 4,
                "mode": "activity",
                "name": "summary",
                "facet": "work",
                "activity": "meeting_090000_300",
            },
        ],
    )

    completed = read_completed_units(day)

    assert completed == {
        ("daily", "alpha", None),
        ("daily", "facet_newsletter", "work"),
    }
    assert all(isinstance(unit, tuple) and len(unit) == 3 for unit in completed)


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


def test_no_output_failure_is_incomplete_and_summarized(pipeline_journal):
    day = "20990107"
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "1_daily.jsonl",
        [
            {
                "event": "talent.fail",
                "mode": "daily",
                "name": "alpha",
                "use_id": "a-1",
                "state": "error",
                "reason_code": "no_output",
                "ts": 1,
            },
            {
                "event": "talent.complete",
                "mode": "daily",
                "name": "beta",
                "use_id": "b-1",
                "state": "finish",
                "ts": 1,
            },
        ],
    )

    assert read_completed_units(day) == {("daily", "beta", None)}

    summary = summarize_pipeline_day(day)
    assert summary["status"] == "warning"
    assert summary["talents"]["completed"] == 1
    assert summary["talents"]["failed"] == 1
    assert summary["talents"]["failed_list"] == [
        {"mode": "daily", "name": "alpha", "use_id": "a-1", "state": "error"}
    ]
    assert summary["anomalies"] == [
        {
            "kind": "talent_failure",
            "mode": "daily",
            "name": "alpha",
            "use_id": "a-1",
            "state": "error",
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


def test_segment_runs_missing_elevates(pipeline_journal):
    day = "20990105"
    segment = "120000_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [_sense_complete(segment, "active", 1)],
    )

    summary = summarize_pipeline_day(day)

    assert summary["status"] == "stale"
    assert {
        "kind": "segment_runs_missing",
        "not_thought": 1,
        "not_sensed": 0,
        "total": 1,
    } in summary["anomalies"]
    assert pipeline_status_message(summary) == {
        "status": "stale",
        "message": "1 segment awaiting thinking",
    }


def test_segment_runs_missing_ignores_idle_and_fully_thought_segments(
    pipeline_journal,
):
    day = "20990106"
    idle_segment = "120000_300"
    complete_segment = "121000_300"
    _seed_screen_segment(pipeline_journal, day, idle_segment)
    _seed_screen_segment(pipeline_journal, day, complete_segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        _complete_segment_events(idle_segment, density="idle")
        + _complete_segment_events(complete_segment),
    )

    summary = summarize_pipeline_day(day)

    assert summary["status"] == "healthy"
    assert not any(
        anomaly["kind"] == "segment_runs_missing" for anomaly in summary["anomalies"]
    )


def test_segment_completion_fold_failure_elevates_status(
    pipeline_journal,
    monkeypatch,
):
    from solstone.think import pipeline_health

    day = "20990107"
    (pipeline_journal / "chronicle" / day / "health").mkdir(parents=True)

    def fail_classify(*_args, **_kwargs):
        raise RuntimeError("fold exploded")

    monkeypatch.setattr(pipeline_health, "classify_segment_completion", fail_classify)

    summary = summarize_pipeline_day(day)

    assert summary["status"] == "stale"
    assert {"kind": "segment_runs_missing", "error": "fold_failed"} in summary[
        "anomalies"
    ]
    assert pipeline_status_message(summary) == {
        "status": "stale",
        "message": "Segment thinking status unavailable",
    }


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


def test_read_day_stuck_returns_true_for_genuine_stuck_day(pipeline_journal):
    day = "20990701"
    segment = "110000_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    last_fail_ts = 3000
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [
            _sense_complete(segment, "active", 1, stream="default"),
            _dispatch(segment, "documents", 2, stream="default"),
            _complete(segment, "documents", 3, stream="default"),
            _dispatch(segment, "entities", 4, stream="default"),
            _fail(segment, "entities", 1000, stream="default"),
            _fail(segment, "entities", 2000, stream="default"),
            _fail(segment, "entities", last_fail_ts, stream="default"),
        ],
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=last_fail_ts)

    assert read_day_stuck(day) is True


def test_read_day_stuck_returns_false_for_pending_day(pipeline_journal):
    day = "20990702"
    segment = "120000_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [
            _sense_complete(segment, "active", 1, stream="default"),
            _dispatch(segment, "documents", 2, stream="default"),
            _complete(segment, "documents", 3, stream="default"),
            _dispatch(segment, "entities", 4, stream="default"),
            _fail(segment, "entities", 1000, stream="default"),
            _fail(segment, "entities", 2000, stream="default"),
        ],
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    assert read_day_stuck(day) is False


def test_read_day_stuck_returns_false_for_missing_day(pipeline_journal):
    assert read_day_stuck("20200101") is False


def test_read_backlog_view_reports_complete_pending_stuck_and_why_axis(
    pipeline_journal,
):
    complete_day = "20990305"
    pending_day = "20990304"
    stuck_day = "20990303"

    _touch_marker(pipeline_journal, complete_day, "stream.updated", mtime_ms=1000)
    _touch_marker(pipeline_journal, complete_day, "daily.updated", mtime_ms=1000)

    never_segment = "100000_300"
    dispatched_segment = "100500_300"
    not_sensed_segment = "101000_300"
    for segment in (never_segment, dispatched_segment):
        _seed_screen_segment(pipeline_journal, pending_day, segment)
    _seed_pending_segment(pipeline_journal, pending_day, not_sensed_segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / pending_day / "health" / "001_segment.jsonl",
        [
            _sense_complete(never_segment, "active", 1),
            _sense_complete(dispatched_segment, "active", 1),
            _dispatch(dispatched_segment, "entities", 2),
            _complete(dispatched_segment, "entities", 3),
            _dispatch(dispatched_segment, "documents", 4),
            _complete(dispatched_segment, "documents", 5),
            _dispatch(dispatched_segment, "screen", 6),
            {
                "event": "talent.fail",
                "ts": 7,
                "mode": "daily",
                "name": "newsletter",
                "provider": "openai",
                "model": "gpt-5",
            },
        ],
    )
    _touch_marker(pipeline_journal, pending_day, "stream.updated", mtime_ms=8000)

    stuck_segment = "110000_300"
    _seed_screen_segment(pipeline_journal, stuck_day, stuck_segment)
    stuck_fail_ts = 3000
    _write_jsonl(
        pipeline_journal / "chronicle" / stuck_day / "health" / "001_segment.jsonl",
        [
            _sense_complete(stuck_segment, "active", 1, stream="default"),
            _dispatch(stuck_segment, "documents", 2, stream="default"),
            _complete(stuck_segment, "documents", 3, stream="default"),
            _dispatch(stuck_segment, "entities", 4, stream="default"),
            _fail(stuck_segment, "entities", 1000, stream="default"),
            _fail(stuck_segment, "entities", 2000, stream="default"),
            _fail(
                stuck_segment,
                "entities",
                stuck_fail_ts,
                stream="default",
                provider="anthropic",
                model="claude-opus-4-1",
            ),
        ],
    )
    _touch_marker(
        pipeline_journal,
        stuck_day,
        "stream.updated",
        mtime_ms=stuck_fail_ts,
    )

    view = read_backlog_view(window=3)
    by_day = {day.day: day for day in view.days}

    assert by_day[complete_day].state == "complete"
    assert by_day[pending_day].state == "pending"
    assert by_day[pending_day].segments == 3
    assert by_day[pending_day].not_sensed == 1
    assert by_day[pending_day].units == 3
    assert {unit.why for unit in by_day[pending_day].why} == {
        "never_attempted",
        "sensed_not_thought",
        "failed",
    }
    assert by_day[stuck_day].state == "stuck"
    assert by_day[stuck_day].units == 1
    stuck_unit = by_day[stuck_day].why[0]
    assert stuck_unit.why == "failed"
    assert stuck_unit.provider == "anthropic"
    assert stuck_unit.model == "claude-opus-4-1"
    assert stuck_unit.trailing_fail_count == STUCK_FAIL_THRESHOLD
    assert stuck_unit.stuck is True
    assert view.pending_days == 1
    assert view.stuck_days == 1
    assert view.oldest_pending_day == stuck_day
    assert read_backlog_view(window=3) == view


@pytest.mark.parametrize(
    ("day", "fail_count", "stream_mtime_ms", "expected_state", "expected_stuck"),
    [
        ("20990401", STUCK_FAIL_THRESHOLD - 1, 3000, "pending", False),
        ("20990402", STUCK_FAIL_THRESHOLD, 3000, "stuck", True),
        ("20990403", STUCK_FAIL_THRESHOLD, 4000, "pending", False),
    ],
)
def test_read_backlog_view_stuck_threshold_and_stream_updated_boundary(
    pipeline_journal,
    day,
    fail_count,
    stream_mtime_ms,
    expected_state,
    expected_stuck,
):
    segment = "120000_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    events = [
        _sense_complete(segment, "active", 1, stream="default"),
        _dispatch(segment, "documents", 2, stream="default"),
        _complete(segment, "documents", 3, stream="default"),
        _dispatch(segment, "entities", 4, stream="default"),
    ]
    events.extend(
        _fail(segment, "entities", 1000 + idx * 1000, stream="default")
        for idx in range(fail_count)
    )
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        events,
    )
    _touch_marker(
        pipeline_journal,
        day,
        "stream.updated",
        mtime_ms=stream_mtime_ms,
    )

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == expected_state
    assert backlog_day.why[0].stuck is expected_stuck


def test_read_backlog_view_promotes_corrupt_raw_marker_to_stuck(pipeline_journal):
    day = "20990404"
    segment = "121000_300"
    segment_dir = _seed_pending_segment(pipeline_journal, day, segment)
    _write_failed_marker(segment_dir)
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    view = read_backlog_view(window=1)
    backlog_day = view.days[0]

    assert backlog_day.state == "stuck"
    assert backlog_day.reason == "corrupt_raw"
    assert view.stuck_days == 1
    assert view.pending_days == 0
    assert backlog_day.segments > 0
    assert backlog_day.state != "complete"
    assert backlog_day.units == 1
    unit = backlog_day.why[0]
    assert unit.why == "corrupt_raw"
    assert unit.stuck is True
    assert unit.name == "screen"
    assert unit.last_fail_ts == 3000


def test_read_backlog_view_promotes_named_stream_corrupt_raw_marker(
    pipeline_journal,
):
    day = "20990405"
    segment = "121500_300"
    segment_dir = _seed_pending_segment(
        pipeline_journal,
        day,
        segment,
        stream="import.apple",
    )
    _write_failed_marker(segment_dir)
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == "stuck"
    assert backlog_day.reason == "corrupt_raw"
    assert backlog_day.why[0].stream == "import.apple"


@pytest.mark.parametrize("reason", ["stale", "exit_7"])
def test_read_backlog_view_does_not_promote_non_corrupt_failed_markers(
    pipeline_journal,
    reason,
):
    day = "20990406"
    segment = "122000_300"
    segment_dir = _seed_pending_segment(pipeline_journal, day, segment)
    _write_failed_marker(segment_dir, reason=reason)
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == "pending"
    assert backlog_day.reason != "corrupt_raw"
    assert backlog_day.reason is None
    assert backlog_day.units == 0


def test_read_backlog_view_revives_corrupt_raw_when_stream_is_newer(
    pipeline_journal,
):
    day = "20990407"
    segment = "122500_300"
    segment_dir = _seed_pending_segment(pipeline_journal, day, segment)
    _write_failed_marker(segment_dir)
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=4000)

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == "pending"
    assert backlog_day.reason is None
    assert backlog_day.segments > 0
    assert backlog_day.state != "complete"
    assert backlog_day.units == 0


def test_read_backlog_view_malformed_failed_marker_stays_pending(
    pipeline_journal,
):
    day = "20990408"
    segment = "123000_300"
    segment_dir = _seed_pending_segment(pipeline_journal, day, segment)
    (segment_dir / ".analyze_failed_screen").write_text(
        "{not-json\n",
        encoding="utf-8",
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == "pending"
    assert backlog_day.reason is None
    assert backlog_day.units == 0


def test_read_backlog_view_repeated_talent_fail_reason_is_failing_step(
    pipeline_journal,
):
    day = "20990409"
    segment = "123500_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [
            _sense_complete(segment, "active", 1, stream="default"),
            _dispatch(segment, "documents", 2, stream="default"),
            _complete(segment, "documents", 3, stream="default"),
            _dispatch(segment, "entities", 4, stream="default"),
            _fail(segment, "entities", 1000, stream="default"),
            _fail(segment, "entities", 2000, stream="default"),
            _fail(segment, "entities", 3000, stream="default"),
        ],
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == "stuck"
    assert backlog_day.reason == "failing_step"
    assert backlog_day.why[0].why == "failed"
    assert backlog_day.why[0].stuck is True


def test_read_backlog_view_no_config_floor_skip_does_not_stick(
    pipeline_journal,
):
    day = "20990410"
    segment = "124000_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [
            _sense_complete(segment, "active", 1, stream="default"),
            _segment_event(
                "talent.skip",
                segment,
                "documents",
                2,
                stream="default",
                reason="no_config",
            ),
            _segment_event(
                "talent.skip",
                segment,
                "entities",
                3,
                stream="default",
                reason="no_config",
            ),
        ],
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == "complete"
    assert backlog_day.reason is None
    assert backlog_day.units == 0
    assert not any(unit.stuck for unit in backlog_day.why)


def test_read_backlog_view_provider_model_failure_remains_failing_step(
    pipeline_journal,
):
    from solstone.think import pipeline_health

    day = "20990411"
    segment = "124500_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [
            _sense_complete(segment, "active", 1, stream="default"),
            _dispatch(segment, "documents", 2, stream="default"),
            _complete(segment, "documents", 3, stream="default"),
            _dispatch(segment, "entities", 4, stream="default"),
            _fail(segment, "entities", 1000, stream="default"),
            _fail(segment, "entities", 2000, stream="default"),
            _fail(
                segment,
                "entities",
                3000,
                stream="default",
                provider="openai",
                model="gpt-5",
            ),
        ],
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    backlog_day = read_backlog_view(window=1).days[0]
    imported_modules = []
    for node in ast.walk(ast.parse(inspect.getsource(pipeline_health))):
        if isinstance(node, ast.Import):
            imported_modules.extend(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module:
            imported_modules.append(node.module)

    assert backlog_day.state == "stuck"
    assert backlog_day.reason == "failing_step"
    assert backlog_day.reason != "provider_down"
    unit = backlog_day.why[0]
    assert unit.provider == "openai"
    assert unit.model == "gpt-5"
    assert unit.stuck is True
    assert not any("supervisor" in module for module in imported_modules)


def test_read_backlog_view_corrupt_raw_reason_wins_over_failing_step(
    pipeline_journal,
):
    day = "20990412"
    corrupt_segment = "125000_300"
    failing_segment = "125500_300"
    corrupt_dir = _seed_pending_segment(pipeline_journal, day, corrupt_segment)
    _write_failed_marker(corrupt_dir)
    _seed_screen_segment(pipeline_journal, day, failing_segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [
            _sense_complete(failing_segment, "active", 1, stream="default"),
            _dispatch(failing_segment, "documents", 2, stream="default"),
            _complete(failing_segment, "documents", 3, stream="default"),
            _dispatch(failing_segment, "entities", 4, stream="default"),
            _fail(failing_segment, "entities", 1000, stream="default"),
            _fail(failing_segment, "entities", 2000, stream="default"),
            _fail(failing_segment, "entities", 3000, stream="default"),
        ],
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=3000)

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == "stuck"
    assert backlog_day.reason == "corrupt_raw"
    assert {unit.why for unit in backlog_day.why} == {"corrupt_raw", "failed"}


def test_read_backlog_view_dispatch_without_terminal_is_pending_not_in_progress(
    pipeline_journal,
):
    day = "20990404"
    segment = "123000_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [
            {"event": "run.start", "ts": 1, "mode": "segment", "segment": segment},
            _sense_complete(segment, "active", 2, stream="default"),
            _dispatch(segment, "entities", 3, stream="default"),
            _complete(segment, "entities", 4, stream="default"),
            _dispatch(segment, "documents", 5, stream="default"),
            _complete(segment, "documents", 6, stream="default"),
            _dispatch(segment, "screen", 7, stream="default"),
        ],
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=8000)

    backlog_day = read_backlog_view(window=1).days[0]

    assert backlog_day.state == "pending"
    assert backlog_day.state != "in_progress"
    assert [unit.why for unit in backlog_day.why] == ["sensed_not_thought"]


def test_read_backlog_view_unknown_day_is_retained(pipeline_journal, monkeypatch):
    from solstone.think import pipeline_health

    day = "20990501"
    (pipeline_journal / "chronicle" / day / "health").mkdir(parents=True)
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=1000)

    def fail_terminal_states(target_day: str):
        raise RuntimeError(f"boom {target_day}")

    monkeypatch.setattr(pipeline_health, "read_terminal_states", fail_terminal_states)

    view = read_backlog_view(window=1)

    assert view.days[0].day == day
    assert view.days[0].state == "unknown"
    assert view.days[0].error is not None
    assert view.errors == (view.days[0].error,)
    assert view.errors[0].stage == "terminal_states"


def test_historical_failures_with_latest_complete_are_not_pending(pipeline_journal):
    day = "20990601"
    segment = "130000_300"
    _seed_screen_segment(pipeline_journal, day, segment)
    failures = [_fail(segment, "entities", ts, stream="default") for ts in range(1, 8)]
    _write_jsonl(
        pipeline_journal / "chronicle" / day / "health" / "001_segment.jsonl",
        [
            _sense_complete(segment, "active", 1, stream="default"),
            _dispatch(segment, "documents", 2, stream="default"),
            _complete(segment, "documents", 3, stream="default"),
            _dispatch(segment, "entities", 4, stream="default"),
            *failures,
            _complete(segment, "entities", 20, stream="default"),
        ],
    )
    _touch_marker(pipeline_journal, day, "stream.updated", mtime_ms=21000)

    view = read_backlog_view(window=1)

    assert view.pending_days == 0
    assert view.stuck_days == 0
    assert view.days[0].state == "complete"
    assert view.days[0].segments == 0
    assert view.days[0].units == 0


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
                "status": "stale",
                "anomalies": [
                    {
                        "kind": "segment_runs_missing",
                        "not_thought": 2,
                        "not_sensed": 0,
                        "total": 5,
                    }
                ],
                "talents": {"failed": 0},
                "day": "20260101",
            },
            {"status": "stale", "message": "2 segments awaiting thinking"},
        ),
        (
            {
                "status": "stale",
                "anomalies": [{"kind": "segment_runs_missing", "error": "fold_failed"}],
                "talents": {"failed": 0},
                "day": "20260101",
            },
            {
                "status": "stale",
                "message": "Segment thinking status unavailable",
            },
        ),
    ],
)
def test_status_message_priorities(summary, expected):
    assert pipeline_status_message(summary) == expected
