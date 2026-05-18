# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for activities app routes — activities API and output serving."""

import json

import pytest

from solstone.apps.activities.routes import (
    _GENERIC_ACTIVITY_ICON,
    _enrich_activity_record,
    activities_bp,
)


@pytest.fixture
def fixture_journal(monkeypatch):
    """Set SOLSTONE_JOURNAL to tests/fixtures/journal for testing."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    yield


@pytest.fixture
def activities_client(fixture_journal):
    """Create a Flask test client with the activities blueprint."""
    from flask import Flask

    from solstone.convey import state

    app = Flask(__name__)
    app.register_blueprint(activities_bp)
    state.journal_root = "tests/fixtures/journal"
    return app.test_client()


class TestActivitiesDayRoutes:
    def test_returns_enriched_records(self, activities_client):
        resp = activities_client.get(
            "/app/activities/api/day/20260214/activities?facet=full-featured"
        )
        assert resp.status_code == 200
        data = resp.get_json()
        assert isinstance(data, list)
        assert len(data) >= 2

        coding = next(a for a in data if a["activity"] == "coding")
        assert coding["id"] == "coding_093000_300"
        assert coding["facet"] == "full-featured"
        assert coding["description"] != ""
        assert coding["level_avg"] == 0.88
        assert coding["duration_minutes"] > 0
        assert "startTime" in coding
        assert "endTime" in coding
        assert len(coding["segments"]) == 4

    def test_includes_activity_metadata(self, activities_client):
        resp = activities_client.get(
            "/app/activities/api/day/20260214/activities?facet=full-featured"
        )
        data = resp.get_json()
        coding = next(a for a in data if a["activity"] == "coding")
        assert coding["name"] != ""
        assert coding["icon"] != ""

    def test_returns_mixed_anticipated_and_realized_records(self, activities_client):
        resp = activities_client.get(
            "/app/activities/api/day/20260422/activities?facet=full-featured"
        )
        assert resp.status_code == 200
        data = resp.get_json()
        by_id = {activity["id"]: activity for activity in data}
        assert set(by_id) == {
            "anticipated_meeting_090000_0422",
            "anticipated_call_140000_0422",
            "anticipated_deadline_170000_0422",
            "anticipated_appointment_000000_0422",
            "anticipated_made_up_type_000000_0422",
            "coding_110000_300",
        }

        meeting = by_id["anticipated_meeting_090000_0422"]
        assert meeting["startTime"] == "2026-04-22T09:00:00"
        assert meeting["endTime"] == "2026-04-22T10:00:00"
        assert meeting["duration_minutes"] == 60

        coding = by_id["coding_110000_300"]
        assert coding["startTime"] == "2026-04-22T11:00:00"
        assert coding["endTime"] == "2026-04-22T11:10:00"
        assert coding["duration_minutes"] == 10

        call = by_id["anticipated_call_140000_0422"]
        assert call["startTime"] == "2026-04-22T14:00:00"
        assert call["endTime"] == "2026-04-22T15:30:00"
        assert call["duration_minutes"] == 90
        assert call["name"] == "call"
        assert call["icon"] == "📞"

        deadline = by_id["anticipated_deadline_170000_0422"]
        assert deadline["startTime"] == "2026-04-22T17:00:00"
        assert "endTime" not in deadline
        assert "duration_minutes" not in deadline
        assert deadline["icon"] == "⏰"

        appointment = by_id["anticipated_appointment_000000_0422"]
        assert "startTime" not in appointment
        assert "endTime" not in appointment
        assert "duration_minutes" not in appointment
        assert appointment["icon"] == "📌"

        unknown = by_id["anticipated_made_up_type_000000_0422"]
        assert unknown["name"] == "made_up_type"
        assert unknown["icon"] == _GENERIC_ACTIVITY_ICON
        assert "duration_minutes" not in unknown

    def test_schedule_activity_defaults_are_lowercase(self):
        from solstone.think.activities import get_default_activity_by_id

        expected_names = {
            "call": "call",
            "deadline": "deadline",
            "appointment": "appointment",
            "event": "event",
            "travel": "travel",
            "reminder": "reminder",
            "errand": "errand",
            "celebration": "celebration",
            "doctor_appointment": "doctor appointment",
        }
        for activity_id, expected_name in expected_names.items():
            activity = get_default_activity_by_id(activity_id)
            assert activity is not None
            assert activity["name"] == expected_name
            assert activity["name"] == activity["name"].lower()

    @pytest.mark.parametrize(
        ("activity_id", "expected_name", "expected_icon"),
        [
            ("call", "call", "📞"),
            ("deadline", "deadline", "⏰"),
            ("appointment", "appointment", "📌"),
            ("event", "event", "🎟️"),
            ("travel", "travel", "✈️"),
            ("reminder", "reminder", "🔔"),
            ("errand", "errand", "🧾"),
            ("celebration", "celebration", "🎉"),
            ("doctor_appointment", "doctor appointment", "🩺"),
        ],
    )
    def test_enrich_activity_record_uses_global_default_for_schedule_activity(
        self,
        activities_client,
        activity_id,
        expected_name,
        expected_icon,
    ):
        record = {
            "id": f"anticipated_{activity_id}_090000_0422",
            "activity": activity_id,
            "target_date": "2026-04-22",
            "start": "09:00:00",
            "end": "09:30:00",
            "description": "planned item",
            "source": "anticipated",
        }

        enriched = _enrich_activity_record(record, "full-featured", "20260422")

        assert enriched is not None
        assert enriched["name"] == expected_name
        assert enriched["icon"] == expected_icon
        assert enriched["duration_minutes"] == 30

    def test_enrich_activity_record_uses_generic_icon_for_unknown_activity(
        self,
        activities_client,
    ):
        record = {
            "id": "anticipated_made_up_type_000000_0422",
            "activity": "made_up_type",
            "target_date": "2026-04-22",
            "start": None,
            "end": None,
            "description": "planned item",
            "source": "anticipated",
        }

        enriched = _enrich_activity_record(record, "full-featured", "20260422")

        assert enriched is not None
        assert enriched["name"] == "made_up_type"
        assert enriched["icon"] == _GENERIC_ACTIVITY_ICON
        assert "duration_minutes" not in enriched

    def test_enrich_activity_record_omits_duration_when_end_before_start(
        self,
        activities_client,
    ):
        record = {
            "id": "anticipated_meeting_inverted_0422",
            "activity": "meeting",
            "target_date": "2026-04-22",
            "start": "10:00:00",
            "end": "09:00:00",
            "description": "data error: end before start",
            "source": "anticipated",
        }

        enriched = _enrich_activity_record(record, "full-featured", "20260422")

        assert enriched is not None
        assert "duration_minutes" not in enriched
        assert enriched["startTime"] == "2026-04-22T10:00:00"
        assert enriched["endTime"] == "2026-04-22T09:00:00"

    def test_lists_output_files(self, activities_client):
        resp = activities_client.get(
            "/app/activities/api/day/20260214/activities?facet=full-featured"
        )
        data = resp.get_json()
        coding = next(a for a in data if a["activity"] == "coding")
        assert len(coding["outputs"]) >= 1
        output = coding["outputs"][0]
        assert output["filename"] == "session_review.md"
        assert "facets/full-featured/activities/" in output["path"]

    def test_invalid_day_returns_400(self, activities_client):
        resp = activities_client.get("/app/activities/api/day/badday/activities")
        assert resp.status_code == 400


class TestActivitiesOutputRoutes:
    def test_serves_activity_output(self, activities_client):
        resp = activities_client.get(
            "/app/activities/api/activity_output/"
            "facets/full-featured/activities/20260214/"
            "coding_093000_300/session_review.md"
        )
        assert resp.status_code == 200
        data = resp.get_json()
        assert "# Coding Session Review" in data["content"]
        assert data["format"] == "md"
        assert data["filename"] == "session_review.md"

    def test_rejects_non_facets_path(self, activities_client):
        resp = activities_client.get(
            "/app/activities/api/activity_output/20260214/talents/flow.md"
        )
        assert resp.status_code == 400


class TestActivitiesStatsRoutes:
    def test_returns_month_activity_counts(self, tmp_path, monkeypatch):
        from flask import Flask

        from solstone.convey import state

        journal = tmp_path / "journal"
        monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

        for facet in ("work", "personal"):
            facet_dir = journal / "facets" / facet
            facet_dir.mkdir(parents=True)
            (facet_dir / "facet.json").write_text(
                json.dumps({"title": facet.title()}), encoding="utf-8"
            )
            (facet_dir / "activities").mkdir()

        (journal / "facets" / "work" / "activities" / "20260418.jsonl").write_text(
            json.dumps({"id": "coding_1", "activity": "coding", "segments": []})
            + "\n"
            + json.dumps({"id": "coding_2", "activity": "coding", "segments": []})
            + "\n",
            encoding="utf-8",
        )
        (journal / "facets" / "personal" / "activities" / "20260418.jsonl").write_text(
            json.dumps({"id": "walk_1", "activity": "walking", "segments": []}) + "\n",
            encoding="utf-8",
        )
        (journal / "facets" / "work" / "activities" / "20260419.jsonl").write_text(
            json.dumps({"id": "coding_3", "activity": "coding", "segments": []}) + "\n",
            encoding="utf-8",
        )

        app = Flask(__name__)
        app.register_blueprint(activities_bp)
        monkeypatch.setattr(state, "journal_root", str(journal), raising=False)
        client = app.test_client()

        resp = client.get("/app/activities/api/stats/202604")

        assert resp.status_code == 200
        data = resp.get_json()
        assert data == {
            "20260418": {"personal": 1, "work": 2},
            "20260419": {"work": 1},
        }
        assert "20260420" not in data
