# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for segment-aware day completion."""

from __future__ import annotations

import importlib
import json
import logging
from pathlib import Path

import pytest

from solstone.think.cluster import cluster_segments
from solstone.think.pipeline_health import (
    SegmentProgress,
    classify_segment_completion,
    read_segment_backlog,
    read_segment_progress,
    segment_fully_sensed,
    segment_fully_thought,
)
from solstone.think.utils import updated_days

DAY = "20990401"
STREAM = "default"
SEGMENT = "090000_300"
SEGMENT_B = "091000_300"
SEGMENT_C = "092000_300"
SEGMENT_D = "093000_300"
SEGMENT_E = "094000_300"


@pytest.fixture
def segment_journal(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    return journal


def _write_jsonl(path: Path, events: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for event in events:
            handle.write(json.dumps(event) + "\n")


def _daily_complete(name: str = "alpha", ts: int = 1) -> dict:
    return {"event": "talent.complete", "ts": ts, "mode": "daily", "name": name}


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


def _dispatch(segment: str, name: str, ts: int = 1) -> dict:
    return _segment_event("talent.dispatch", segment, name, ts)


def _complete(segment: str, name: str, ts: int = 1) -> dict:
    return _segment_event("talent.complete", segment, name, ts, state="finish")


def _fail(segment: str, name: str, ts: int = 1) -> dict:
    return _segment_event("talent.fail", segment, name, ts, state="error")


def _skip(segment: str, name: str, reason: str, ts: int = 1) -> dict:
    return _segment_event("talent.skip", segment, name, ts, reason=reason)


def _sense_complete(segment: str, density: str = "active", ts: int = 1) -> dict:
    return _segment_event("sense.complete", segment, ts=ts, density=density)


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


def _seed_segment(
    journal: Path,
    day: str,
    segment: str,
    *,
    state: str = "analyzed",
    stream: str = STREAM,
) -> Path:
    segment_dir = journal / "chronicle" / day / stream / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    if state == "dropped":
        return segment_dir

    (segment_dir / "screen.webm").write_bytes(b"raw")
    if state == "analyzed":
        (segment_dir / "screen.jsonl").write_text(
            json.dumps({"raw": "screen.webm", "type": "screencast"})
            + "\n"
            + json.dumps({"timestamp": 0, "content": {}})
            + "\n",
            encoding="utf-8",
        )
    else:
        (segment_dir / "screen.jsonl").write_text(
            json.dumps({"raw": "screen.webm", "type": "screencast"}) + "\n",
            encoding="utf-8",
        )
        if state == "failed":
            (segment_dir / ".analyze_failed_screen").write_text(
                json.dumps({"reason": "fixture_failure"}) + "\n",
                encoding="utf-8",
            )
        elif state == "analyzing":
            (segment_dir / ".analyzing_screen").write_text(
                json.dumps({"modality": "screen"}) + "\n",
                encoding="utf-8",
            )
    return segment_dir


def _write_health(journal: Path, day: str, filename: str, events: list[dict]) -> Path:
    path = journal / "chronicle" / day / "health" / filename
    _write_jsonl(path, events)
    return path


def _patch_daily_main(monkeypatch, mod, applicable_units=None) -> None:
    if applicable_units is None:
        applicable_units = {("alpha", None)}

    monkeypatch.setattr(mod, "run_command", lambda cmd, day: True)
    monkeypatch.setattr(mod, "run_queued_command", lambda cmd, day, timeout=600: True)
    monkeypatch.setattr(
        mod,
        "run_daily_prompts",
        lambda **kwargs: (len(applicable_units), 0, [], applicable_units),
    )


def _run_daily_gate(journal: Path, day: str, monkeypatch) -> Path:
    mod = importlib.import_module("solstone.think.thinking")
    _patch_daily_main(monkeypatch, mod)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", day])
    mod.main()
    return journal / "chronicle" / day / "health"


def _build_all_gate_states(journal: Path, day: str) -> None:
    _seed_segment(journal, day, SEGMENT)
    _seed_segment(journal, day, SEGMENT_B, state="pending")
    _seed_segment(journal, day, SEGMENT_C)
    _seed_segment(journal, day, SEGMENT_D)
    _seed_segment(journal, day, SEGMENT_E)
    _write_health(journal, day, "001_daily.jsonl", [_daily_complete()])
    _write_health(
        journal,
        day,
        "002_segment.jsonl",
        _complete_segment_events(SEGMENT)
        + _complete_segment_events(SEGMENT_B)
        + [_sense_complete(SEGMENT_C, "active", 20)]
        + _complete_segment_events(SEGMENT_D)
        + [_dispatch(SEGMENT_D, "screen", 30)],
    )


def test_read_segment_progress_folds_latest_terminal_and_segments(
    segment_journal,
):
    _write_health(
        segment_journal,
        DAY,
        "001_segment.jsonl",
        [
            _sense_complete(SEGMENT, "active", 1),
            _dispatch(SEGMENT, "entities", 2),
            _complete(SEGMENT, "entities", 3),
            _skip(SEGMENT, "entities", "not_recommended", 4),
            _fail(SEGMENT, "entities", 5),
            _sense_complete(SEGMENT_B, "active", 1),
            _dispatch(SEGMENT_B, "entities", 2),
            _complete(SEGMENT_B, "entities", 3),
        ],
    )

    progress = read_segment_progress(DAY)

    assert progress[SEGMENT].sensed is True
    assert progress[SEGMENT].density == "active"
    assert "entities" not in progress[SEGMENT].completed
    assert progress[SEGMENT].dispatched == frozenset({"entities"})
    assert progress[SEGMENT_B].completed == frozenset({"entities"})


def test_read_segment_progress_tracks_latest_sense_density(segment_journal):
    _write_health(
        segment_journal,
        DAY,
        "001_segment.jsonl",
        [
            _sense_complete(SEGMENT, "active", 1),
            _sense_complete(SEGMENT, "idle", 2),
        ],
    )

    assert read_segment_progress(DAY)[SEGMENT].density == "idle"


def test_read_segment_progress_fail_closed_on_unexpected_error(
    monkeypatch,
    caplog,
):
    from solstone.think import pipeline_health

    def fail_day_path(*_args, **_kwargs):
        raise OSError("unreadable")

    monkeypatch.setattr(pipeline_health, "day_path", fail_day_path)
    caplog.set_level(logging.WARNING)

    assert pipeline_health.read_segment_progress(DAY) == {}
    assert "unexpected error reading segment progress" in caplog.text


@pytest.mark.parametrize("state", ["pending", "failed", "analyzing"])
def test_segment_fully_sensed_rejects_unfinished_states(state):
    assert segment_fully_sensed({"screen": state}) is False


def test_segment_fully_sensed_accepts_done_states():
    assert segment_fully_sensed({"screen": "analyzed", "audio": "purged"}) is True


def test_segment_fully_thought_idle_short_circuits():
    progress = SegmentProgress(
        sensed=True,
        density="idle",
        dispatched=frozenset({"sense"}),
        completed=frozenset({"sense"}),
        unconfigured=frozenset(),
    )

    assert segment_fully_thought(progress) == (True, None)


def test_segment_fully_thought_requires_floor_after_sense():
    progress = SegmentProgress(
        sensed=True,
        density="active",
        dispatched=frozenset({"sense"}),
        completed=frozenset({"sense"}),
        unconfigured=frozenset(),
    )

    assert segment_fully_thought(progress) == (False, "floor:entities")


def test_segment_fully_thought_ignores_skipped_not_dispatched_conditionals(
    segment_journal,
):
    _write_health(
        segment_journal,
        DAY,
        "001_segment.jsonl",
        _complete_segment_events(SEGMENT)
        + [_skip(SEGMENT, "speaker_attribution", "not_recommended", 30)],
    )

    progress = read_segment_progress(DAY)[SEGMENT]

    assert "speaker_attribution" not in progress.dispatched
    assert segment_fully_thought(progress) == (True, None)


def test_segment_fully_thought_does_not_require_rolling_talents():
    progress = SegmentProgress(
        sensed=True,
        density="active",
        dispatched=frozenset({"sense", "entities", "documents"}),
        completed=frozenset({"sense", "entities", "documents"}),
        unconfigured=frozenset(),
    )

    assert segment_fully_thought(progress) == (True, None)


def test_segment_fully_thought_allows_unconfigured_floor_talent():
    progress = SegmentProgress(
        sensed=True,
        density="active",
        dispatched=frozenset({"sense", "documents"}),
        completed=frozenset({"sense", "documents"}),
        unconfigured=frozenset({"entities"}),
    )

    assert segment_fully_thought(progress) == (True, None)


def test_segment_fully_thought_requires_dispatched_completion():
    progress = SegmentProgress(
        sensed=True,
        density="active",
        dispatched=frozenset({"sense", "entities", "documents", "screen"}),
        completed=frozenset({"sense", "entities", "documents"}),
        unconfigured=frozenset(),
    )

    assert segment_fully_thought(progress) == (False, "dispatched:screen")


def test_classifier_stats_and_gate_agree_on_all_gate_states(
    segment_journal,
    monkeypatch,
    caplog,
):
    from solstone.think.journal_stats import JournalStats

    day = "20990402"
    _build_all_gate_states(segment_journal, day)

    completion = classify_segment_completion(
        cluster_segments(day),
        read_segment_progress(day),
    )

    assert completion.not_thought == 3
    assert completion.not_sensed == 1
    assert completion.total == 5
    assert completion.blockers == [
        {
            "segment": SEGMENT_B,
            "dimension": "not_sensed",
            "detail": "screen=pending",
        },
        {
            "segment": SEGMENT_C,
            "dimension": "not_thought",
            "detail": "floor:entities",
        },
        {
            "segment": SEGMENT_D,
            "dimension": "not_thought",
            "detail": "dispatched:screen",
        },
        {
            "segment": SEGMENT_E,
            "dimension": "not_thought",
            "detail": "no_sense_complete",
        },
    ]

    stats = JournalStats().scan_day(
        day,
        str(segment_journal / "chronicle" / day),
    )
    assert stats["stats"]["segments_pending_think"] == completion.not_thought

    caplog.set_level(logging.INFO)
    health = _run_daily_gate(segment_journal, day, monkeypatch)

    assert not (health / "daily.updated").exists()
    assert str(completion.blockers) in caplog.text


def test_classify_segment_completion_latest_terminal_wins(segment_journal):
    day = "20990403"
    fail_then_complete = SEGMENT
    complete_then_fail = SEGMENT_B
    _seed_segment(segment_journal, day, fail_then_complete)
    _seed_segment(segment_journal, day, complete_then_fail)
    _write_health(
        segment_journal,
        day,
        "001_segment.jsonl",
        [
            _sense_complete(fail_then_complete, "active", 1),
            _complete(fail_then_complete, "documents", 4),
            _dispatch(fail_then_complete, "entities", 4),
            _fail(fail_then_complete, "entities", 5),
            _complete(fail_then_complete, "entities", 6),
            _sense_complete(complete_then_fail, "active", 1),
            _complete(complete_then_fail, "documents", 4),
            _dispatch(complete_then_fail, "entities", 4),
            _complete(complete_then_fail, "entities", 5),
            _fail(complete_then_fail, "entities", 6),
        ],
    )

    completion = classify_segment_completion(
        cluster_segments(day),
        read_segment_progress(day),
    )

    assert completion.not_thought == 1
    assert completion.blockers == [
        {
            "segment": complete_then_fail,
            "dimension": "not_thought",
            "detail": "floor:entities",
        }
    ]


def test_dropped_empty_modality_segment_is_not_counted(segment_journal):
    day = "20990404"
    _seed_segment(segment_journal, day, SEGMENT)
    _seed_segment(segment_journal, day, SEGMENT_B, state="dropped")
    _write_health(
        segment_journal,
        day,
        "001_segment.jsonl",
        _complete_segment_events(SEGMENT),
    )

    completion = classify_segment_completion(
        cluster_segments(day),
        read_segment_progress(day),
    )

    assert completion.total == 1
    assert completion.blockers == []


def test_read_segment_backlog_sums_updated_days(segment_journal):
    day_one = "20990405"
    day_two = "20990406"
    day_three = "20990407"

    _seed_segment(segment_journal, day_one, SEGMENT)
    _seed_segment(segment_journal, day_one, SEGMENT_B, state="pending")
    _write_health(
        segment_journal,
        day_one,
        "001_segment.jsonl",
        [_sense_complete(SEGMENT, "active", 1)],
    )

    _seed_segment(segment_journal, day_two, SEGMENT)
    _seed_segment(segment_journal, day_two, SEGMENT_B)
    _write_health(
        segment_journal,
        day_two,
        "001_segment.jsonl",
        _complete_segment_events(SEGMENT),
    )

    _seed_segment(segment_journal, day_three, SEGMENT)
    _write_health(
        segment_journal,
        day_three,
        "001_segment.jsonl",
        [_sense_complete(SEGMENT, "active", 1)],
    )

    for day in (day_one, day_two):
        health = segment_journal / "chronicle" / day / "health"
        health.mkdir(parents=True, exist_ok=True)
        (health / "stream.updated").touch()

    bound = tuple(updated_days())
    backlog = read_segment_backlog()

    assert backlog.days == bound
    assert backlog.days == (day_one, day_two)
    assert set(backlog.per_day) == {day_one, day_two}
    assert backlog.errors == ()
    assert backlog.not_thought == sum(
        completion.not_thought for completion in backlog.per_day.values()
    )
    assert backlog.not_sensed == sum(
        completion.not_sensed for completion in backlog.per_day.values()
    )
    assert backlog.total == sum(
        completion.total for completion in backlog.per_day.values()
    )
    assert backlog.not_thought == 2
    assert backlog.not_sensed == 1
    assert backlog.total == 4


def test_daily_marker_written_when_daily_and_segments_complete(
    segment_journal,
    monkeypatch,
):
    _seed_segment(segment_journal, DAY, SEGMENT)
    _write_health(segment_journal, DAY, "001_daily.jsonl", [_daily_complete()])
    _write_health(
        segment_journal, DAY, "002_segment.jsonl", _complete_segment_events(SEGMENT)
    )

    health = _run_daily_gate(segment_journal, DAY, monkeypatch)

    assert (health / "daily.updated").exists()


def test_downstream_failure_withholds_until_later_complete(
    segment_journal,
    monkeypatch,
):
    _seed_segment(segment_journal, DAY, SEGMENT)
    _write_health(segment_journal, DAY, "001_daily.jsonl", [_daily_complete()])
    events = _complete_segment_events(SEGMENT) + [
        _dispatch(SEGMENT, "screen", 20),
        _fail(SEGMENT, "screen", 21),
    ]
    _write_health(segment_journal, DAY, "002_segment.jsonl", events)

    health = _run_daily_gate(segment_journal, DAY, monkeypatch)
    assert not (health / "daily.updated").exists()

    _write_health(
        segment_journal,
        DAY,
        "003_segment.jsonl",
        [_complete(SEGMENT, "screen", 22)],
    )
    health = _run_daily_gate(segment_journal, DAY, monkeypatch)
    assert (health / "daily.updated").exists()


def test_unterminated_downstream_withholds(segment_journal, monkeypatch):
    _seed_segment(segment_journal, DAY, SEGMENT)
    _write_health(segment_journal, DAY, "001_daily.jsonl", [_daily_complete()])
    events = _complete_segment_events(SEGMENT) + [_dispatch(SEGMENT, "screen", 20)]
    _write_health(segment_journal, DAY, "002_segment.jsonl", events)

    health = _run_daily_gate(segment_journal, DAY, monkeypatch)

    assert not (health / "daily.updated").exists()


def test_not_fully_sensed_segment_withholds(segment_journal, monkeypatch, caplog):
    _seed_segment(segment_journal, DAY, SEGMENT, state="pending")
    _write_health(segment_journal, DAY, "001_daily.jsonl", [_daily_complete()])
    _write_health(
        segment_journal, DAY, "002_segment.jsonl", _complete_segment_events(SEGMENT)
    )
    caplog.set_level(logging.INFO)

    health = _run_daily_gate(segment_journal, DAY, monkeypatch)

    assert not (health / "daily.updated").exists()
    assert "not_sensed" in caplog.text
    assert "screen=pending" in caplog.text


def test_idle_segment_does_not_block_day(segment_journal, monkeypatch):
    _seed_segment(segment_journal, DAY, SEGMENT)
    _write_health(segment_journal, DAY, "001_daily.jsonl", [_daily_complete()])
    _write_health(
        segment_journal,
        DAY,
        "002_segment.jsonl",
        _complete_segment_events(SEGMENT, density="idle"),
    )

    health = _run_daily_gate(segment_journal, DAY, monkeypatch)

    assert (health / "daily.updated").exists()


def test_dropped_segment_directory_is_not_required(segment_journal, monkeypatch):
    _seed_segment(segment_journal, DAY, SEGMENT)
    _seed_segment(segment_journal, DAY, SEGMENT_B, state="dropped")
    _write_health(segment_journal, DAY, "001_daily.jsonl", [_daily_complete()])
    _write_health(
        segment_journal, DAY, "002_segment.jsonl", _complete_segment_events(SEGMENT)
    )

    health = _run_daily_gate(segment_journal, DAY, monkeypatch)

    assert (health / "daily.updated").exists()


def test_all_skip_rerun_writes_marker_and_leaves_updated_days(
    segment_journal,
    monkeypatch,
):
    _seed_segment(segment_journal, DAY, SEGMENT)
    health = segment_journal / "chronicle" / DAY / "health"
    _write_health(
        segment_journal,
        DAY,
        "001_daily.jsonl",
        [
            _daily_complete(ts=1),
            {"event": "talent.skip", "ts": 2, "mode": "daily", "name": "alpha"},
        ],
    )
    _write_health(
        segment_journal,
        DAY,
        "002_segment.jsonl",
        _complete_segment_events(SEGMENT)
        + [
            _skip(SEGMENT, "entities", "already_complete", 30),
            _skip(SEGMENT, "documents", "already_complete", 31),
        ],
    )
    health.mkdir(parents=True, exist_ok=True)
    (health / "stream.updated").touch()
    assert DAY in updated_days()

    health = _run_daily_gate(segment_journal, DAY, monkeypatch)

    assert (health / "daily.updated").exists()
    assert DAY not in updated_days()
    assert (health / "daily.updated").stat().st_mtime_ns >= (
        health / "stream.updated"
    ).stat().st_mtime_ns


def test_empty_segment_progress_withholds_and_logs_blocker(
    segment_journal,
    monkeypatch,
    caplog,
):
    mod = importlib.import_module("solstone.think.thinking")
    _seed_segment(segment_journal, DAY, SEGMENT)
    _write_health(segment_journal, DAY, "001_daily.jsonl", [_daily_complete()])
    monkeypatch.setattr(mod, "read_segment_progress", lambda day: {})
    _patch_daily_main(monkeypatch, mod)
    monkeypatch.setattr("sys.argv", ["sol think", "--day", DAY])
    caplog.set_level(logging.INFO)

    mod.main()
    health = segment_journal / "chronicle" / DAY / "health"

    assert not (health / "daily.updated").exists()
    assert SEGMENT in caplog.text
    assert "not_thought" in caplog.text
    assert "no_sense_complete" in caplog.text
