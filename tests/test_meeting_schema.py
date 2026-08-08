# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

from jsonschema import Draft202012Validator

from solstone.observe.categories import meeting as meeting_mod
from solstone.think.describe_categories import CATEGORIES


def _load_schema() -> dict:
    return json.loads(
        (
            Path(meeting_mod.__file__).resolve().parent / "meeting.schema.json"
        ).read_text(encoding="utf-8")
    )


def test_meeting_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_schema())


def test_meeting_schema_accepts_and_rejects_expected_values():
    validator = Draft202012Validator(_load_schema())

    assert validator.is_valid(
        {
            "platform": "zoom",
            "participants": [
                {"name": "Alice", "status": "active", "video": True, "box_2d": None},
            ],
            "screen_share": None,
        }
    )
    assert validator.is_valid(
        {
            "platform": "teams",
            "participants": [
                {
                    "name": "Bob",
                    "status": "presenting",
                    "video": True,
                    "box_2d": [0, 10, 20, 30],
                },
            ],
            "screen_share": {
                "box_2d": [40, 50, 60, 70],
                "presenter": "Bob",
                "description": "Showing a roadmap deck.",
                "formatted_text": "# Roadmap",
            },
        }
    )
    assert not validator.is_valid(
        {
            "platform": "hangouts",
            "participants": [
                {"name": "Alice", "status": "active", "video": True, "box_2d": None},
            ],
            "screen_share": None,
        }
    )
    assert not validator.is_valid(
        {
            "platform": "zoom",
            "participants": [
                {"name": "Alice", "status": "talking", "video": True, "box_2d": None},
            ],
            "screen_share": None,
        }
    )
    assert not validator.is_valid(
        {
            "platform": "zoom",
            "participants": ["Alice"],
            "screen_share": None,
        }
    )
    assert not validator.is_valid(
        {
            "platform": "zoom",
            "participants": [
                {"name": "Alice", "status": "active", "video": True, "box_2d": None},
            ],
            "screen_share": None,
            "extra": True,
        }
    )
    assert not validator.is_valid(
        {
            "platform": "zoom",
            "participants": [
                {"status": "active", "video": True, "box_2d": None},
            ],
            "screen_share": None,
        }
    )
    assert not validator.is_valid(
        {
            "platform": "zoom",
            "participants": [
                {"name": "Alice", "status": "active", "video": True},
            ],
            "screen_share": None,
        }
    )


def test_discover_categories_attaches_meeting_schema():
    expected = _load_schema()

    assert CATEGORIES["meeting"]["json_schema"] == expected
    assert {
        name for name, meta in CATEGORIES.items() if "json_schema" in meta
    } == {
        name
        for name, meta in CATEGORIES.items()
        if meta["output"] == "json"
    }


def test_meeting_formatter_skips_non_dict_participant(caplog):
    with caplog.at_level("WARNING", logger="solstone.observe.categories.meeting"):
        result = meeting_mod.format(
            {
                "platform": "zoom",
                "participants": [
                    "Alice",
                    {"name": "Bob", "status": "active", "video": False},
                ],
                "screen_share": None,
            },
            {},
        )

    assert "🔇 Bob (active)" in result
    assert "Alice" not in result
    assert "skipping non-dict participant" in caplog.text
