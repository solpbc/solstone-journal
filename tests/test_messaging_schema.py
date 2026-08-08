# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

from jsonschema import Draft202012Validator

from solstone.observe.categories import messaging as messaging_mod
from solstone.think.describe_categories import CATEGORIES
from solstone.think.schema_bounds import unbounded_nodes


def _load_schema() -> dict:
    return json.loads(
        (
            Path(messaging_mod.__file__).resolve().parent / "messaging.schema.json"
        ).read_text(encoding="utf-8")
    )


def _valid_payload() -> dict:
    return {
        "app": "Signal",
        "thread": "Bluesky Board ++",
        "view": "conversation",
        "messages": [
            {
                "sender": "Alice",
                "timestamp": "2:34 PM",
                "subject": None,
                "text": "Hello\n> quoted text",
            },
        ],
    }


def test_messaging_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_schema())


def test_messaging_schema_accepts_and_rejects_expected_values():
    validator = Draft202012Validator(_load_schema())

    assert validator.is_valid(_valid_payload())

    bad_enum = _valid_payload()
    bad_enum["view"] = "thread"
    assert not validator.is_valid(bad_enum)

    extra_property = _valid_payload()
    extra_property["extra"] = True
    assert not validator.is_valid(extra_property)

    missing_required = _valid_payload()
    del missing_required["thread"]
    assert not validator.is_valid(missing_required)

    wrong_item_type = _valid_payload()
    wrong_item_type["messages"] = ["Alice: hello"]
    assert not validator.is_valid(wrong_item_type)


def test_discover_categories_attaches_messaging_schema():
    expected = _load_schema()

    assert CATEGORIES["messaging"]["json_schema"] == expected
    assert CATEGORIES["messaging"]["output"] == "json"


def test_messaging_schema_has_no_unbounded_nodes():
    assert unbounded_nodes(_load_schema()) == []


def test_messaging_formatter_renders_valid_dict():
    result = messaging_mod.format(
        {
            "app": "Gmail",
            "thread": "Inbox",
            "view": "inbox",
            "messages": [
                {
                    "sender": "Alice",
                    "timestamp": "2:34 PM",
                    "subject": "Project update",
                    "text": "Latest visible message",
                },
                {
                    "sender": "Bob",
                    "timestamp": None,
                    "subject": None,
                    "text": "Reply\n> quoted context",
                },
            ],
        },
        {},
    )

    assert "**Messaging** (Gmail - Inbox)" in result
    assert "**Alice** (2:34 PM): Project update - Latest visible message" in result
    assert "**Bob**: Reply\n> quoted context" in result


def test_messaging_formatter_returns_empty_for_non_dict():
    assert messaging_mod.format("**Alice**: Hello", {}) == ""


def test_messaging_formatter_skips_non_dict_message(caplog):
    with caplog.at_level("WARNING", logger="solstone.observe.categories.messaging"):
        result = messaging_mod.format(
            {
                "app": "Signal",
                "thread": "Team",
                "view": "conversation",
                "messages": [
                    "Alice: hello",
                    {
                        "sender": "Bob",
                        "timestamp": None,
                        "subject": None,
                        "text": "Hi there",
                    },
                ],
            },
            {},
        )

    assert "**Messaging** (Signal - Team)" in result
    assert "**Bob**: Hi there" in result
    assert "Alice: hello" not in result
    assert "skipping non-dict message" in caplog.text
