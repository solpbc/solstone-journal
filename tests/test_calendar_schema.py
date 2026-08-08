# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

from jsonschema import Draft202012Validator

from solstone.observe.categories import calendar as calendar_mod
from solstone.think.describe_categories import CATEGORIES
from solstone.think.schema_bounds import unbounded_nodes


def _load_schema() -> dict:
    return json.loads(
        (
            Path(calendar_mod.__file__).resolve().parent / "calendar.schema.json"
        ).read_text(encoding="utf-8")
    )


def _valid_payload() -> dict:
    return {
        "app": "Google Calendar",
        "view": "week",
        "range": "Apr 13 - Apr 19, 2026",
        "events": [
            {
                "title": "Planning review",
                "start": "Tue 10:00 AM",
                "end": "11:00 AM",
                "location": "Conference Room A",
                "conferencing": "Google Meet",
                "guests": ["Alice", "Bob"],
                "status": "accepted",
                "recurrence": None,
                "calendar": "Work",
                "description": "Visible event notes",
            },
        ],
        "availability": ["Tue 2:00 PM"],
        "notes": "Timezone: America/Denver",
    }


def test_calendar_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_schema())


def test_calendar_schema_accepts_and_rejects_expected_values():
    validator = Draft202012Validator(_load_schema())

    assert validator.is_valid(_valid_payload())

    bad_enum = _valid_payload()
    bad_enum["view"] = "list"
    assert not validator.is_valid(bad_enum)

    extra_property = _valid_payload()
    extra_property["extra"] = True
    assert not validator.is_valid(extra_property)

    missing_required = _valid_payload()
    del missing_required["events"]
    assert not validator.is_valid(missing_required)

    wrong_item_type = _valid_payload()
    wrong_item_type["events"] = ["Planning review"]
    assert not validator.is_valid(wrong_item_type)


def test_discover_categories_attaches_calendar_schema():
    expected = _load_schema()

    assert CATEGORIES["calendar"]["json_schema"] == expected
    assert CATEGORIES["calendar"]["output"] == "json"


def test_calendar_schema_has_no_unbounded_nodes():
    assert unbounded_nodes(_load_schema()) == []


def test_calendar_formatter_renders_valid_dict():
    result = calendar_mod.format(_valid_payload(), {})

    assert "**Calendar** (Google Calendar - week)" in result
    assert "*Apr 13 - Apr 19, 2026*" in result
    assert "- **Planning review** (Tue 10:00 AM - 11:00 AM) [accepted]" in result
    assert "  - Location: Conference Room A" in result
    assert "  - Conferencing: Google Meet" in result
    assert "  - Guests: Alice, Bob" in result
    assert "  - Calendar: Work" in result
    assert "  - Description: Visible event notes" in result
    assert "**Availability:** Tue 2:00 PM" in result
    assert "Timezone: America/Denver" in result


def test_calendar_formatter_returns_empty_for_non_dict():
    assert calendar_mod.format("# [Calendar - Week]", {}) == ""


def test_calendar_formatter_skips_non_dict_event(caplog):
    payload = _valid_payload()
    payload["events"] = [
        "Planning review",
        {
            "title": "Follow-up",
            "start": None,
            "end": None,
            "location": None,
            "conferencing": None,
            "guests": [],
            "status": "unknown",
            "recurrence": None,
            "calendar": None,
            "description": None,
        },
    ]

    with caplog.at_level("WARNING", logger="solstone.observe.categories.calendar"):
        result = calendar_mod.format(payload, {})

    assert "**Calendar** (Google Calendar - week)" in result
    assert "- **Follow-up**" in result
    assert "Planning review" not in result
    assert "skipping non-dict event" in caplog.text
