# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path
from unittest.mock import AsyncMock, patch

import pytest
from jsonschema import Draft202012Validator

from solstone.observe import category_registry as registry_mod
from solstone.observe.categories import messaging as messaging_mod
from solstone.think.batch import Batch
from solstone.think.schema_bounds import unbounded_nodes


def _load_schema() -> dict:
    return json.loads(
        (
            Path(registry_mod.__file__).resolve().parent
            / "categories"
            / "messaging.schema.json"
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

    assert registry_mod.CATEGORIES["messaging"]["json_schema"] == expected
    assert registry_mod.CATEGORIES["messaging"]["output"] == "json"


@pytest.mark.asyncio
@patch("solstone.think.batch.agenerate_with_result", new_callable=AsyncMock)
async def test_messaging_extract_batch_call_passes_schema(mock_agenerate):
    mock_agenerate.return_value = {
        "text": json.dumps(_valid_payload()),
        "finish_reason": "stop",
    }

    cat_meta = registry_mod.CATEGORIES["messaging"]
    batch = Batch(max_concurrent=1)
    req = batch.create(
        contents="Analyze this messaging screenshot.",
        context=cat_meta["context"],
        json_schema=cat_meta["json_schema"],
    )
    batch.add(req)

    results = []
    async for completed_req in batch.drain_batch():
        results.append(completed_req)

    assert len(results) == 1
    assert mock_agenerate.call_args.kwargs["json_schema"] == _load_schema()


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
