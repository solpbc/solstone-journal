# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
import re
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest
from click.testing import Result
from typer.testing import CliRunner

from solstone.think.pipeline_health import SegmentBacklog, SegmentCompletion
from solstone.think.surfaces import health as health_surface

_RUNNER = CliRunner()
_SPEC_POINTER = "cpo/specs/in-flight/consumer-surface-health.md"


def _configure_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setenv("SOL_SKIP_SUPERVISOR_CHECK", "1")

    from solstone.think.entities.journal import clear_journal_entity_cache
    from solstone.think.entities.loading import clear_entity_loading_cache
    from solstone.think.entities.relationships import clear_relationship_caches

    clear_journal_entity_cache()
    clear_entity_loading_cache()
    clear_relationship_caches()


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


def _write_pipeline_log(
    tmp_path: Path,
    day: str,
    filename: str,
    *rows: dict[str, object],
) -> None:
    path = tmp_path / "chronicle" / day / "health" / filename
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(row) + "\n" for row in rows),
        encoding="utf-8",
    )


def _segment_backlog(
    per_day_counts: dict[str, int],
    *,
    errors: tuple[str, ...] = (),
) -> SegmentBacklog:
    per_day = {
        day: SegmentCompletion(
            blockers=[],
            not_sensed=0,
            not_thought=not_thought,
            total=max(not_thought, 1),
        )
        for day, not_thought in per_day_counts.items()
    }
    return SegmentBacklog(
        days=tuple(per_day_counts),
        not_thought=sum(per_day_counts.values()),
        not_sensed=0,
        total=sum(completion.total for completion in per_day.values()),
        per_day=per_day,
        errors=errors,
    )


def _invoke_health_summary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> Result:
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    from solstone.think.call import call_app

    return _RUNNER.invoke(call_app, ["health", "summary", "--day", "20260410"])


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
    assert any(
        note.category == "synthesis"
        and note.severity == "info"
        and "talent day-index logs missing" in note.message
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


def test_talent_run_failures_24h_use_today_and_yesterday_day_indices(
    tmp_path, monkeypatch
):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)
    _write_talent_day(
        tmp_path,
        "20260410",
        {
            "use_id": "1",
            "name": "flow",
            "day": "20260410",
            "facet": None,
            "ts": _utc_ms("20260410", 9),
            "status": "completed",
            "error": "boom",
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
            "status": "failed",
        },
        {
            "use_id": "4",
            "name": "flow",
            "day": "20260409",
            "facet": None,
            "ts": _utc_ms("20260409", 11, 30),
            "status": "failed",
        },
    )

    report = health_surface.summary("20260410")

    assert report.synthesis_health.talent_run_failures_24h == 2


def test_cli_summary_renders_segment_backlog(tmp_path, monkeypatch):
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({"20260409": 1, "20260410": 2}),
    )

    result = _invoke_health_summary(tmp_path, monkeypatch)

    assert result.exit_code == 0
    assert "3 segments across 2 days awaiting thinking" in result.stdout


def test_cli_summary_renders_singular_segment_backlog(tmp_path, monkeypatch):
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({"20260410": 1}),
    )

    result = _invoke_health_summary(tmp_path, monkeypatch)

    assert result.exit_code == 0
    assert "1 segment across 1 day awaiting thinking" in result.stdout


def test_cli_summary_renders_segment_backlog_unavailable(tmp_path, monkeypatch):
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}, errors=("20260101",)),
    )

    result = _invoke_health_summary(tmp_path, monkeypatch)

    assert result.exit_code == 0
    assert "Segment thinking status unavailable" in result.stdout
    assert "awaiting thinking" not in result.stdout


def test_cli_summary_renders_partial_segment_backlog(tmp_path, monkeypatch):
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({"20260409": 1, "20260410": 2}, errors=("bad",)),
    )

    result = _invoke_health_summary(tmp_path, monkeypatch)

    assert result.exit_code == 0
    assert "at least" in result.stdout
    assert "awaiting thinking (status incomplete)" in result.stdout


def test_cli_summary_omits_caught_up_segment_backlog(tmp_path, monkeypatch):
    monkeypatch.setattr(
        health_surface,
        "read_segment_backlog",
        lambda: _segment_backlog({}),
    )

    result = _invoke_health_summary(tmp_path, monkeypatch)

    assert result.exit_code == 0
    assert "awaiting thinking" not in result.stdout


def test_cli_health_summary_full_range_json(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _set_now(monkeypatch, _utc_dt("20260410"))
    _minimal_facet_tree(tmp_path)

    from solstone.think.call import call_app

    summary_result = _RUNNER.invoke(call_app, ["health", "summary", "--json"])
    full_result = _RUNNER.invoke(call_app, ["health", "full", "--json"])
    range_result = _RUNNER.invoke(
        call_app,
        [
            "health",
            "for-range",
            "--day-from",
            "20260409",
            "--day-to",
            "20260410",
            "--json",
        ],
    )

    for result in (summary_result, full_result, range_result):
        assert result.exit_code == 0
        payload = json.loads(result.stdout)
        assert set(payload) == {
            "generated_at",
            "range",
            "facets",
            "capture_health",
            "synthesis_health",
            "consumer_signal",
            "segment_backlog",
            "notes",
        }


def test_cli_help_disambiguates_and_lists_health_once(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _minimal_facet_tree(tmp_path)

    from solstone.think.call import call_app

    health_help = _RUNNER.invoke(call_app, ["health", "--help"])
    root_help = _RUNNER.invoke(call_app, ["--help"])
    normalized_help = " ".join(health_help.stdout.split())

    assert health_help.exit_code == 0
    assert (
        "Health: journal-data trust signals (for infrastructure/service liveness, use `sol health`)."
        in normalized_help
    )
    assert root_help.exit_code == 0
    assert (
        sum(
            1
            for line in root_help.stdout.splitlines()
            if re.search(r"│\s+health\s+Health:", line)
        )
        == 1
    )


def test_cli_pipeline_relocated_behavior(tmp_path, monkeypatch):
    _configure_env(tmp_path, monkeypatch)
    _minimal_facet_tree(tmp_path)
    day = "20260101"
    expected_today = datetime.now().strftime("%Y%m%d")
    expected_yesterday = (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")
    _write_pipeline_log(
        tmp_path,
        day,
        "123_segment.jsonl",
        {"event": "run.start", "mode": "segment"},
        {"event": "talent.dispatch", "mode": "segment"},
        {"event": "talent.complete", "mode": "segment"},
        {"event": "run.complete", "mode": "segment", "duration_ms": 42},
    )

    from solstone.think.call import call_app

    default_result = _RUNNER.invoke(call_app, ["health", "pipeline"])
    day_result = _RUNNER.invoke(call_app, ["health", "pipeline", "--day", day])
    yesterday_result = _RUNNER.invoke(call_app, ["health", "pipeline", "--yesterday"])
    error_result = _RUNNER.invoke(
        call_app,
        ["health", "pipeline", "--day", day, "--yesterday"],
    )

    assert default_result.exit_code == 0
    assert json.loads(default_result.stdout)["day"] == expected_today
    assert day_result.exit_code == 0
    day_payload = json.loads(day_result.stdout)
    assert day_payload["day"] == day
    assert day_payload["runs"]["segment"]["count"] == 1
    assert day_payload["talents"]["dispatched"] >= 1
    assert yesterday_result.exit_code == 0
    assert json.loads(yesterday_result.stdout)["day"] == expected_yesterday
    assert error_result.exit_code == 1
    assert "mutually exclusive" in error_result.stderr
