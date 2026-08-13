# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest
from jsonschema import Draft202012Validator

from solstone.observe import category_registry as registry_mod
from solstone.observe.categories import calendar as calendar_mod
from solstone.think.batch import Batch
from solstone.think.schema_bounds import unbounded_nodes


def _load_schema() -> dict:
    return json.loads(
        (
            Path(registry_mod.__file__).resolve().parent
            / "categories"
            / "calendar.schema.json"
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

    assert registry_mod.CATEGORIES["calendar"]["json_schema"] == expected
    assert registry_mod.CATEGORIES["calendar"]["output"] == "json"


@pytest.mark.asyncio
@patch("solstone.think.batch.agenerate_with_result", new_callable=AsyncMock)
async def test_calendar_extract_batch_call_passes_schema(mock_agenerate):
    mock_agenerate.return_value = {
        "text": json.dumps(_valid_payload()),
        "finish_reason": "stop",
    }

    cat_meta = registry_mod.CATEGORIES["calendar"]
    batch = Batch(max_concurrent=1)
    req = batch.create(
        contents="Analyze this calendar screenshot.",
        context=cat_meta["context"],
        json_schema=cat_meta["json_schema"],
    )
    batch.add(req)

    results = []
    async for completed_req in batch.drain_batch():
        results.append(completed_req)

    assert len(results) == 1
    assert mock_agenerate.call_args.kwargs["json_schema"] == _load_schema()


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
