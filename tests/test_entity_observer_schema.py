# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path

from jsonschema import Draft202012Validator

from solstone.think.talent import get_talent

SCHEMA_PATH = (
    Path(__file__).parent.parent
    / "solstone"
    / "apps"
    / "entities"
    / "talent"
    / "entity_observer.schema.json"
)


def _load_schema() -> dict:
    return json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))


def test_schema_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_schema())


def test_talent_exposes_json_schema():
    assert get_talent("entities:entity_observer")["json_schema"] == _load_schema()


def test_valid_payload_empty_observations():
    validator = Draft202012Validator(_load_schema())

    assert validator.is_valid(
        {"observations": [], "skipped": [], "summary": "nothing today"}
    )


def test_valid_payload_single_group_single_item():
    validator = Draft202012Validator(_load_schema())

    assert validator.is_valid(
        {
            "observations": [
                {
                    "entity_id": "alice_johnson",
                    "items": [
                        {
                            "content": "Prefers async communication",
                            "reasoning": "Durable working style preference.",
                        }
                    ],
                }
            ],
            "skipped": [],
            "summary": "one observation",
        }
    )


def test_valid_payload_multi_group():
    validator = Draft202012Validator(_load_schema())

    assert validator.is_valid(
        {
            "observations": [
                {
                    "entity_id": "alice_johnson",
                    "items": [
                        {
                            "content": "Prefers async communication",
                            "reasoning": "Durable working style preference.",
                        },
                        {
                            "content": "Works Pacific time hours",
                            "reasoning": "Stable schedule pattern.",
                        },
                    ],
                },
                {
                    "entity_id": "verona_platform",
                    "items": [
                        {
                            "content": "Uses event sourcing in core workflows",
                            "reasoning": "Architectural constraint.",
                        }
                    ],
                },
            ],
            "skipped": ["bob_smith"],
            "summary": "three observations across two entities",
        }
    )


def test_invalid_missing_top_required():
    validator = Draft202012Validator(_load_schema())

    assert not validator.is_valid({"observations": [], "skipped": []})


def test_invalid_missing_group_entity_id():
    validator = Draft202012Validator(_load_schema())

    assert not validator.is_valid(
        {
            "observations": [
                {
                    "items": [
                        {
                            "content": "Prefers async communication",
                            "reasoning": "Durable working style preference.",
                        }
                    ]
                }
            ],
            "skipped": [],
            "summary": "missing entity id",
        }
    )


def test_invalid_empty_content():
    validator = Draft202012Validator(_load_schema())

    assert not validator.is_valid(
        {
            "observations": [
                {
                    "entity_id": "alice_johnson",
                    "items": [{"content": 7, "reasoning": "Durable preference."}],
                }
            ],
            "skipped": [],
            "summary": "invalid content",
        }
    )


def test_invalid_extra_property_on_group():
    validator = Draft202012Validator(_load_schema())

    assert not validator.is_valid(
        {
            "observations": [
                {
                    "entity_id": "alice_johnson",
                    "items": [
                        {
                            "content": "Prefers async communication",
                            "reasoning": "Durable working style preference.",
                        }
                    ],
                    "extra": True,
                }
            ],
            "skipped": [],
            "summary": "extra property",
        }
    )
