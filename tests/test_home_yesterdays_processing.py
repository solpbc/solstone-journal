# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for the Yesterday's processing home card."""

from __future__ import annotations

import json
import os
import shutil
from datetime import datetime
from pathlib import Path

import pytest

from solstone.apps.home.routes import (
    BRIEFING_LATENESS_THRESHOLD_HOURS,
    BRIEFING_MORNING_END_HOUR,
    _briefing_freshness,
    _briefing_lateness_state,
    _build_pulse_context,
    _collect_activities,
    _collect_anticipated_activities,
    _format_activity_label,
    _format_duration,
    _format_gap_links,
    _format_heatmap_summary,
    _knowledge_graph_freshness,
    _newsletter_attempts_from_think_logs,
    _summarize_yesterday_processing,
)

FIXTURES = Path(__file__).parent / "fixtures" / "journal"


def _copy_fixture_file(journal: Path, rel_path: str) -> None:
    src = FIXTURES / rel_path
    dst = journal / rel_path
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def _write_jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows),
        encoding="utf-8",
    )


def _write_facet_meta(journal: Path, facet: str, title: str) -> None:
    path = journal / "facets" / facet / "facet.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(
            {
                "title": title,
                "description": "",
                "color": "#0f172a",
                "emoji": "📁",
            }
        ),
        encoding="utf-8",
    )


def _seed_journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    for day, transcript_seconds in (("20260415", 3600), ("20260414", 2700)):
        facet_data = {"work": {"count": 1, "minutes": 15}}
        if day == "20260414":
            facet_data = {}
        stats_path = journal / "chronicle" / day / "stats.json"
        stats_path.parent.mkdir(parents=True, exist_ok=True)
        stats_path.write_text(
            json.dumps(
                {
                    "stats": {
                        "transcript_segments": 3,
                        "transcript_duration": transcript_seconds,
                    },
                    "facet_data": facet_data,
                    "heatmap_data": {"weekday": 2, "hours": {"9": 45.0}},
                }
            ),
            encoding="utf-8",
        )

    health_path = journal / "chronicle" / "20260415" / "health" / "100_daily.jsonl"
    health_path.parent.mkdir(parents=True, exist_ok=True)
    health_path.write_text("", encoding="utf-8")
    sparse_health_path = (
        journal / "chronicle" / "20260414" / "health" / "100_daily.jsonl"
    )
    sparse_health_path.parent.mkdir(parents=True, exist_ok=True)
    sparse_health_path.write_text(
        json.dumps({"event": "run.complete", "mode": "daily", "duration_ms": 10})
        + "\n",
        encoding="utf-8",
    )

    for rel_path in [
        "chronicle/20260415/talents/knowledge_graph.md",
    ]:
        _copy_fixture_file(journal, rel_path)

    for rel_path in [
        "facets/work/activities/20260415.jsonl",
        "facets/personal/activities/20260415.jsonl",
        "facets/work/news/20260415.md",
        "facets/personal/news/20260415.md",
    ]:
        _copy_fixture_file(journal, rel_path)

    (journal / "chronicle" / "20260416").mkdir(parents=True, exist_ok=True)
    _write_facet_meta(journal, "work", "Work")
    _write_facet_meta(journal, "personal", "Personal")
    return journal


def _write_briefing(
    journal: Path, generated: str, *, metadata_type: str = "morning_briefing"
) -> None:
    path = journal / "identity" / "briefing.md"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        (
            f"---\n"
            f"type: {metadata_type}\n"
            f'generated: "{generated}"\n'
            f"---\n\n"
            "## Your Day\n\n"
            "- One thing.\n"
        ),
        encoding="utf-8",
    )


def _append_think_log(
    journal: Path,
    day: str,
    name: str,
    *,
    facet: str | None = None,
    event: str = "talent.fail",
) -> None:
    path = journal / "chronicle" / day / "health" / "101_daily.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        record = {
            "event": event,
            "mode": "daily",
            "name": name,
            "state": "error",
        }
        if facet is not None:
            record["facet"] = facet
        handle.write(json.dumps(record) + "\n")


def _patch_minimal_pulse_context(monkeypatch, pipeline_status):
    monkeypatch.setattr(
        "solstone.apps.home.routes.get_capture_health",
        lambda: {"status": "active", "observers": []},
    )
    monkeypatch.setattr("solstone.apps.home.routes.get_cached_state", lambda: {})
    monkeypatch.setattr("solstone.apps.home.routes.get_current", lambda: None)
    monkeypatch.setattr(
        "solstone.apps.home.routes._resolve_attention", lambda awareness: None
    )
    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260416")
    monkeypatch.setattr("solstone.apps.home.routes._yesterday", lambda: "20260415")
    monkeypatch.setattr(
        "solstone.apps.home.routes._count_journal_age_days", lambda today: 8
    )
    monkeypatch.setattr("solstone.apps.home.routes._load_stats", lambda today: {})
    monkeypatch.setattr(
        "solstone.apps.home.routes._load_flow_md", lambda today: (None, None)
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._load_pulse_md", lambda: (None, None, [])
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._load_briefing_md", lambda today: ({}, None, [])
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._collect_anticipated_activities", lambda today: []
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._collect_activities", lambda today: []
    )
    monkeypatch.setattr("solstone.apps.home.routes._collect_todos", lambda today: [])
    monkeypatch.setattr("solstone.apps.home.routes._collect_routines", lambda: [])
    monkeypatch.setattr("solstone.apps.home.routes._collect_skills", lambda: [])
    monkeypatch.setattr(
        "solstone.apps.home.routes.read_steward_health",
        lambda: pipeline_status,
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._summarize_yesterday_processing",
        lambda yesterday, journal_age_days: {
            "title": "Yesterday's processing",
            "mode": "healthy",
            "default_collapsed": True,
            "summary_line": "I wrote 2 newsletters.",
            "details": [],
            "sparse_lines": None,
            "first_week_framing": None,
            "status_reasons": [],
        },
    )


def _set_mtime(path: Path, dt: datetime) -> None:
    ts = dt.timestamp()
    os.utime(path, (ts, ts))


def test_yesterdays_card_hidden_when_stats_missing(tmp_path, monkeypatch):
    journal = _seed_journal(tmp_path, monkeypatch)
    _write_briefing(journal, "2026-04-17T06:45:00")

    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260417")

    assert _summarize_yesterday_processing("20260416", 9) is None


def test_yesterdays_card_hidden_when_all_zero(tmp_path, monkeypatch):
    journal = _seed_journal(tmp_path, monkeypatch)
    _write_briefing(journal, "2026-04-17T06:45:00")
    (journal / "chronicle" / "20260416" / "stats.json").write_text(
        json.dumps(
            {
                "stats": {
                    "transcript_segments": 0,
                    "transcript_duration": 0,
                },
                "facet_data": {},
                "heatmap_data": {"weekday": 3, "hours": {}},
            }
        ),
        encoding="utf-8",
    )

    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260417")

    assert _summarize_yesterday_processing("20260416", 9) is None


def test_collect_anticipated_activities_surfaces_only_anticipated_records(
    tmp_path,
    monkeypatch,
):
    journal = tmp_path / "journal"
    journal.mkdir()
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    today = "20260418"
    now_ms = int(datetime.now().timestamp() * 1000)

    _write_facet_meta(journal, "work", "Work")
    _write_jsonl(
        journal / "facets" / "work" / "activities" / f"{today}.jsonl",
        [
            {
                "id": "anticipated_call_103000_0418",
                "activity": "call",
                "title": "Mari intro",
                "description": "Planned intro call",
                "target_date": "2026-04-18",
                "start": "10:30:00",
                "end": "11:00:00",
                "source": "anticipated",
                "created_at": now_ms,
                "participation": [
                    {
                        "name": "Mari Zumbro",
                        "role": "attendee",
                        "source": "screen",
                        "confidence": 0.9,
                        "context": "calendar invite",
                    },
                    {
                        "name": "Ramon",
                        "role": "mentioned",
                        "source": "screen",
                        "confidence": 0.6,
                        "context": "note",
                    },
                ],
            },
            {
                "id": "coding_090000_300",
                "activity": "coding",
                "title": "Focused coding",
                "description": "Recent work",
                "created_at": now_ms,
                "source": "user",
            },
        ],
    )

    anticipated_activities = _collect_anticipated_activities(today)
    activities = _collect_activities(today)

    assert [item["title"] for item in anticipated_activities] == ["Mari intro"]
    assert anticipated_activities[0]["occurred"] is False
    assert anticipated_activities[0]["participants"] == ["Mari Zumbro"]
    assert [activity["id"] for activity in activities] == ["coding_090000_300"]


def test_yesterdays_card_sparse_mode_copy(tmp_path, monkeypatch):
    journal = _seed_journal(tmp_path, monkeypatch)
    _write_briefing(journal, "2026-04-15T06:45:00")

    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260415")
    monkeypatch.setattr(
        "solstone.apps.home.routes._knowledge_graph_freshness",
        lambda _day: {"fresh": True},
    )

    summary = _summarize_yesterday_processing("20260414", 2)

    assert summary["mode"] == "sparse"
    assert summary["default_collapsed"] is False
    assert summary["first_week_framing"] is None
    assert summary["summary_line"] == "I watched 45 min yesterday."
    assert summary["sparse_lines"] == [
        "I didn't produce any facet newsletters.",
        "There wasn't much else to process.",
    ]


def test_yesterdays_card_healthy_collapsed_on_day_8_plus(tmp_path, monkeypatch):
    journal = _seed_journal(tmp_path, monkeypatch)
    _write_briefing(journal, "2026-04-16T06:45:00")

    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260416")

    summary = _summarize_yesterday_processing("20260415", 8)

    assert summary["mode"] == "healthy"
    assert summary["title"] == "Yesterday's processing"
    assert summary["default_collapsed"] is True
    assert summary["first_week_framing"] is None
    assert (
        summary["summary_line"]
        == "I wrote 2 newsletters, refreshed your knowledge graph, and prepared your morning briefing."
    )


def test_yesterdays_card_healthy_expanded_with_framing_on_days_1_to_7(
    tmp_path, monkeypatch
):
    journal = _seed_journal(tmp_path, monkeypatch)
    _write_briefing(journal, "2026-04-16T06:45:00")

    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260416")

    summary = _summarize_yesterday_processing("20260415", 5)

    assert summary["mode"] == "healthy"
    assert summary["default_collapsed"] is False
    assert (
        summary["first_week_framing"]
        == "Most of what I learn becomes useful in the third or fourth week, when I've seen enough patterns to surface them. For now, here's what's already happening:"
    )


def test_yesterdays_card_degraded_shows_warning_and_partial_count(
    tmp_path, monkeypatch
):
    journal = _seed_journal(tmp_path, monkeypatch)
    _write_briefing(journal, "2026-04-16T06:45:00")
    _append_think_log(journal, "20260415", "facet_newsletter", facet="personal")

    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260416")

    summary = _summarize_yesterday_processing("20260415", 8)

    assert summary["mode"] == "degraded"
    assert summary["title"] == "⚠ Yesterday's processing"
    assert summary["summary_line"] == (
        "I wrote 2 of 3 newsletters, but some overnight processing didn't finish."
    )
    assert summary["status_reasons"] == ["newsletter_partial", "pipeline_warning"]
    assert summary["gap_links"][0] == {
        "text": "The facet newsletter run didn't finish.",
        "href": "/app/sol/20260415#facet_newsletter",
    }
    assert "I wrote 2 of 3 newsletters." in summary["details"]


def test_yesterdays_card_degraded_zero_newsletters_keeps_failure_caveat(
    tmp_path, monkeypatch
):
    journal = _seed_journal(tmp_path, monkeypatch)
    for path in journal.glob("facets/*/news/20260415.md"):
        path.unlink()

    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260416")

    summary = _summarize_yesterday_processing("20260415", 8)

    assert summary["mode"] == "degraded"
    assert (
        summary["summary_line"]
        == "I didn't produce any facet newsletters, and some overnight processing didn't finish."
    )


def test_format_duration_boundaries():
    assert _format_duration(59) == "59 min"
    assert _format_duration(60) == "1 hour"


def test_heatmap_peaks_top_3():
    assert (
        _format_heatmap_summary(
            {
                "heatmap_data": {
                    "hours": {
                        "9": 45.0,
                        "11": 40.0,
                        "14": 30.0,
                        "10": 20.0,
                    }
                }
            }
        )
        == "I watched most closely during 9-10am · 11am-12pm · 2-3pm."
    )


def test_activity_bullet_title_duration_facet(tmp_path, monkeypatch):
    _seed_journal(tmp_path, monkeypatch)
    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260416")
    activity = _summarize_yesterday_processing("20260415", 8)

    first_activity = next(
        detail for detail in activity["details"] if detail.startswith("I took notes on")
    )

    assert (
        first_activity
        == "I took notes on Extended coding session with Git and VS Code for 15 min in work."
    )
    assert (
        _format_activity_label(
            {
                "title": "Drafted weekend notes",
                "duration_minutes": 10,
                "facet": "personal",
            }
        )
        == "I took notes on Drafted weekend notes for 10 min in personal."
    )


def test_knowledge_graph_refresh_detection_yesterday_and_overnight(
    tmp_path, monkeypatch
):
    journal = _seed_journal(tmp_path, monkeypatch)
    path = journal / "chronicle" / "20260415" / "talents" / "knowledge_graph.md"

    _set_mtime(path, datetime(2026, 4, 15, 12, 0, 0))
    assert _knowledge_graph_freshness("20260415")["fresh"] is True

    _set_mtime(path, datetime(2026, 4, 16, 2, 0, 0))
    assert _knowledge_graph_freshness("20260415")["fresh"] is True


def test_briefing_frontmatter_missing_counts_as_gap(tmp_path, monkeypatch):
    _seed_journal(tmp_path, monkeypatch)

    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260416")

    summary = _summarize_yesterday_processing("20260415", 8)

    assert summary["mode"] == "degraded"
    assert "briefing_missing" in summary["status_reasons"]
    assert {
        "text": "I didn't prepare your morning briefing overnight.",
        "href": "/app/sol/20260416#morning_briefing",
    } in summary["gap_links"]
    assert _briefing_freshness("20260416") == {
        "exists": False,
        "valid": False,
        "generated_label": None,
    }


def test_gap_links_show_specific_daily_and_activity_without_generic():
    links = _format_gap_links(
        {
            "anomalies": [
                {"kind": "daily_agents_missing"},
                {"kind": "activity_agents_missing"},
                {"kind": "talent_failure"},
            ]
        },
        {"fresh": True},
        {"valid": True},
        "20260415",
        "20260416",
    )

    assert links == [
        {
            "text": "I didn't finish the full overnight review.",
            "href": "/app/sol/20260415",
        },
        {
            "text": "I didn't finish writing all of yesterday's notes.",
            "href": "/app/sol/20260415",
        },
    ]


@pytest.mark.parametrize(
    ("anomalies", "knowledge_graph", "briefing", "expected"),
    [
        (
            [{"kind": "daily_agents_missing"}],
            {"fresh": True},
            {"valid": True},
            {
                "text": "I didn't finish the full overnight review.",
                "href": "/app/sol/20260415",
            },
        ),
        (
            [{"kind": "activity_agents_missing"}],
            {"fresh": True},
            {"valid": True},
            {
                "text": "I didn't finish writing all of yesterday's notes.",
                "href": "/app/sol/20260415",
            },
        ),
        (
            [{"kind": "talent_failure", "name": "flow", "use_id": "run-1"}],
            {"fresh": True},
            {"valid": True},
            {
                "text": "The flow run didn't finish.",
                "href": "/app/sol/20260415#flow/run-1",
            },
        ),
        (
            [{"kind": "talent_failure", "name": "facet_newsletter"}],
            {"fresh": True},
            {"valid": True},
            {
                "text": "The facet newsletter run didn't finish.",
                "href": "/app/sol/20260415#facet_newsletter",
            },
        ),
        (
            [{"kind": "talent_failure"}],
            {"fresh": True},
            {"valid": True},
            {
                "text": "Some of my overnight work didn't finish.",
                "href": "/app/sol/20260415",
            },
        ),
        (
            [],
            {"fresh": False},
            {"valid": True},
            {
                "text": "I didn't refresh your knowledge graph overnight.",
                "href": "/app/sol/20260415#knowledge_graph",
            },
        ),
        (
            [],
            {"fresh": True},
            {"valid": False},
            {
                "text": "I didn't prepare your morning briefing overnight.",
                "href": "/app/sol/20260416#morning_briefing",
            },
        ),
    ],
)
def test_gap_links_href_for_each_anomaly_kind(
    anomalies, knowledge_graph, briefing, expected
):
    assert expected in _format_gap_links(
        {"anomalies": anomalies},
        knowledge_graph,
        briefing,
        "20260415",
        "20260416",
    )


def test_briefing_lateness_threshold():
    due_hour = BRIEFING_MORNING_END_HOUR
    before = datetime(2026, 4, 16, due_hour + BRIEFING_LATENESS_THRESHOLD_HOURS, 45)
    after = datetime(2026, 4, 16, due_hour + BRIEFING_LATENESS_THRESHOLD_HOURS + 1, 0)

    assert _briefing_lateness_state(before, "pending") == {
        "late": False,
        "late_hours": 0,
    }
    assert _briefing_lateness_state(after, "pending") == {
        "late": True,
        "late_hours": BRIEFING_LATENESS_THRESHOLD_HOURS + 1,
    }
    assert _briefing_lateness_state(after, "active") == {
        "late": False,
        "late_hours": 0,
    }


def test_newsletter_attempts_option_a_matches_facet_newsletter_failures_only(
    tmp_path, monkeypatch
):
    journal = _seed_journal(tmp_path, monkeypatch)
    _append_think_log(journal, "20260415", "facet_newsletter", facet="work")
    _append_think_log(journal, "20260415", "knowledge_graph", facet="work")
    _append_think_log(journal, "20260415", "facet_newsletter")

    assert _newsletter_attempts_from_think_logs("20260415") == (2, 3)


def test_build_pulse_context_includes_yesterday_processing(monkeypatch):
    monkeypatch.setattr(
        "solstone.apps.home.routes.get_capture_health",
        lambda: {"status": "active", "observers": []},
    )
    monkeypatch.setattr("solstone.apps.home.routes.get_cached_state", lambda: {})
    monkeypatch.setattr("solstone.apps.home.routes.get_current", lambda: None)
    monkeypatch.setattr(
        "solstone.apps.home.routes._resolve_attention", lambda awareness: None
    )
    monkeypatch.setattr("solstone.apps.home.routes._today", lambda: "20260416")
    monkeypatch.setattr("solstone.apps.home.routes._yesterday", lambda: "20260415")
    monkeypatch.setattr(
        "solstone.apps.home.routes._count_journal_age_days", lambda today: 8
    )
    monkeypatch.setattr("solstone.apps.home.routes._load_stats", lambda today: {})
    monkeypatch.setattr(
        "solstone.apps.home.routes._load_flow_md", lambda today: (None, None)
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._load_pulse_md", lambda: (None, None, [])
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._load_briefing_md", lambda today: ({}, None, [])
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._collect_anticipated_activities", lambda today: []
    )
    monkeypatch.setattr(
        "solstone.apps.home.routes._collect_activities", lambda today: []
    )
    monkeypatch.setattr("solstone.apps.home.routes._collect_todos", lambda today: [])
    monkeypatch.setattr("solstone.apps.home.routes._collect_routines", lambda: [])
    monkeypatch.setattr("solstone.apps.home.routes._collect_skills", lambda: [])
    monkeypatch.setattr("solstone.apps.home.routes.read_steward_health", lambda: None)
    monkeypatch.setattr(
        "solstone.apps.home.routes._summarize_yesterday_processing",
        lambda yesterday, journal_age_days: {
            "title": "Yesterday's processing",
            "mode": "healthy",
            "default_collapsed": True,
            "summary_line": "I wrote 2 newsletters.",
            "details": [],
            "sparse_lines": None,
            "first_week_framing": None,
            "status_reasons": [],
        },
    )

    ctx = _build_pulse_context()

    assert ctx["yesterday_processing"]["summary_line"] == "I wrote 2 newsletters."


def test_build_pulse_context_pipeline_status_none_when_steward_healthy(monkeypatch):
    _patch_minimal_pulse_context(monkeypatch, None)

    ctx = _build_pulse_context()

    assert ctx["pipeline_status"] is None


def test_build_pulse_context_pipeline_status_surfaces_steward_warning(monkeypatch):
    status = {"status": "warning", "message": "Foo bar"}
    _patch_minimal_pulse_context(monkeypatch, status)

    ctx = _build_pulse_context()

    assert ctx["pipeline_status"] == status
