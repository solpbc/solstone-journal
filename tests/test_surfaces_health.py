# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
from datetime import UTC, datetime, timedelta
from pathlib import Path
from unittest.mock import MagicMock

import pytest

from solstone.think.pipeline_health import SegmentBacklog, SegmentCompletion
from solstone.think.processing import (
    DISPLAY_POWERSAVE_UNAVAILABLE,
    DRAIN_STATE_NO_CONDITION,
    DRAIN_STATE_NO_ENGINE,
    DRAIN_STATE_REALTIME,
    DRAIN_STATE_WAITING,
    DRAIN_STATE_WINDOW_OPEN,
    ConditionState,
    DisplayPowersaveReading,
    DisplayPowersaveSettings,
    GateSettings,
    GateState,
    ProcessingSettings,
    TimeWindowSettings,
    format_awaiting_analysis,
)
from solstone.think.surfaces import health as health_surface

_SPEC_POINTER = "solstone/think/surfaces/health.py"


def _configure_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")
    config_dir = tmp_path / "config"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "journal.json").write_text(
        json.dumps({"setup": {"completed_at": 1}}),
        encoding="utf-8",
    )


def _set_now(monkeypatch: pytest.MonkeyPatch, value: datetime) -> None:
    assert value.tzinfo == UTC
    monkeypatch.setattr(health_surface, "_resolve_now", lambda: value)


def _minimal_facet_tree(
    tmp_path: Path,
    *,
    facets: tuple[str, ...] = ("work",),
    muted_facets: tuple[str, ...] = (),
) -> None:
    muted = set(muted_facets)
    for facet in facets:
        facet_dir = tmp_path / "facets" / facet
        facet_dir.mkdir(parents=True, exist_ok=True)
        (facet_dir / "activities").mkdir(exist_ok=True)
        (facet_dir / "facet.json").write_text(
            json.dumps(
                {
                    "title": facet.title(),
                    "description": "",
                    "color": "",
                    "emoji": "",
                    "muted": facet in muted,
                }
            ),
            encoding="utf-8",
        )


def _write_entity(
    tmp_path: Path,
    entity_id: str,
    name: str,
    *,
    entity_type: str = "Person",
) -> None:
    entity_dir = tmp_path / "entities" / entity_id
    entity_dir.mkdir(parents=True, exist_ok=True)
    (entity_dir / "entity.json").write_text(
        json.dumps({"id": entity_id, "name": name, "type": entity_type}),
        encoding="utf-8",
    )


def _utc_dt(day: str, hour: int = 12, minute: int = 0) -> datetime:
    return datetime.strptime(
        f"{day} {hour:02d}:{minute:02d}:00",
        "%Y%m%d %H:%M:%S",
    ).replace(tzinfo=UTC)


def _utc_ms(day: str, hour: int = 12, minute: int = 0) -> int:
    return int(_utc_dt(day, hour, minute).timestamp() * 1000)


def _iso_utc(day: str, hour: int = 12, minute: int = 0) -> str:
    return _utc_dt(day, hour, minute).isoformat().replace("+00:00", "Z")


def _activity_record(
    day: str,
    record_id: str,
    *,
    activity: str = "meeting",
    segments: list[str] | None = None,
    participation: object = None,
    story: object = None,
    edits: list[dict[str, object]] | None = None,
    source: str = "user",
    hidden: bool = False,
    created_at: int | None = None,
    start: str | None = None,
    cancelled: bool = False,
    commitments: list[dict[str, object]] | None = None,
    closures: list[dict[str, object]] | None = None,
) -> dict[str, object]:
    record: dict[str, object] = {
        "id": record_id,
        "activity": activity,
        "title": record_id,
        "description": record_id,
        "segments": segments or [],
        "created_at": created_at if created_at is not None else _utc_ms(day),
        "source": source,
        "hidden": hidden,
        "edits": edits or [],
    }
    if participation is not None:
        record["participation"] = participation
    if story is not None:
        record["story"] = story
    if start is not None:
        record["start"] = start
    if cancelled:
        record["cancelled"] = True
    if commitments is not None:
        record["commitments"] = commitments
    if closures is not None:
        record["closures"] = closures
    return record


def _append_jsonl(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(payload) + "\n")


def _write_activity(
    tmp_path: Path,
    facet: str,
    day: str,
    payload: dict[str, object],
) -> None:
    _append_jsonl(tmp_path / "facets" / facet / "activities" / f"{day}.jsonl", payload)


def _write_talent_day(
    tmp_path: Path,
    day: str,
    *rows: dict[str, object],
) -> None:
    path = tmp_path / "talents" / f"{day}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(row) + "\n" for row in rows),
        encoding="utf-8",
    )


def _write_indexer_db(tmp_path: Path, dt: datetime) -> int:
    path = tmp_path / "indexer" / "journal.sqlite"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("", encoding="utf-8")
    ts = dt.timestamp()
    os.utime(path, (ts, ts))
    return path.stat().st_mtime_ns // 1_000_000


def _segment_backlog(
    per_day_counts: dict[str, int],
    *,
    not_sensed: int = 0,
    errors: tuple[str, ...] = (),
) -> SegmentBacklog:
    per_day = {
        day: SegmentCompletion(
            blockers=[],
            not_sensed=0,
            not_thought=not_thought,
            total=max(not_thought, 1),
            capped=0,
            exhausted=(),
        )
        for day, not_thought in per_day_counts.items()
    }
    return SegmentBacklog(
        days=tuple(per_day_counts),
        not_thought=sum(per_day_counts.values()),
        not_sensed=not_sensed,
        total=max(
            sum(completion.total for completion in per_day.values()),
            sum(per_day_counts.values()) + not_sensed,
        ),
        per_day=per_day,
        errors=errors,
    )


def _processing_settings(mode: str) -> ProcessingSettings:
    return ProcessingSettings(
        mode=mode,
        gate=GateSettings(
            time_window=TimeWindowSettings(
                enabled=True,
                start="02:00",
                end="06:00",
            ),
            display_powersave=DisplayPowersaveSettings(enabled=False),
        ),
    )


def _stub_display_powersave(
    monkeypatch: pytest.MonkeyPatch,
    *,
    reading=DISPLAY_POWERSAVE_UNAVAILABLE,
    detectable: bool = False,
) -> None:
    monkeypatch.setattr(health_surface, "last_display_powersave", lambda: reading)
    monkeypatch.setattr(
        health_surface,
        "display_powersave_detectable",
        lambda: detectable,
    )


def test_summary_metrics_from_controlled_range(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    indexer_ms = _write_indexer_db(tmp_path, _utc_dt("20260410", 11, 0))
    _write_talent_day(
        tmp_path,
        "20260410",
        {
            "use_id": "1",
            "name": "flow",
            "day": "20260410",
            "facet": None,
            "ts": _utc_ms("20260410", 9),
            "status": "failed",
        },
        {
            "use_id": "2",
            "name": "flow",
            "day": "20260410",
            "facet": None,
            "ts": _utc_ms("20260410", 8),
            "status": "completed",
        },
    )
    _write_talent_day(
        tmp_path,
        "20260409",
        {
            "use_id": "3",
            "name": "flow",
            "day": "20260409",
            "facet": None,
            "ts": _utc_ms("20260409", 15),
            "status": "completed",
        },
    )
    _write_activity(
        tmp_path,
        "work",
        "20260409",
        _activity_record(
            "20260409",
            "meeting_100000_3600",
            segments=["100000_3600"],
            participation=[{"entity_id": "alex"}],
            story={"body": "Discussed launch."},
            edits=[{"actor": "cli:update", "fields": ["details"]}],
            created_at=_utc_ms("20260409", 10),
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "meeting_140000_1800",
            segments=["140000_1800"],
            edits=[{"actor": "system:story", "fields": ["story"]}],
            created_at=_utc_ms("20260410", 14),
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "anticipated_090000_1800",
            source="anticipated",
            start=_iso_utc("20260410", 9),
            created_at=_utc_ms("20260410", 8, 30),
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "hidden_080000_1800",
            hidden=True,
            participation=[{"entity_id": "ignored"}],
            story={"body": "Ignored."},
            edits=[{"actor": "cli:update", "fields": ["details"]}],
            created_at=_utc_ms("20260410", 8),
        ),
    )

    report = health_surface.for_range("20260409", "20260410")

    assert report.range == ("20260409", "20260410")
    assert report.capture_health.hours_with_capture == 2
    assert report.capture_health.hours_total == 48
    assert report.capture_health.coverage_ratio is None
    assert report.capture_health.last_segment_at == _utc_ms("20260410", 14, 30)
    assert report.capture_health.facets_with_recent_capture == ("work",)
    assert report.capture_health.facets_silent_24h == ()
    assert report.synthesis_health.activities_count == 3
    assert report.synthesis_health.activities_with_participation == 1
    assert report.synthesis_health.activities_with_story == 1
    assert report.synthesis_health.activities_user_edited == 1
    assert report.synthesis_health.activities_anticipated_unfilled == 1
    assert report.synthesis_health.talent_run_failures_24h == 1
    assert report.synthesis_health.indexer_last_rebuild_at == indexer_ms


def test_activities_with_participation_counts_truthy_field(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record("20260410", "one", participation=[{"entity_id": "a"}]),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record("20260410", "two", participation=[]),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record("20260410", "three", hidden=True, participation=[{"a": 1}]),
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.activities_with_participation == 1


def test_hours_total_is_24_times_range_days(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    report = health_surface.for_range("20260408", "20260410")

    assert report.capture_health.hours_total == 72


def test_facets_partition_into_recent_or_silent_24h(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path, facets=("home", "work"))
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record("20260410", "work_100000_1800", segments=["100000_1800"]),
    )

    report = health_surface.summary("20260410")

    assert report.facets == ("home", "work")
    assert report.capture_health.facets_with_recent_capture == ("work",)
    assert report.capture_health.facets_silent_24h == ("home",)
    assert sorted(
        report.capture_health.facets_with_recent_capture
        + report.capture_health.facets_silent_24h
    ) == list(report.facets)


def test_profile_entities_total_lives_on_consumer_signal(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_entity(tmp_path, "alex", "Alex")
    _write_entity(tmp_path, "blair", "Blair")

    report = health_surface.summary("20260410")

    assert report.consumer_signal.profile_entities_total == 2
    assert not hasattr(report.synthesis_health, "profile_entities_total")


def test_segment_backlog_health_counts_days_with_backlog(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({"20260408": 2, "20260409": 0, "20260410": 1}),
    )

    report = health_surface.summary("20260410")

    assert report.segment_backlog.not_thought == 3
    assert report.segment_backlog.days_with_backlog == 2


def test_segment_backlog_health_caught_up(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}),
    )

    report = health_surface.summary("20260410")

    assert report.segment_backlog.not_thought == 0
    assert report.segment_backlog.days_with_backlog == 0
    assert report.segment_backlog.errors == ()


def test_segment_backlog_health_preserves_errors(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}, errors=("20260101",)),
    )

    report = health_surface.summary("20260410")

    assert report.segment_backlog.errors == ("20260101",)


@pytest.mark.parametrize(
    ("not_sensed", "not_thought", "expected_count"),
    [(3, 0, 3), (2, 1, 3)],
)
def test_segment_backlog_deferred_awaiting_analysis_uses_unsensed(
    monkeypatch,
    not_sensed: int,
    not_thought: int,
    expected_count: int,
) -> None:
    per_day = {"20260410": not_thought} if not_thought else {}
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog(per_day, not_sensed=not_sensed),
    )
    monkeypatch.setattr(
        health_surface,
        "load_processing_settings",
        lambda: _processing_settings("deferred"),
    )
    monkeypatch.setattr(
        health_surface,
        "evaluate_drain_gate",
        lambda settings, now, reading: GateState(open=False, conditions={}),
    )
    monkeypatch.setattr(health_surface, "read_last_drained_at", lambda: None)
    monkeypatch.setattr(
        "solstone.think.models.no_thinking_engine_chosen", lambda: False
    )
    _stub_display_powersave(monkeypatch)

    backlog = health_surface._build_segment_backlog_health()

    assert backlog.not_sensed == not_sensed
    assert backlog.not_thought == not_thought
    assert backlog.awaiting_analysis_text == format_awaiting_analysis(expected_count)


def test_segment_backlog_realtime_omits_awaiting_analysis_text(monkeypatch) -> None:
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({"20260410": 2}, not_sensed=3),
    )
    monkeypatch.setattr(
        health_surface,
        "load_processing_settings",
        lambda: _processing_settings("realtime"),
    )
    monkeypatch.setattr(
        health_surface,
        "evaluate_drain_gate",
        lambda settings, now, reading: GateState(open=True, conditions={}),
    )
    monkeypatch.setattr(health_surface, "read_last_drained_at", lambda: None)
    monkeypatch.setattr(
        "solstone.think.models.no_thinking_engine_chosen", lambda: False
    )
    _stub_display_powersave(monkeypatch)

    backlog = health_surface._build_segment_backlog_health()

    assert backlog.awaiting_analysis_text is None
    assert backlog.drain_state == DRAIN_STATE_REALTIME


@pytest.mark.parametrize(
    ("gate", "expected_state"),
    [
        (
            GateState(
                open=True,
                conditions={"time_window": ConditionState(True, True, True)},
            ),
            DRAIN_STATE_WINDOW_OPEN,
        ),
        (
            GateState(
                open=False,
                conditions={"time_window": ConditionState(True, True, False)},
            ),
            DRAIN_STATE_WAITING,
        ),
        (
            GateState(
                open=False,
                conditions={
                    "time_window": ConditionState(False, True, False),
                    "display_powersave": ConditionState(True, False, False),
                },
            ),
            DRAIN_STATE_NO_CONDITION,
        ),
    ],
)
def test_segment_backlog_drain_state_tokens(monkeypatch, gate, expected_state) -> None:
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}),
    )
    monkeypatch.setattr(
        health_surface,
        "load_processing_settings",
        lambda: _processing_settings("deferred"),
    )
    monkeypatch.setattr(
        health_surface,
        "evaluate_drain_gate",
        lambda settings, now, reading: gate,
    )
    monkeypatch.setattr(health_surface, "read_last_drained_at", lambda: None)
    monkeypatch.setattr(
        "solstone.think.models.no_thinking_engine_chosen", lambda: False
    )
    _stub_display_powersave(monkeypatch)

    backlog = health_surface._build_segment_backlog_health()

    assert backlog.drain_state == expected_state


def test_segment_backlog_no_engine_wins_before_realtime(monkeypatch) -> None:
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({"20260410": 2}, not_sensed=3),
    )
    monkeypatch.setattr(
        health_surface,
        "load_processing_settings",
        lambda: _processing_settings("realtime"),
    )
    monkeypatch.setattr(
        health_surface,
        "evaluate_drain_gate",
        lambda settings, now, reading: GateState(open=True, conditions={}),
    )
    monkeypatch.setattr(health_surface, "read_last_drained_at", lambda: None)
    monkeypatch.setattr("solstone.think.models.no_thinking_engine_chosen", lambda: True)
    _stub_display_powersave(monkeypatch)

    backlog = health_surface._build_segment_backlog_health()

    assert backlog.awaiting_analysis_text == health_surface.NO_ENGINE_ANALYSIS_TEXT
    assert backlog.drain_state == DRAIN_STATE_NO_ENGINE


def test_segment_backlog_last_drained_at_passes_through(monkeypatch) -> None:
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}),
    )
    monkeypatch.setattr(
        health_surface,
        "load_processing_settings",
        lambda: _processing_settings("realtime"),
    )
    monkeypatch.setattr(
        health_surface,
        "evaluate_drain_gate",
        lambda settings, now, reading: GateState(open=False, conditions={}),
    )
    monkeypatch.setattr(
        health_surface,
        "read_last_drained_at",
        lambda: 1_700_000_000_000,
    )
    _stub_display_powersave(monkeypatch)

    backlog = health_surface._build_segment_backlog_health()

    assert backlog.last_drained_at == 1_700_000_000_000


def test_segment_backlog_uses_last_display_powersave_snapshot(monkeypatch) -> None:
    expected = DisplayPowersaveReading(available=True, asleep=True, debounced=True)
    captured = {}
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}),
    )
    monkeypatch.setattr(
        health_surface,
        "load_processing_settings",
        lambda: _processing_settings("deferred"),
    )

    def evaluate(settings, now, reading):
        captured["reading"] = reading
        return GateState(open=True, conditions={})

    monkeypatch.setattr(health_surface, "evaluate_drain_gate", evaluate)
    monkeypatch.setattr(health_surface, "read_last_drained_at", lambda: None)
    _stub_display_powersave(monkeypatch, reading=expected)

    health_surface._build_segment_backlog_health()

    assert captured["reading"] == expected


def test_segment_backlog_exposes_display_powersave_detectable(monkeypatch) -> None:
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}),
    )
    monkeypatch.setattr(
        health_surface,
        "load_processing_settings",
        lambda: _processing_settings("deferred"),
    )
    monkeypatch.setattr(
        health_surface,
        "evaluate_drain_gate",
        lambda settings, now, reading: GateState(open=False, conditions={}),
    )
    monkeypatch.setattr(health_surface, "read_last_drained_at", lambda: None)
    _stub_display_powersave(monkeypatch, detectable=True)

    backlog = health_surface._build_segment_backlog_health()

    assert backlog.display_powersave_detectable is True


def test_segment_backlog_health_never_polls_display(monkeypatch) -> None:
    from solstone.think import display_powersave

    poll = MagicMock()
    monkeypatch.setattr(display_powersave, "poll_display_powersave", poll)
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}),
    )
    monkeypatch.setattr(
        health_surface,
        "load_processing_settings",
        lambda: _processing_settings("deferred"),
    )
    monkeypatch.setattr(
        health_surface,
        "evaluate_drain_gate",
        lambda settings, now, reading: GateState(open=False, conditions={}),
    )
    monkeypatch.setattr(health_surface, "read_last_drained_at", lambda: None)
    _stub_display_powersave(monkeypatch)

    health_surface._build_segment_backlog_health()

    poll.assert_not_called()


def test_silent_facet_note_ladder_thresholds(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(
        tmp_path,
        facets=("criticalf", "fresh", "infof", "never", "warnf"),
    )
    _write_activity(
        tmp_path,
        "fresh",
        "20260410",
        _activity_record("20260410", "fresh_110000_1800", segments=["110000_1800"]),
    )
    _write_activity(
        tmp_path,
        "infof",
        "20260409",
        _activity_record("20260409", "info_100000_1800", segments=["100000_1800"]),
    )
    _write_activity(
        tmp_path,
        "warnf",
        "20260407",
        _activity_record("20260407", "warn_100000_1800", segments=["100000_1800"]),
    )
    _write_activity(
        tmp_path,
        "criticalf",
        "20260403",
        _activity_record(
            "20260403",
            "critical_100000_1800",
            segments=["100000_1800"],
        ),
    )

    report = health_surface.summary("20260410")
    note_map = {
        note.message.split(":", 1)[0]: note
        for note in report.notes
        if note.category == "capture" and ":" in note.message
    }

    assert "fresh" not in note_map
    assert note_map["infof"].severity == "info"
    assert "last capture" in note_map["infof"].message
    assert note_map["warnf"].severity == "warn"
    assert note_map["criticalf"].severity == "critical"
    assert note_map["never"].severity == "info"
    assert "no captures recorded" in note_map["never"].message


def test_silent_facet_emits_single_highest_severity_note(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path, facets=("criticalf",))
    _write_activity(
        tmp_path,
        "criticalf",
        "20260403",
        _activity_record(
            "20260403",
            "critical_100000_1800",
            segments=["100000_1800"],
        ),
    )

    report = health_surface.summary("20260410")
    critical_notes = [
        note
        for note in report.notes
        if note.category == "capture" and note.message.startswith("criticalf:")
    ]

    assert len(critical_notes) == 1
    assert critical_notes[0].severity == "critical"


def test_indexer_stale_warn_threshold(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    fixed_now = _utc_dt("20260410")
    _set_now(monkeypatch, fixed_now)
    _minimal_facet_tree(tmp_path)
    _write_indexer_db(tmp_path, fixed_now - timedelta(days=8))

    report = health_surface.summary("20260410")

    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "indexer database last rebuilt" in note.message
        for note in report.notes
    )


def test_indexer_missing_emits_warn_and_none_timestamp(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    report = health_surface.summary("20260410")

    assert report.synthesis_health.indexer_last_rebuild_at is None
    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "indexer database missing" in note.message
        for note in report.notes
    )


def test_missing_talent_day_indexes_emit_info(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h is None
    assert report.synthesis_health.talent_degraded_outputs_24h is None
    assert any(
        note.category == "synthesis"
        and note.severity == "info"
        and "talent day-index logs missing" in note.message
        for note in report.notes
    )
    assert not any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "talent day-index" in note.message
        for note in report.notes
    )


def test_degraded_talent_outputs_count_and_warn_without_failure(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(
        tmp_path,
        "20260410",
        {
            "use_id": "1",
            "name": "morning_briefing",
            "day": "20260410",
            "facet": None,
            "ts": _utc_ms("20260410", 9),
            "status": "completed",
            "provider": "openai",
            "model": "gpt-5",
            "degraded": {"reason": "near_empty", "output_tokens": 12},
        },
    )
    _write_talent_day(
        tmp_path,
        "20260409",
        {
            "use_id": "2",
            "name": "weekly_reflection",
            "day": "20260409",
            "facet": None,
            "ts": _utc_ms("20260409", 15),
            "status": "completed",
            "provider": "anthropic",
            "model": "claude-haiku-4-5",
        },
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_degraded_outputs_24h == 1
    assert report.synthesis_health.talent_run_failures_24h == 0
    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "morning_briefing" in note.message
        and "12" in note.message
        for note in report.notes
    )


def test_degraded_talent_outputs_follow_execution_timestamps_across_indexes(
    tmp_path, monkeypatch
):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(tmp_path, "20260410")
    _write_talent_day(tmp_path, "20260409")
    _write_talent_day(
        tmp_path,
        "20260401",
        {
            "use_id": "old-index-recent-degraded",
            "name": "morning_briefing",
            "day": "20260401",
            "facet": None,
            "ts": _utc_ms("20260410", 10),
            "status": "completed",
            "provider": "openai",
            "model": "gpt-5",
            "degraded": {"reason": "near_empty", "output_tokens": 12},
        },
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_degraded_outputs_24h == 1
    assert report.synthesis_health.talent_run_failures_24h == 0
    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "morning_briefing" in note.message
        and "openai/gpt-5" in note.message
        and "20260401" in note.message
        for note in report.notes
    )


def test_healthy_talent_outputs_do_not_emit_degraded_notes(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(
        tmp_path,
        "20260410",
        {
            "use_id": "1",
            "name": "morning_briefing",
            "day": "20260410",
            "facet": None,
            "ts": _utc_ms("20260410", 9),
            "status": "completed",
            "provider": "openai",
            "model": "gpt-5",
        },
    )
    _write_talent_day(
        tmp_path,
        "20260409",
        {
            "use_id": "2",
            "name": "weekly_reflection",
            "day": "20260409",
            "facet": None,
            "ts": _utc_ms("20260409", 15),
            "status": "completed",
            "provider": "anthropic",
            "model": "claude-haiku-4-5",
        },
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_degraded_outputs_24h == 0
    assert not any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "finished near-empty" in note.message
        for note in report.notes
    )


@pytest.mark.parametrize(
    ("raw_line", "problem_fragment"),
    [
        ("{not json\n", "malformed JSON"),
        (json.dumps(["not", "an", "object"]) + "\n", "non-object row"),
        (
            json.dumps(
                {
                    "use_id": "missing-ts",
                    "name": "flow",
                    "status": "error",
                }
            )
            + "\n",
            "missing ts",
        ),
        (
            json.dumps(
                {
                    "use_id": "bad-ts",
                    "name": "flow",
                    "ts": "1712757600000",
                    "status": "error",
                }
            )
            + "\n",
            "non-integer ts",
        ),
    ],
)
def test_corrupt_talent_day_index_rows_fail_closed_globally(
    tmp_path, monkeypatch, raw_line, problem_fragment
):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(tmp_path, "20260410")
    _write_talent_day(tmp_path, "20260409")
    _write_talent_day(
        tmp_path,
        "20260401",
        {
            "use_id": "valid-sibling",
            "name": "morning_briefing",
            "day": "20260401",
            "facet": None,
            "ts": _utc_ms("20260410", 10),
            "status": "completed",
            "provider": "openai",
            "model": "gpt-5",
            "degraded": {"reason": "near_empty", "output_tokens": 12},
        },
    )
    with (tmp_path / "talents" / "20260401.jsonl").open(
        "a", encoding="utf-8"
    ) as handle:
        handle.write(raw_line)

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h is None
    assert report.synthesis_health.talent_degraded_outputs_24h is None
    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "20260401.jsonl" in note.message
        and problem_fragment in note.message
        for note in report.notes
    )
    assert not any("finished near-empty" in note.message for note in report.notes)


def test_invalid_utf8_talent_day_index_fails_closed(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(tmp_path, "20260410")
    _write_talent_day(tmp_path, "20260409")
    path = tmp_path / "talents" / "20260401.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"\xff")

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h is None
    assert report.synthesis_health.talent_degraded_outputs_24h is None
    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "20260401.jsonl" in note.message
        and "invalid UTF-8" in note.message
        for note in report.notes
    )


def test_unreadable_talent_day_index_fails_closed(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(tmp_path, "20260410")
    _write_talent_day(tmp_path, "20260409")
    _write_talent_day(
        tmp_path,
        "20260401",
        {
            "use_id": "valid",
            "name": "flow",
            "ts": _utc_ms("20260410", 10),
            "status": "error",
        },
    )
    path = tmp_path / "talents" / "20260401.jsonl"
    path.chmod(0)
    try:
        report = health_surface.summary("20260410")
    finally:
        path.chmod(0o600)

    assert report.synthesis_health.talent_run_failures_24h is None
    assert report.synthesis_health.talent_degraded_outputs_24h is None
    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "20260401.jsonl" in note.message
        and "unreadable" in note.message
        for note in report.notes
    )


def test_vanished_talent_day_index_fails_closed(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(tmp_path, "20260410")
    _write_talent_day(tmp_path, "20260409")
    _write_talent_day(
        tmp_path,
        "20260401",
        {
            "use_id": "valid",
            "name": "flow",
            "ts": _utc_ms("20260410", 10),
            "status": "error",
        },
    )
    target = tmp_path / "talents" / "20260401.jsonl"
    original_open = Path.open

    def fake_open(self: Path, *args, **kwargs):
        if self == target:
            raise FileNotFoundError("vanished")
        return original_open(self, *args, **kwargs)

    monkeypatch.setattr(Path, "open", fake_open)

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h is None
    assert report.synthesis_health.talent_degraded_outputs_24h is None
    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "20260401.jsonl" in note.message
        and "vanished" in note.message
        for note in report.notes
    )


def test_missing_guard_and_scan_problems_both_emit_notes(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    path = tmp_path / "talents" / "20260401.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("{not json\n", encoding="utf-8")

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h is None
    assert report.synthesis_health.talent_degraded_outputs_24h is None
    assert any(
        note.category == "synthesis"
        and note.severity == "info"
        and "talent day-index logs missing" in note.message
        for note in report.notes
    )
    assert any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "20260401.jsonl" in note.message
        and "malformed JSON" in note.message
        for note in report.notes
    )


@pytest.mark.parametrize(
    ("offset_ms", "expected_failures"),
    [
        (-1, 0),
        (0, 1),
        (86_400_000, 1),
        (86_400_001, 0),
    ],
)
def test_talent_run_24h_boundary_is_inclusive(
    tmp_path,
    monkeypatch,
    offset_ms,
    expected_failures,
):
    _configure_env(tmp_path, monkeypatch)
    generated_at = _utc_ms("20260410", 12)
    _set_now(monkeypatch, _utc_dt("20260410", 12))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(tmp_path, "20260410")
    _write_talent_day(tmp_path, "20260409")
    _write_talent_day(
        tmp_path,
        "20260401",
        {
            "use_id": f"boundary-{offset_ms}",
            "name": "flow",
            "day": "20260401",
            "facet": None,
            "ts": generated_at - 86_400_000 + offset_ms,
            "status": "error",
            "error_message": "provider failed",
            "reason_code": "provider_unavailable",
        },
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h == expected_failures


def test_non_day_index_files_are_ignored_by_talent_health_fold(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(tmp_path, "20260410")
    _write_talent_day(tmp_path, "20260409")
    talents = tmp_path / "talents"
    (talents / "20260231.jsonl").write_text("{not json\n", encoding="utf-8")
    (talents / "abcdefgh.jsonl").write_text("{not json\n", encoding="utf-8")
    (talents / "default.log").write_text("{not json\n", encoding="utf-8")
    (talents / "default").mkdir()
    (talents / "default" / "1712757600000.jsonl").write_text(
        "{not json\n",
        encoding="utf-8",
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h == 0
    assert report.synthesis_health.talent_degraded_outputs_24h == 0
    assert not any(
        note.category == "synthesis"
        and note.severity == "warn"
        and "talent day-index" in note.message
        for note in report.notes
    )


def test_for_range_defaults_to_last_7_days_ending_today(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    report = health_surface.for_range()

    assert report.range == ("20260404", "20260410")


def test_summary_defaults_to_today_utc(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    report = health_surface.summary()

    assert report.range == ("20260410", "20260410")


def test_for_range_validation_errors(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    with pytest.raises(ValueError, match="both endpoints or neither"):
        health_surface.for_range(day_from="20260410")
    with pytest.raises(ValueError, match="day_from must be <="):
        health_surface.for_range("20260411", "20260410")
    with pytest.raises(ValueError, match="day must match YYYYMMDD"):
        health_surface.for_range("2026-04-10", "20260410")


def test_report_notes_sorted_by_severity_category_message(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path, facets=("alpha",))

    report = health_surface.summary("20260410")
    ordered = [(note.severity, note.category, note.message) for note in report.notes]

    assert ordered == [
        (
            "warn",
            "synthesis",
            "indexer database missing at journal/indexer/journal.sqlite; search-backed consumers may be stale.",
        ),
        ("info", "capture", "alpha: no captures recorded in the last 7 days."),
        (
            "info",
            "capture",
            "coverage_ratio unavailable in v1 — expected-hours denominator arrives Sprint 5+",
        ),
        (
            "info",
            "synthesis",
            "corrections roll-up not available — corrections ledger exists only from Sprint 5+",
        ),
        (
            "info",
            "synthesis",
            "talent day-index logs missing for 20260410, 20260409; last-24h failure count unavailable.",
        ),
    ]


def test_consumer_signal_counts_compose_ledger(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    now = datetime.now(UTC)
    day_recent = now.strftime("%Y%m%d")
    day_stale = (now - timedelta(days=30)).strftime("%Y%m%d")
    _minimal_facet_tree(tmp_path)
    _write_entity(tmp_path, "alex", "Alex")
    _write_activity(
        tmp_path,
        "work",
        day_recent,
        _activity_record(
            day_recent,
            "recent_090000_1800",
            commitments=[
                {
                    "owner": "Alex",
                    "owner_entity_id": "alex",
                    "action": "send recap",
                    "counterparty": "Blair",
                    "counterparty_entity_id": "blair",
                    "context": "Recent open item",
                }
            ],
            created_at=int((now - timedelta(days=1)).timestamp() * 1000),
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        day_stale,
        _activity_record(
            day_stale,
            "stale_090000_1800",
            commitments=[
                {
                    "owner": "Alex",
                    "owner_entity_id": "alex",
                    "action": "draft proposal",
                    "counterparty": "Blair",
                    "counterparty_entity_id": "blair",
                    "context": "Stale open item",
                }
            ],
            created_at=int((now - timedelta(days=30)).timestamp() * 1000),
        ),
    )

    from solstone.think.surfaces import ledger as ledger_surface

    original_list = ledger_surface.list
    calls: list[dict[str, object]] = []

    def spy_list(**kwargs):
        calls.append(dict(kwargs))
        return original_list(**kwargs)

    monkeypatch.setattr(health_surface.ledger, "list", spy_list)

    report = health_surface.summary(day_recent)

    assert report.consumer_signal.ledger_open_items_total == len(
        original_list(state="open")
    )
    assert report.consumer_signal.ledger_stale_items_count == len(
        original_list(state="open", age_days_gte=14)
    )
    assert calls == [{"state": "open"}, {"state": "open", "age_days_gte": 14}]


def test_structural_info_notes_are_always_present_and_coverage_ratio_is_none(
    tmp_path, monkeypatch
):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    report = health_surface.summary("20260410")

    assert report.capture_health.coverage_ratio is None
    assert all(note.detected_at == report.generated_at for note in report.notes)
    note_tuples = [
        (
            note.severity,
            note.category,
            note.message,
            note.detail_pointer,
        )
        for note in report.notes
    ]
    assert (
        "info",
        "capture",
        "coverage_ratio unavailable in v1 — expected-hours denominator arrives Sprint 5+",
        _SPEC_POINTER,
    ) in note_tuples
    assert (
        "info",
        "synthesis",
        "corrections roll-up not available — corrections ledger exists only from Sprint 5+",
        _SPEC_POINTER,
    ) in note_tuples


def test_segment_crossing_midnight_clipping(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "night_233000_3600",
            segments=["233000_3600"],
        ),
    )

    report = health_surface.summary("20260410")

    assert report.capture_health.hours_with_capture == 1


def test_activities_anticipated_unfilled_counts_past_visible_only(
    tmp_path, monkeypatch
):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "anticipated_past",
            source="anticipated",
            start=_iso_utc("20260410", 9),
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "anticipated_hidden",
            source="anticipated",
            start=_iso_utc("20260410", 8),
            hidden=True,
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "anticipated_future",
            source="anticipated",
            start=_iso_utc("20260410", 18),
        ),
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.activities_anticipated_unfilled == 1


def test_activities_user_edited_counts_prefixed_actors(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "cli_edit",
            edits=[{"actor": "cli:update", "fields": ["details"]}],
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "owner_edit",
            edits=[{"actor": "owner", "fields": ["details"]}],
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "user_edit",
            edits=[{"actor": "user", "fields": ["details"]}],
        ),
    )
    _write_activity(
        tmp_path,
        "work",
        "20260410",
        _activity_record(
            "20260410",
            "system_edit",
            edits=[{"actor": "system:story", "fields": ["story"]}],
        ),
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.activities_user_edited == 3


def test_talent_run_failures_24h_follow_execution_timestamps_across_indexes(
    tmp_path, monkeypatch
):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(tmp_path, "20260410")
    _write_talent_day(tmp_path, "20260409")
    _write_talent_day(
        tmp_path,
        "20260401",
        {
            "use_id": "1",
            "name": "flow",
            "day": "20260401",
            "facet": None,
            "ts": _utc_ms("20260410", 9),
            "status": "error",
            "error_message": "provider failed",
            "reason_code": "provider_unavailable",
        },
        {
            "use_id": "2",
            "name": "flow",
            "day": "20260401",
            "facet": None,
            "ts": _utc_ms("20260410", 8),
            "status": "completed",
        },
        {
            "use_id": "3",
            "name": "flow",
            "day": "20260401",
            "facet": None,
            "ts": _utc_ms("20260409", 15),
            "status": "error",
            "error_message": "provider failed",
            "reason_code": "provider_unavailable",
        },
        {
            "use_id": "4",
            "name": "flow",
            "day": "20260401",
            "facet": None,
            "ts": _utc_ms("20260409", 11, 30),
            "status": "error",
            "error_message": "provider failed",
            "reason_code": "provider_unavailable",
        },
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h == 2
