# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import copy
import json
from pathlib import Path

import frontmatter
from jsonschema import Draft202012Validator

from solstone.think.activities import DEFAULT_ACTIVITIES
from solstone.think.talent import (
    RUNTIME_FACETS_SENTINEL,
    get_talent,
    hydrate_runtime_enums,
)

SENSE_PATH = Path(__file__).resolve().parents[1] / "solstone" / "talent" / "sense.md"
SENSE_SCHEMA_PATH = SENSE_PATH.with_suffix(".schema.json")


def _section(text: str, start: str, end: str | None = None) -> str:
    section_start = text.index(start)
    if end is None:
        return text[section_start:]
    section_end = text.index(end, section_start)
    return text[section_start:section_end]


def test_sense_prompt_parses_and_documents_role_and_source():
    post = frontmatter.load(SENSE_PATH)

    assert post.metadata["tier"] == 3

    output_schema = _section(
        post.content, "## Output Schema", "## Field-by-Field Instructions"
    )
    entities = _section(post.content, "### entities", "### facets")
    entity_props = get_talent("sense")["json_schema"]["properties"]["entities"][
        "items"
    ]["properties"]

    assert post.metadata["schema"] == "sense.schema.json"
    assert "Authoritative schema: `sense.schema.json`." in output_schema
    assert set(entity_props["role"]["enum"]) == {"attendee", "mentioned"}
    assert set(entity_props["source"]["enum"]) == {
        "voice",
        "speaker_label",
        "transcript",
        "screen",
        "other",
    }
    assert "#### role" in entities
    assert "#### source" in entities


def test_sense_loaded_json_schema_matches_on_disk_schema():
    on_disk = json.loads(SENSE_SCHEMA_PATH.read_text(encoding="utf-8"))

    assert get_talent("sense")["json_schema"] == on_disk


def test_content_type_enum_matches_default_activities_drift_detector():
    schedule_only = "Scheduled events emitted by talent/schedule.md"
    expected = [
        a["id"]
        for a in DEFAULT_ACTIVITIES
        if schedule_only not in a.get("instructions", "")
    ] + ["idle"]
    schema = json.loads(SENSE_SCHEMA_PATH.read_text(encoding="utf-8"))

    assert schema["properties"]["content_type"]["enum"] == expected


def test_sense_schema_facet_uses_runtime_sentinel_constant():
    schema = json.loads(SENSE_SCHEMA_PATH.read_text(encoding="utf-8"))
    facet_schema = schema["properties"]["facets"]["items"]["properties"]["facet"]

    assert facet_schema["enum"] == [RUNTIME_FACETS_SENTINEL]


def test_hydrate_runtime_enums_replaces_facet_sentinel(monkeypatch):
    monkeypatch.setattr(
        "solstone.think.talent.get_facets",
        lambda: {"alpha": {}, "Beta": {}, "weird,name": {}, "valid_one": {}},
    )
    schema = {
        "properties": {"facet": {"type": "string", "enum": [RUNTIME_FACETS_SENTINEL]}}
    }

    hydrated = hydrate_runtime_enums(schema)

    assert hydrated["properties"]["facet"]["enum"] == ["alpha", "valid_one"]


def test_hydrate_runtime_enums_preserves_facet_minItems_when_facets_exist(
    monkeypatch,
):
    monkeypatch.setattr(
        "solstone.think.talent.get_facets",
        lambda: {"alpha": {}, "Beta": {}, "weird,name": {}, "valid_one": {}},
    )
    schema = {
        "type": "object",
        "properties": {
            "facets": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "properties": {
                        "facet": {
                            "type": "string",
                            "enum": [RUNTIME_FACETS_SENTINEL],
                        }
                    },
                },
            }
        },
    }

    hydrated = hydrate_runtime_enums(schema)
    facets_node = hydrated["properties"]["facets"]
    facet_schema = facets_node["items"]["properties"]["facet"]

    assert facets_node["minItems"] == 1
    assert facet_schema["enum"] == ["alpha", "valid_one"]
    Draft202012Validator.check_schema(hydrated)


def test_hydrate_runtime_enums_empty_facets_fallback(monkeypatch):
    monkeypatch.setattr("solstone.think.talent.get_facets", lambda: {})
    schema = {
        "type": "object",
        "properties": {"facet": {"type": "string", "enum": [RUNTIME_FACETS_SENTINEL]}},
    }

    hydrated = hydrate_runtime_enums(schema)
    facet_schema = hydrated["properties"]["facet"]

    assert facet_schema == {"type": "string"}
    Draft202012Validator.check_schema(hydrated)


def test_hydrate_runtime_enums_keeps_portable_facet_shape_on_empty_facets_fallback(
    monkeypatch,
):
    monkeypatch.setattr("solstone.think.talent.get_facets", lambda: {})
    schema = {
        "type": "object",
        "properties": {
            "facets": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "facet": {
                            "type": "string",
                            "enum": [RUNTIME_FACETS_SENTINEL],
                        }
                    },
                },
            }
        },
    }

    hydrated = hydrate_runtime_enums(schema)
    facets_node = hydrated["properties"]["facets"]
    facet_schema = facets_node["items"]["properties"]["facet"]

    assert "minItems" not in facets_node
    assert facet_schema == {"type": "string"}
    Draft202012Validator.check_schema(hydrated)


def test_hydrate_runtime_enums_idempotent_and_pure(monkeypatch):
    monkeypatch.setattr("solstone.think.talent.get_facets", lambda: {})
    original = {"type": "object", "properties": {"x": {"type": "string"}}}
    saved_copy = copy.deepcopy(original)

    hydrated = hydrate_runtime_enums(original)

    assert hydrated == original
    assert original == saved_copy
    assert hydrated is not original
    assert hydrate_runtime_enums(hydrated) == hydrated


def test_hydrate_runtime_enums_none_passthrough():
    assert hydrate_runtime_enums(None) is None


def test_role_and_source_do_not_leak_into_other_sense_sections():
    content = frontmatter.load(SENSE_PATH).content

    sections = [
        _section(content, "### density", "### content_type"),
        _section(content, "### content_type", "### activity_summary"),
        _section(content, "### activity_summary", "### entities"),
        _section(content, "### facets", "### meeting_detected"),
        _section(content, "### meeting_detected", "### speakers"),
        _section(content, "### speakers", "### recommend"),
        _section(content, "### recommend", "### emotional_register"),
        _section(content, "### emotional_register", "## Rules"),
        _section(content, "## Rules"),
    ]

    for section in sections:
        assert "attendee|mentioned" not in section
        assert "voice|speaker_label|transcript|screen|other" not in section
        assert "#### role" not in section
        assert "#### source" not in section
