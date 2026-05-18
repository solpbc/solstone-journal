# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
from pathlib import Path

from solstone.apps.entities.talent.entity_observer import post_process, pre_process
from solstone.think.entities.context import assemble_observer_context
from solstone.think.entities.journal import clear_journal_entity_cache
from solstone.think.entities.loading import clear_entity_loading_cache
from solstone.think.entities.observations import (
    clear_observation_cache,
    load_observations,
)
from solstone.think.entities.relationships import clear_relationship_caches
from solstone.think.talent import get_talent


def _set_journal(monkeypatch, path: str) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", path)
    clear_entity_loading_cache()
    clear_observation_cache()
    clear_relationship_caches()
    clear_journal_entity_cache()


def _write_json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data), encoding="utf-8")


def _write_jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows),
        encoding="utf-8",
    )


def _attach_entity(
    root: Path,
    facet: str,
    entity_id: str,
    name: str,
    entity_type: str = "Person",
    description: str = "Attached entity",
) -> None:
    _write_json(
        root / "entities" / entity_id / "entity.json",
        {"id": entity_id, "name": name, "type": entity_type},
    )
    _write_json(
        root / "facets" / facet / "entities" / entity_id / "entity.json",
        {"entity_id": entity_id, "description": description},
    )


def _obs_path(facet: str, entity_id: str) -> Path:
    return Path("facets") / facet / "entities" / entity_id / "observations.jsonl"


# ============================================================================
# Context assembly tests
# ============================================================================


def test_assemble_observer_context_with_fixture_data(monkeypatch):
    _set_journal(monkeypatch, "tests/fixtures/journal")

    result = assemble_observer_context("capulet", "20260304")

    assert result
    assert "Juliet Capulet" in result
    assert "Knowledge Graph" in result
    assert (
        "Prepared revenue projections for Verona Platform board presentation" in result
    )


def test_assemble_observer_context_no_kg(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    _write_jsonl(
        tmp_path / "facets" / facet / "entities" / f"{day}.jsonl",
        [
            {
                "id": "alice_johnson",
                "type": "Person",
                "name": "Alice Johnson",
                "description": "Detected from activity",
            }
        ],
    )

    result = assemble_observer_context(facet, day)

    assert result
    assert "No knowledge graph available for this day." in result
    assert "Alice Johnson" in result


def test_assemble_observer_context_no_active_entities(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    _attach_entity(tmp_path, "work", "alice_johnson", "Alice Johnson")

    result = assemble_observer_context("work", "20260304")

    assert "No active entities" in result


def test_assemble_observer_context_empty_facet(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    (tmp_path / "facets" / "empty" / "entities").mkdir(parents=True)

    result = assemble_observer_context("empty", "20260304")

    assert "No active entities" in result


def test_assemble_observer_context_observations_sliced(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    entity_id = "alice_johnson"
    _attach_entity(tmp_path, facet, entity_id, "Alice Johnson")
    _write_jsonl(
        tmp_path / "facets" / facet / "entities" / f"{day}.jsonl",
        [
            {
                "id": entity_id,
                "type": "Person",
                "name": "Alice Johnson",
                "description": "",
            }
        ],
    )
    _write_jsonl(
        tmp_path / _obs_path(facet, entity_id),
        [
            {"content": "Observation 1", "observed_at": 1, "source_day": "20260301"},
            {"content": "Observation 2", "observed_at": 2, "source_day": "20260302"},
            {"content": "Observation 3", "observed_at": 3, "source_day": "20260303"},
            {"content": "Observation 4", "observed_at": 4, "source_day": "20260304"},
            {"content": "Observation 5", "observed_at": 5, "source_day": "20260305"},
        ],
    )

    result = assemble_observer_context(facet, day)

    assert "Observation 1" not in result
    assert "Observation 2" not in result
    assert result.count("(source: ") == 3


# ============================================================================
# Hook tests
# ============================================================================


def test_pre_process_returns_template_vars(monkeypatch):
    _set_journal(monkeypatch, "tests/fixtures/journal")

    result = pre_process({"facet": "capulet", "day": "20260304"})

    assert isinstance(result, dict)
    assert "template_vars" in result
    assert "observer_context" in result["template_vars"]
    assert result["template_vars"]["observer_context"]


def test_pre_process_missing_facet():
    assert pre_process({"day": "20260304"}) is None


def test_pre_process_missing_day():
    assert pre_process({"facet": "work"}) is None


def test_post_process_persists_observations(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    result = post_process(
        json.dumps(
            {
                "observations": [
                    {
                        "entity_id": "alice_johnson",
                        "items": [
                            {
                                "content": "Prefers morning meetings",
                                "reasoning": "Durable preference",
                            }
                        ],
                    }
                ],
                "skipped": [],
                "summary": "1 entity, 1 observation",
            }
        ),
        {"facet": facet, "day": "20260304"},
    )

    assert result is None
    observations = load_observations(facet, "alice_johnson")
    assert [obs["content"] for obs in observations] == ["Prefers morning meetings"]


def test_post_process_filters_unrecognized_entity(tmp_path, caplog, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    with caplog.at_level(logging.DEBUG):
        result = post_process(
            json.dumps(
                {
                    "observations": [
                        {
                            "entity_id": "unknown_entity",
                            "items": [
                                {
                                    "content": "Should be ignored",
                                    "reasoning": "Unknown entity",
                                }
                            ],
                        }
                    ],
                    "skipped": ["alice_johnson"],
                    "summary": "1 entity skipped",
                }
            ),
            {"facet": facet, "day": "20260304"},
        )

    assert result is None
    assert load_observations(facet, "alice_johnson") == []
    assert load_observations(facet, "unknown_entity") == []
    assert "Skipping unrecognized entity_id: unknown_entity" in caplog.text


def test_post_process_skips_empty_content(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    post_process(
        json.dumps(
            {
                "observations": [
                    {
                        "entity_id": "alice_johnson",
                        "items": [{"content": "", "reasoning": "empty"}],
                    }
                ],
                "skipped": [],
                "summary": "No valid observations",
            }
        ),
        {"facet": facet, "day": "20260304"},
    )

    assert load_observations(facet, "alice_johnson") == []


def test_post_process_skips_non_list_group_items(tmp_path, caplog, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    with caplog.at_level(logging.DEBUG):
        post_process(
            json.dumps(
                {
                    "observations": [
                        {
                            "entity_id": "alice_johnson",
                            "items": "not a list",
                        }
                    ],
                    "skipped": [],
                    "summary": "Malformed items",
                }
            ),
            {"facet": facet, "day": "20260304"},
        )

    assert load_observations(facet, "alice_johnson") == []
    assert "Skipping malformed observation entry" in caplog.text


def test_post_process_skips_group_missing_entity_id(tmp_path, caplog, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    with caplog.at_level(logging.DEBUG):
        post_process(
            json.dumps(
                {
                    "observations": [
                        {
                            "items": [{"content": "x", "reasoning": "y"}],
                        }
                    ],
                    "skipped": [],
                    "summary": "Missing entity id",
                }
            ),
            {"facet": facet, "day": "20260304"},
        )

    assert load_observations(facet, "alice_johnson") == []
    assert "Skipping malformed observation entry" in caplog.text


def test_post_process_deduplicates_existing(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    _write_jsonl(
        tmp_path / _obs_path(facet, "alice_johnson"),
        [{"content": "Prefers morning meetings", "observed_at": 1}],
    )

    post_process(
        json.dumps(
            {
                "observations": [
                    {
                        "entity_id": "alice_johnson",
                        "items": [
                            {
                                "content": "Prefers morning meetings",
                                "reasoning": "dupe",
                            },
                            {
                                "content": "Expert in distributed systems",
                                "reasoning": "new",
                            },
                        ],
                    }
                ],
                "skipped": [],
                "summary": "One duplicate, one new observation",
            }
        ),
        {"facet": facet, "day": "20260304"},
    )

    observations = load_observations(facet, "alice_johnson")
    contents = [obs["content"] for obs in observations]
    assert contents.count("Prefers morning meetings") == 1
    assert "Expert in distributed systems" in contents


def test_post_process_handles_malformed_json():
    assert post_process("not valid json", {"facet": "work", "day": "20260304"}) is None


# ============================================================================
# Agent config test
# ============================================================================


def test_entity_observer_agent_config(monkeypatch):
    _set_journal(monkeypatch, "tests/fixtures/journal")

    config = get_talent("entities:entity_observer")

    assert config["type"] == "generate"
    assert config.get("output") == "json"
    assert config.get("tier") == 2
    assert config.get("thinking_budget") == 2048
    assert config.get("hook", {}).get("pre") == "entities:entity_observer"
    assert config.get("hook", {}).get("post") == "entities:entity_observer"
    assert "$observer_context" in config["user_instruction"]
