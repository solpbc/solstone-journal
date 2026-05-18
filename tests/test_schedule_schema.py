# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

from jsonschema import Draft202012Validator

from solstone.think.activities import DEFAULT_ACTIVITIES
from solstone.think.talent import (
    RUNTIME_FACETS_SENTINEL,
    get_talent,
    hydrate_runtime_enums,
)

TALENT_DIR = Path(__file__).resolve().parents[1] / "solstone" / "talent"
PARTICIPATION_ENTRY_SCHEMA_PATH = TALENT_DIR / "participation_entry.schema.json"
SCHEDULE_SCHEMA_PATH = TALENT_DIR / "schedule.schema.json"

SCHEDULE_REQUIRED_FIELDS = {
    "activity",
    "target_date",
    "start",
    "end",
    "title",
    "description",
    "details",
    "participation",
    "participation_confidence",
    "facet",
    "cancelled",
}


def _load_json(path: Path) -> dict | list:
    return json.loads(path.read_text(encoding="utf-8"))


def _load_schedule_schema() -> dict:
    schema = _load_json(SCHEDULE_SCHEMA_PATH)
    assert isinstance(schema, dict)
    return schema


def _strip_portability_annotations(value):
    if isinstance(value, dict):
        for key in ("minLength", "minimum", "maximum"):
            value.pop(key, None)
        for child in value.values():
            _strip_portability_annotations(child)
    elif isinstance(value, list):
        for child in value:
            _strip_portability_annotations(child)


def _expected_schedule_activity_ids() -> set[str]:
    # Why: `meeting` is emitted by both the schedule talent and sense; the
    # other 9 are schedule-only (their instructions carry the marker).
    schedule_only = {
        a["id"]
        for a in DEFAULT_ACTIVITIES
        if "Scheduled events emitted by talent/schedule.md" in a.get("instructions", "")
    }
    return {"meeting"} | schedule_only


def _sample_schedule_payloads() -> list[list[dict]]:
    return [
        [
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
        ],
        [
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
        ],
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
        ],
        [
            {
                "activity": "deadline",
                "target_date": "2026-05-05",
                "start": None,
                "end": None,
                "title": "Demo Day",
                "description": "Betaworks Camp Demo Day.",
                "details": "Live demo presentation to cohort investors",
                "participation": [],
                "participation_confidence": 0.5,
                "facet": "work",
                "cancelled": False,
            }
        ],
    ]


def test_schedule_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_schedule_schema())


def test_schedule_talent_loads_schema():
    assert get_talent("schedule")["json_schema"] == _load_schedule_schema()


def test_schedule_schema_facet_uses_runtime_sentinel_constant():
    schema = _load_schedule_schema()
    facet_schema = schema["items"]["properties"]["facet"]

    assert facet_schema["enum"] == [RUNTIME_FACETS_SENTINEL]


def test_schedule_activity_enum_matches_default_activities_drift_detector():
    schema = _load_schedule_schema()
    item_schema = schema["items"]

    assert set(item_schema["properties"]["activity"]["enum"]) == (
        _expected_schedule_activity_ids()
    )


def test_schedule_participation_entry_diverges_from_shared_fragment():
    """Schedule omits entity_id because the hook fills it via find_matching_entity."""
    schedule_schema = _load_schedule_schema()
    fragment = _load_json(PARTICIPATION_ENTRY_SCHEMA_PATH)

    assert isinstance(fragment, dict)
    fragment_without_schema = dict(fragment)
    fragment_without_schema["properties"] = dict(fragment_without_schema["properties"])
    fragment_without_schema["properties"].pop("entity_id")
    fragment_without_schema["required"] = [
        key for key in fragment_without_schema["required"] if key != "entity_id"
    ]

    raw_inline_items = dict(
        schedule_schema["items"]["properties"]["participation"]["items"]
    )
    assert "entity_id" in fragment["properties"]
    assert "entity_id" not in raw_inline_items["properties"]
    assert raw_inline_items != fragment

    inline_items = json.loads(json.dumps(raw_inline_items))
    _strip_portability_annotations(inline_items)

    assert inline_items == fragment_without_schema


def test_schedule_schema_mirrors_hook_requirements():
    schedule_schema = _load_schedule_schema()
    item_schema = schedule_schema["items"]
    properties = item_schema["properties"]
    participation_items = properties["participation"]["items"]
    fragment = _load_json(PARTICIPATION_ENTRY_SCHEMA_PATH)

    assert schedule_schema["type"] == "array"
    assert set(item_schema["required"]) == SCHEDULE_REQUIRED_FIELDS
    assert set(properties["activity"]["enum"]) == _expected_schedule_activity_ids()
    assert (
        participation_items["properties"]["role"]["enum"]
        == fragment["properties"]["role"]["enum"]
    )
    assert (
        participation_items["properties"]["source"]["enum"]
        == fragment["properties"]["source"]["enum"]
    )
    assert properties["start"]["type"] == ["string", "null"]
    assert properties["end"]["type"] == ["string", "null"]
    assert properties["cancelled"]["type"] == "boolean"


def test_schedule_hook_fixtures_validate_against_schema(monkeypatch):
    monkeypatch.setattr("solstone.think.talent.get_facets", lambda: {"work": {}})
    validator = Draft202012Validator(hydrate_runtime_enums(_load_schedule_schema()))

    for payload in _sample_schedule_payloads():
        assert list(validator.iter_errors(payload)) == []
