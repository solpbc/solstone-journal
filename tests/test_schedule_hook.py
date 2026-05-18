# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
from pathlib import Path


def _write_facet(journal: Path, facet: str) -> None:
    facet_path = journal / "facets" / facet / "facet.json"
    facet_path.parent.mkdir(parents=True, exist_ok=True)
    facet_path.write_text(
        json.dumps({"title": facet.title(), "description": ""}),
        encoding="utf-8",
    )


def _write_detected_entities(
    journal: Path,
    facet: str,
    day: str,
    rows: list[dict],
) -> None:
    path = journal / "facets" / facet / "entities" / f"{day}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows),
        encoding="utf-8",
    )


def test_schedule_post_process_writes_record_and_resolves_entities(
    tmp_path,
    monkeypatch,
):
    from solstone.talent.schedule import post_process
    from solstone.think.activities import load_activity_records

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")
    _write_detected_entities(
        tmp_path,
        "work",
        "20260420",
        [
            {"id": "yuri_namikawa", "type": "Person", "name": "Yuri Namikawa"},
            {"id": "scott_ward", "type": "Person", "name": "Scott Ward"},
        ],
    )

    payload = [
        {
            "activity": "meeting",
            "target_date": "2026-04-20",
            "start": "16:30:00",
            "end": "17:30:00",
            "title": "Yuri Namikawa intro call",
            "description": "Intro call with Yuri from Offline Ventures.",
            "details": "Google Meet",
            "participation": [
                {
                    "name": "Yuri Namikawa",
                    "role": "attendee",
                    "source": "screen",
                    "confidence": 0.95,
                    "context": "calendar invite",
                },
                {
                    "name": "Scott Ward",
                    "role": "mentioned",
                    "source": "screen",
                    "confidence": 0.5,
                    "context": "mentioned in notes",
                },
                {
                    "name": "Unknown Guest",
                    "role": "attendee",
                    "source": "screen",
                    "confidence": 0.4,
                    "context": "guest field",
                },
            ],
            "participation_confidence": 0.88,
            "facet": "work",
            "cancelled": False,
        }
    ]

    assert post_process(json.dumps(payload), {"day": "20260418"}) is None

    records = load_activity_records("work", "20260420", include_hidden=True)
    assert len(records) == 1
    record = records[0]
    assert record["id"] == "anticipated_meeting_163000_0420"
    assert record["source"] == "anticipated"
    assert record["active_entities"] == ["yuri_namikawa"]
    assert record["cancelled"] is False
    assert record["hidden"] is False
    assert record["participation_confidence"] == 0.88
    assert record["participation"][0]["entity_id"] == "yuri_namikawa"
    assert record["participation"][1]["entity_id"] == "scott_ward"
    assert record["participation"][2]["entity_id"] is None
    assert record["edits"][-1]["actor"] == "schedule"
    assert record["edits"][-1]["note"] == "created by schedule"


def test_schedule_post_process_accepts_wrapped_events(tmp_path, monkeypatch):
    from solstone.talent.schedule import post_process
    from solstone.think.activities import load_activity_records

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")

    payload = {
        "events": [
            {
                "activity": "meeting",
                "target_date": "2026-04-23",
                "start": "09:00:00",
                "end": "09:30:00",
                "title": "Planning call",
                "description": "Planning call with the team.",
                "details": "Google Meet",
                "participation": [],
                "participation_confidence": 0.8,
                "facet": "work",
                "cancelled": False,
            }
        ]
    }

    post_process(json.dumps(payload), {"day": "20260418"})

    records = load_activity_records("work", "20260423", include_hidden=True)
    assert len(records) == 1
    assert records[0]["id"] == "anticipated_meeting_090000_0423"
    assert records[0]["source"] == "anticipated"


def test_schedule_post_process_marks_cancelled_records_hidden(tmp_path, monkeypatch):
    from solstone.talent.schedule import post_process
    from solstone.think.activities import load_activity_records

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")

    payload = [
        {
            "activity": "meeting",
            "target_date": "2026-04-22",
            "start": "09:00:00",
            "end": "10:00:00",
            "title": "Scott Ward standup",
            "description": "Weekly standup with Scott Ward.",
            "details": "Recurring invite",
            "participation": [],
            "participation_confidence": 0.85,
            "facet": "work",
            "cancelled": True,
        }
    ]

    post_process(json.dumps(payload), {"day": "20260418"})

    records = load_activity_records("work", "20260422", include_hidden=True)
    assert len(records) == 1
    record = records[0]
    assert record["cancelled"] is True
    assert record["hidden"] is True
    assert record["edits"][-1]["note"] == "created by schedule (cancelled on calendar)"


def test_schedule_post_process_skips_missing_required_field(
    tmp_path,
    monkeypatch,
    caplog,
):
    from solstone.talent.schedule import post_process
    from solstone.think.activities import load_activity_records

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")
    caplog.set_level(logging.WARNING, logger="solstone.talent.schedule")

    post_process(
        json.dumps(
            [
                {
                    "activity": "meeting",
                    "target_date": "2026-04-20",
                    "start": "09:00:00",
                    "end": None,
                    "description": "Missing title should fail.",
                    "details": "",
                    "participation": [],
                    "participation_confidence": 0.5,
                    "facet": "work",
                    "cancelled": False,
                }
            ]
        ),
        {"day": "20260418"},
    )

    assert load_activity_records("work", "20260420", include_hidden=True) == []
    assert "missing required field 'title'" in caplog.text


def test_schedule_post_process_skips_unknown_facet(tmp_path, monkeypatch, caplog):
    from solstone.talent.schedule import post_process

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")
    caplog.set_level(logging.WARNING, logger="solstone.talent.schedule")

    post_process(
        json.dumps(
            [
                {
                    "activity": "meeting",
                    "target_date": "2026-04-20",
                    "start": "09:00:00",
                    "end": None,
                    "title": "Wrong facet",
                    "description": "This facet should be rejected.",
                    "details": "",
                    "participation": [],
                    "participation_confidence": 0.5,
                    "facet": "missing",
                    "cancelled": False,
                }
            ]
        ),
        {"day": "20260418"},
    )

    assert "unknown facet 'missing'" in caplog.text


def test_schedule_post_process_skips_empty_facet_from_zero_facet_hydration(
    tmp_path, monkeypatch, caplog
):
    from jsonschema import Draft202012Validator

    from solstone.talent.schedule import post_process
    from solstone.think.activities import load_activity_records
    from solstone.think.talent import hydrate_runtime_enums

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr("solstone.think.talent._valid_runtime_facets", lambda: [])
    _write_facet(tmp_path, "work")
    caplog.set_level(logging.WARNING, logger="solstone.talent.schedule")
    schema_path = (
        Path(__file__).resolve().parents[1]
        / "solstone"
        / "talent"
        / "schedule.schema.json"
    )
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    hydrated_schema = hydrate_runtime_enums(schema)
    event = {
        "activity": "meeting",
        "target_date": "2026-04-20",
        "start": "09:00:00",
        "end": None,
        "title": "Empty facet",
        "description": "This facet should be rejected by the hook.",
        "details": "",
        "participation": [],
        "participation_confidence": 0.5,
        "facet": "",
        "cancelled": False,
    }
    payload = {"events": [event]}

    assert list(Draft202012Validator(hydrated_schema).iter_errors(payload)) == []
    assert post_process(json.dumps(payload), {"day": "20260418"}) is None

    assert load_activity_records("work", "20260420", include_hidden=True) == []
    assert "missing required field 'facet'" in caplog.text


def test_schedule_post_process_skips_non_future_items(tmp_path, monkeypatch, caplog):
    from solstone.talent.schedule import post_process
    from solstone.think.activities import load_activity_records

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")
    caplog.set_level(logging.WARNING, logger="solstone.talent.schedule")

    post_process(
        json.dumps(
            [
                {
                    "activity": "meeting",
                    "target_date": "2026-04-18",
                    "start": "09:00:00",
                    "end": None,
                    "title": "Too soon",
                    "description": "Should be dropped because it is not future-dated.",
                    "details": "",
                    "participation": [],
                    "participation_confidence": 0.5,
                    "facet": "work",
                    "cancelled": False,
                }
            ]
        ),
        {"day": "20260418"},
    )

    assert load_activity_records("work", "20260418", include_hidden=True) == []
    assert "target_date must be after context day" in caplog.text


def test_schedule_post_process_logs_error_on_invalid_json(
    tmp_path, monkeypatch, caplog
):
    from solstone.talent.schedule import post_process

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")
    caplog.set_level(logging.ERROR, logger="solstone.talent.schedule")

    assert post_process("{not valid json", {"day": "20260418"}) is None
    assert "failed to parse JSON" in caplog.text


def test_schedule_post_process_is_idempotent(tmp_path, monkeypatch):
    from solstone.talent.schedule import post_process
    from solstone.think.activities import load_activity_records

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")

    payload = json.dumps(
        [
            {
                "activity": "call",
                "target_date": "2026-04-21",
                "start": "10:30:00",
                "end": "11:00:00",
                "title": "Mari Zumbro intro",
                "description": "First call with Mari Zumbro.",
                "details": "Google Meet",
                "participation": [],
                "participation_confidence": 0.9,
                "facet": "work",
                "cancelled": False,
            }
        ]
    )

    post_process(payload, {"day": "20260418"})
    post_process(payload, {"day": "20260418"})

    records = load_activity_records("work", "20260421", include_hidden=True)
    assert len(records) == 1
    assert records[0]["id"] == "anticipated_call_103000_0421"


def test_schedule_post_process_fuzzy_supersedes_previous_record(tmp_path, monkeypatch):
    from solstone.talent.schedule import post_process
    from solstone.think.activities import append_activity_record, load_activity_records

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")

    append_activity_record(
        "work",
        "20260421",
        {
            "id": "anticipated_call_100000_0421",
            "activity": "call",
            "target_date": "2026-04-21",
            "start": "10:00:00",
            "end": "10:30:00",
            "title": "Mari Zumbro intro",
            "description": "Old version",
            "details": "",
            "facet": "work",
            "source": "anticipated",
            "participation": [],
            "active_entities": [],
            "participation_confidence": 0.8,
            "cancelled": False,
            "hidden": False,
        },
    )

    post_process(
        json.dumps(
            [
                {
                    "activity": "call",
                    "target_date": "2026-04-21",
                    "start": "10:30:00",
                    "end": "11:00:00",
                    "title": "Mari Zumbro intro",
                    "description": "Updated invite",
                    "details": "Google Meet",
                    "participation": [],
                    "participation_confidence": 0.9,
                    "facet": "work",
                    "cancelled": False,
                }
            ]
        ),
        {"day": "20260418"},
    )

    records = {
        record["id"]: record
        for record in load_activity_records("work", "20260421", include_hidden=True)
    }
    assert set(records) == {
        "anticipated_call_100000_0421",
        "anticipated_call_103000_0421",
    }
    assert records["anticipated_call_100000_0421"]["hidden"] is True
    assert (
        records["anticipated_call_100000_0421"]["edits"][-1]["note"]
        == "superseded by anticipated_call_103000_0421"
    )
    assert records["anticipated_call_103000_0421"]["hidden"] is False


def test_schedule_post_process_cancelled_record_supersedes_pending(
    tmp_path,
    monkeypatch,
):
    from solstone.talent.schedule import post_process
    from solstone.think.activities import append_activity_record, load_activity_records

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    _write_facet(tmp_path, "work")

    append_activity_record(
        "work",
        "20260424",
        {
            "id": "anticipated_meeting_090000_0424",
            "activity": "meeting",
            "target_date": "2026-04-24",
            "start": "09:00:00",
            "end": "10:00:00",
            "title": "Scott Ward standup",
            "description": "Pending version",
            "details": "",
            "facet": "work",
            "source": "anticipated",
            "participation": [],
            "active_entities": [],
            "participation_confidence": 0.85,
            "cancelled": False,
            "hidden": False,
        },
    )

    post_process(
        json.dumps(
            [
                {
                    "activity": "meeting",
                    "target_date": "2026-04-24",
                    "start": "09:30:00",
                    "end": "10:00:00",
                    "title": "Scott Ward standup",
                    "description": "Calendar now shows it cancelled.",
                    "details": "Recurring invite",
                    "participation": [],
                    "participation_confidence": 0.85,
                    "facet": "work",
                    "cancelled": True,
                }
            ]
        ),
        {"day": "20260418"},
    )

    records = {
        record["id"]: record
        for record in load_activity_records("work", "20260424", include_hidden=True)
    }
    assert set(records) == {
        "anticipated_meeting_090000_0424",
        "anticipated_meeting_093000_0424",
    }
    assert records["anticipated_meeting_090000_0424"]["hidden"] is True
    assert records["anticipated_meeting_093000_0424"]["hidden"] is True
    assert (
        records["anticipated_meeting_090000_0424"]["edits"][-1]["note"]
        == "superseded by anticipated_meeting_093000_0424"
    )
    assert (
        records["anticipated_meeting_093000_0424"]["edits"][-1]["note"]
        == "created by schedule (cancelled on calendar)"
    )
