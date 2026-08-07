# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from pathlib import Path

import solstone.apps.entities.talent.entity_observer as entity_observer_hook
import solstone.think.entities.context as entity_context
from solstone.apps.entities.talent.entity_observer import post_process, pre_process
from solstone.think.entities.observations import load_observations
from solstone.think.talent import get_talent


def _set_journal(monkeypatch, path: str) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", path)


def _write_json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data), encoding="utf-8")


def _write_jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows),
        encoding="utf-8",
    )


def _write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def _write_audio_jsonl(path: Path, records: list[dict]) -> None:
    rows = [{"raw": "raw.flac", "model": "whisper-1", "duration": 120}, *records]
    _write_jsonl(path, rows)


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


COUNT_KEYS = ("update", "add", "drop", "keep", "skipped", "relation_unresolved")


def _outcome_path(root: Path, facet: str, day: str) -> Path:
    return root / "facets" / facet / "entities" / f"{day}_observer_outcome.json"


def _load_outcome(root: Path, facet: str, day: str) -> dict:
    return json.loads(_outcome_path(root, facet, day).read_text(encoding="utf-8"))


def _count_sum(outcome: dict) -> int:
    return sum(outcome[key] for key in COUNT_KEYS)


def _assert_outcome_shape(outcome: dict) -> None:
    assert set(outcome) == {*COUNT_KEYS, "error", "ts"}
    assert isinstance(outcome["ts"], int)


def _empty_search(*_args, **_kwargs) -> tuple[int, list[dict]]:
    return 0, []


def _section_for(result: str, entity_name: str) -> str:
    start = result.index(f"#### {entity_name}")
    end = result.find("\n---\n", start)
    if end == -1:
        return result[start:]
    return result[start:end]


# ============================================================================
# Context assembly tests
# ============================================================================


def test_assemble_observer_context_deep_segment_context(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    monkeypatch.setattr(entity_context, "search_journal", _empty_search)
    facet = "work"
    day = "20260405"
    segment = "090000_300"
    composite = f"{day}/default/{segment}"
    unrelated_segment = "100000_300"
    unrelated_composite = f"{day}/default/{unrelated_segment}"
    _attach_entity(
        tmp_path,
        facet,
        "alice_johnson",
        "Alice Johnson",
        description="Strategic owner for partner integrations.",
    )
    _write_jsonl(
        tmp_path / "facets" / facet / "entities" / f"{day}.jsonl",
        [
            {
                "id": "alice_johnson",
                "type": "Person",
                "name": "Alice Johnson",
                "description": "Detected from integration planning.",
                "segments": [composite],
            },
            {
                "id": "bob_lee",
                "type": "Person",
                "name": "Bob Lee",
                "description": "Unattached unrelated detection.",
                "segments": [unrelated_composite],
            },
        ],
    )
    _write_jsonl(
        tmp_path / _obs_path(facet, "alice_johnson"),
        [
            {
                "content": "Existing Alpha observation 1",
                "observed_at": 1,
                "source_day": "20260401",
            },
            {
                "content": "Existing Alpha observation 2",
                "observed_at": 2,
                "source_day": "20260402",
            },
        ],
    )
    seg_dir = tmp_path / "chronicle" / day / "default" / segment
    _write_json(
        seg_dir / "talents" / "sense.json",
        {
            "activity_summary": "Prepared integration roadmap for Alpha Platform.",
            "entities": [
                {
                    "type": "Person",
                    "name": "Alice Johnson",
                    "role": "participant",
                    "source": "audio",
                    "context": "Alice owns the durable partner integration strategy.",
                    "level": 5,
                },
                {
                    "type": "Person",
                    "name": "Bob Lee",
                    "role": "mentioned",
                    "source": "audio",
                    "context": "Bob unrelated sense context should not appear.",
                    "level": 2,
                },
            ],
        },
    )
    _write_audio_jsonl(
        seg_dir / "audio.jsonl",
        [
            {
                "start": f"00:00:{index:02d}",
                "text": f"Alice Johnson transcript line {index}",
            }
            for index in range(1, 14)
        ]
        + [
            {
                "start": "00:01:00",
                "text": "Bob unrelated transcript should not appear.",
            }
        ],
    )
    result = entity_context.assemble_observer_context(facet, day)

    assert "Alice Johnson" in result
    assert "Strategic owner for partner integrations." in result
    assert "0. Existing Alpha observation 1 (source: 20260401)" in result
    assert "1. Existing Alpha observation 2 (source: 20260402)" in result
    assert "Prepared integration roadmap for Alpha Platform." in result
    assert "Alice owns the durable partner integration strategy." in result
    assert "Alice Johnson transcript line 1" in result
    assert "Alice Johnson transcript line 12" in result
    assert "Alice Johnson transcript line 13" not in result
    assert "Bob unrelated sense context should not appear." not in result
    assert "Bob unrelated transcript should not appear." not in result


def test_assemble_observer_context_per_lever_resilience(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    monkeypatch.setattr(entity_context, "search_journal", _empty_search)
    facet = "work"
    day = "20260405"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    broken_composite = f"{day}/default/080000_300"
    sense_only_composite = f"{day}/default/090000_300"
    _write_jsonl(
        tmp_path / "facets" / facet / "entities" / f"{day}.jsonl",
        [
            {
                "id": "alice_johnson",
                "type": "Person",
                "name": "Alice Johnson",
                "description": "Detected from activity",
                "segments": [broken_composite, sense_only_composite],
            }
        ],
    )
    _write_text(
        tmp_path
        / "chronicle"
        / day
        / "default"
        / "080000_300"
        / "talents"
        / "sense.json",
        "{bad json",
    )
    _write_json(
        tmp_path
        / "chronicle"
        / day
        / "default"
        / "090000_300"
        / "talents"
        / "sense.json",
        {
            "activity_summary": "Reviewed resilient source handling.",
            "entities": [
                {
                    "type": "Person",
                    "name": "Alice Johnson",
                    "role": "participant",
                    "source": "audio",
                    "context": "Alice has valid sense even without audio.",
                    "level": 4,
                }
            ],
        },
    )

    result = entity_context.assemble_observer_context(facet, day)

    assert "Alice Johnson" in result
    assert "Alice has valid sense even without audio." in result
    assert f"(segment {broken_composite}: source unavailable)" in result
    assert f"(segment {sense_only_composite}: source unavailable)" not in result


def test_assemble_observer_context_thin_source_marker(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    monkeypatch.setattr(entity_context, "search_journal", _empty_search)
    facet = "work"
    day = "20260405"
    _attach_entity(tmp_path, facet, "alpha_person", "Alpha Person")
    _attach_entity(tmp_path, facet, "beta_person", "Beta Person")
    alpha_composite = f"{day}/default/090000_300"
    _write_jsonl(
        tmp_path / "facets" / facet / "entities" / f"{day}.jsonl",
        [
            {
                "id": "alpha_person",
                "type": "Person",
                "name": "Alpha Person",
                "description": "Rich source entity",
                "segments": [alpha_composite],
            },
            {
                "id": "beta_person",
                "type": "Person",
                "name": "Beta Person",
                "description": "Thin source entity",
            },
        ],
    )
    _write_json(
        tmp_path
        / "chronicle"
        / day
        / "default"
        / "090000_300"
        / "talents"
        / "sense.json",
        {
            "activity_summary": "Alpha created a durable plan.",
            "entities": [
                {
                    "type": "Person",
                    "name": "Alpha Person",
                    "role": "participant",
                    "source": "audio",
                    "context": "Alpha has fresh sense context.",
                    "level": 4,
                }
            ],
        },
    )

    result = entity_context.assemble_observer_context(facet, day)

    alpha_section = _section_for(result, "Alpha Person")
    beta_section = _section_for(result, "Beta Person")
    assert entity_context.THIN_SOURCE_MARKER not in alpha_section
    assert entity_context.THIN_SOURCE_MARKER in beta_section


def test_assemble_observer_context_budget_ceiling(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    monkeypatch.setattr(entity_context, "search_journal", _empty_search)
    facet = "work"
    day = "20260405"
    entity_id = "alice_johnson"
    _attach_entity(tmp_path, facet, entity_id, "Alice Johnson")
    _write_jsonl(
        tmp_path / "facets" / facet / "entities" / f"{day}.jsonl",
        [
            {
                "id": entity_id,
                "type": "Person",
                "name": "Alice Johnson",
                "description": "Budget pressure entity",
            }
        ],
    )
    _write_jsonl(
        tmp_path / _obs_path(facet, entity_id),
        [
            {
                "content": f"Observation {index} " + ("x" * 240),
                "observed_at": index,
                "source_day": day,
            }
            for index in range(140)
        ],
    )

    result = entity_context.assemble_observer_context(facet, day)
    source = Path(__file__).read_text(encoding="utf-8")

    assert len(result) <= entity_context.TOTAL_CHAR_BUDGET
    for forbidden in (
        ("local", "_budget"),
        ("count", "_tokens"),
        ("fit", "_contents"),
    ):
        assert "".join(forbidden) not in source


def test_assemble_observer_context_search_lever(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260405"
    entity_id = "alice_johnson"
    segment = "090000_300"
    composite = f"{day}/default/{segment}"
    _attach_entity(tmp_path, facet, entity_id, "Alice Johnson")
    _write_jsonl(
        tmp_path / "facets" / facet / "entities" / f"{day}.jsonl",
        [
            {
                "id": entity_id,
                "type": "Person",
                "name": "Alice Johnson",
                "description": "",
                "segments": [composite],
            }
        ],
    )

    calls = []

    def fake_search(query: str, **kwargs) -> tuple[int, list[dict]]:
        calls.append((query, kwargs))
        return 3, [
            {
                "text": "Deduped segment evidence should not show.",
                "metadata": {
                    "agent": "audio",
                    "path": f"{composite}/audio.jsonl",
                },
            },
            {
                "text": "Chat noise should not show.",
                "metadata": {"agent": "chat", "path": f"{day}/chat/1"},
            },
            {
                "text": "Related journal evidence says Alice owns partner APIs.",
                "metadata": {"agent": "note_transcript", "path": f"{day}/notes.md"},
            },
        ]

    monkeypatch.setattr(entity_context, "search_journal", fake_search)

    result = entity_context.assemble_observer_context(facet, day)

    assert "Related journal evidence says Alice owns partner APIs." in result
    assert "Deduped segment evidence should not show." not in result
    assert "Chat noise should not show." not in result
    assert entity_context.THIN_SOURCE_MARKER not in result
    assert calls[0][1]["include_total"] is False


def test_assemble_observer_context_no_active_entities(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    _attach_entity(tmp_path, "work", "alice_johnson", "Alice Johnson")

    result = entity_context.assemble_observer_context("work", "20260304")

    assert "No active entities" in result


def test_assemble_observer_context_empty_facet(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    (tmp_path / "facets" / "empty" / "entities").mkdir(parents=True)

    result = entity_context.assemble_observer_context("empty", "20260304")

    assert "No active entities" in result


def test_assemble_observer_context_observations_are_numbered_full_list(
    tmp_path, monkeypatch
):
    _set_journal(monkeypatch, str(tmp_path))
    monkeypatch.setattr(entity_context, "search_journal", _empty_search)
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
            {
                "content": f"Observation {index}",
                "observed_at": index,
                "source_day": f"2026030{index}",
            }
            for index in range(1, 6)
        ],
    )

    result = entity_context.assemble_observer_context(facet, day)

    for index in range(5):
        assert f"{index}. Observation {index + 1}" in result


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


def test_post_process_applies_operations_and_writes_outcome(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    _write_jsonl(
        tmp_path / _obs_path(facet, "alice_johnson"),
        [
            {"content": "Prefers morning meetings", "observed_at": 1},
            {"content": "Uses legacy planning notes", "observed_at": 2},
            {"content": "Works Pacific time hours", "observed_at": 3},
        ],
    )

    result = post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            {
                                "op": "update",
                                "target_index": 0,
                                "content": "Prefers concise morning planning meetings",
                                "target_quote": "morning meetings",
                                "reasoning": "Fresh source narrows the preference.",
                            },
                            {
                                "op": "drop",
                                "target_index": 1,
                                "target_quote": "legacy planning",
                                "reasoning": "Stale duplicate planning note.",
                            },
                            {
                                "op": "keep",
                                "target_index": 2,
                                "reasoning": None,
                            },
                            {
                                "op": "add",
                                "content": "Has deep knowledge of distributed systems",
                                "reasoning": "Durable expertise.",
                            },
                        ],
                    }
                ],
                "summary": "Applied four operations.",
            }
        ),
        {"facet": facet, "day": day},
    )

    assert result is None
    observations = load_observations(facet, "alice_johnson")
    assert [obs["content"] for obs in observations] == [
        "Prefers concise morning planning meetings",
        "Works Pacific time hours",
        "Has deep knowledge of distributed systems",
    ]
    outcome = _load_outcome(tmp_path, facet, day)
    _assert_outcome_shape(outcome)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 1,
        "add": 1,
        "drop": 1,
        "keep": 1,
        "skipped": 0,
        "relation_unresolved": 0,
    }
    assert outcome["error"] is None
    assert _count_sum(outcome) == 4


def test_post_process_unknown_entity_counts_all_operation_rows_skipped(
    tmp_path, monkeypatch
):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    result = post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "unknown_entity",
                        "operations": [
                            {
                                "op": "add",
                                "content": "Should be ignored",
                                "reasoning": "Unknown entity.",
                            },
                            {
                                "op": "drop",
                                "target_index": 0,
                                "reasoning": "Unknown entity.",
                            },
                            "not an object",
                        ],
                    }
                ],
                "summary": "unknown entity",
            }
        ),
        {"facet": facet, "day": day},
    )

    assert result is None
    assert load_observations(facet, "alice_johnson") == []
    assert load_observations(facet, "unknown_entity") == []
    outcome = _load_outcome(tmp_path, facet, day)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 0,
        "add": 0,
        "drop": 0,
        "keep": 0,
        "skipped": 3,
        "relation_unresolved": 0,
    }
    assert outcome["error"] is None
    assert _count_sum(outcome) == 3


def test_post_process_handles_malformed_json_with_zero_outcome(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"

    assert post_process("not valid json", {"facet": facet, "day": day}) is None

    outcome = _load_outcome(tmp_path, facet, day)
    _assert_outcome_shape(outcome)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 0,
        "add": 0,
        "drop": 0,
        "keep": 0,
        "skipped": 0,
        "relation_unresolved": 0,
    }
    assert outcome["error"] is None


def test_post_process_rejects_malformed_ops_and_counts_skipped(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    _write_jsonl(
        tmp_path / _obs_path(facet, "alice_johnson"),
        [{"content": "Existing observation stays unchanged", "observed_at": 1}],
    )

    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            "not an object",
                            {"op": "add", "content": " ", "reasoning": "empty"},
                            {
                                "op": "update",
                                "target_index": True,
                                "content": "Invalid bool index.",
                                "reasoning": "bad index",
                            },
                            {
                                "op": "update",
                                "target_index": 0,
                                "content": "",
                                "reasoning": "empty update",
                            },
                            {
                                "op": "drop",
                                "target_index": "0",
                                "reasoning": "string index",
                            },
                            {
                                "op": "keep",
                                "target_index": None,
                                "reasoning": "missing index",
                            },
                            {"op": "replace", "target_index": 0, "reasoning": "bad"},
                            {
                                "op": "drop",
                                "target_index": 0,
                                "target_quote": 7,
                                "reasoning": "bad quote",
                            },
                            {
                                "op": "add",
                                "content": "Durable valid observation",
                                "reasoning": "valid add",
                            },
                        ],
                    }
                ],
                "summary": "malformed rows",
            }
        ),
        {"facet": facet, "day": day},
    )

    assert [obs["content"] for obs in load_observations(facet, "alice_johnson")] == [
        "Existing observation stays unchanged",
        "Durable valid observation",
    ]
    outcome = _load_outcome(tmp_path, facet, day)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 0,
        "add": 1,
        "drop": 0,
        "keep": 0,
        "skipped": 8,
        "relation_unresolved": 0,
    }
    assert outcome["error"] is None
    assert _count_sum(outcome) == 9


def test_post_process_duplicate_target_index_first_clean_op_wins(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    _write_jsonl(
        tmp_path / _obs_path(facet, "alice_johnson"),
        [
            {"content": "Prefers morning meetings", "observed_at": 1},
            {"content": "Works Pacific time hours", "observed_at": 2},
        ],
    )

    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            {
                                "op": "update",
                                "target_index": 0,
                                "content": "Prefers concise morning planning meetings",
                                "target_quote": "morning meetings",
                                "reasoning": "first clean op wins",
                            },
                            {
                                "op": "drop",
                                "target_index": 0,
                                "target_quote": "morning meetings",
                                "reasoning": "second op skipped",
                            },
                            {
                                "op": "keep",
                                "target_index": 1,
                                "reasoning": "unchanged",
                            },
                        ],
                    }
                ],
                "summary": "duplicate target",
            }
        ),
        {"facet": facet, "day": day},
    )

    observations = load_observations(facet, "alice_johnson")
    assert [obs["content"] for obs in observations] == [
        "Prefers concise morning planning meetings",
        "Works Pacific time hours",
    ]
    outcome = _load_outcome(tmp_path, facet, day)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 1,
        "add": 0,
        "drop": 0,
        "keep": 1,
        "skipped": 1,
        "relation_unresolved": 0,
    }
    assert outcome["error"] is None
    assert _count_sum(outcome) == 3


def test_post_process_storage_failure_sets_error_and_writes_outcome(
    tmp_path, monkeypatch
):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    def fail_record_ops(*_args, **_kwargs):
        raise OSError("disk busy")

    monkeypatch.setattr(entity_observer_hook, "record_observation_ops", fail_record_ops)

    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            {
                                "op": "add",
                                "content": "Will be skipped by storage failure",
                                "reasoning": "valid row",
                            },
                            {
                                "op": "add",
                                "content": "Also skipped by storage failure",
                                "reasoning": "valid row",
                            },
                        ],
                    }
                ],
                "summary": "storage failure",
            }
        ),
        {"facet": facet, "day": day},
    )

    assert load_observations(facet, "alice_johnson") == []
    outcome = _load_outcome(tmp_path, facet, day)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 0,
        "add": 0,
        "drop": 0,
        "keep": 0,
        "skipped": 2,
        "relation_unresolved": 0,
    }
    assert outcome["error"] == "OSError: disk busy"
    assert _count_sum(outcome) == 2


def test_post_process_invalid_entity_id_counts_operation_rows_skipped(
    tmp_path, monkeypatch
):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": 123,
                        "operations": [
                            {
                                "op": "add",
                                "content": "Skipped because entity id is invalid",
                                "reasoning": "bad entity id",
                            }
                        ],
                    },
                    {
                        "entity_id": "alice_johnson",
                        "operations": "not a list",
                    },
                ],
                "summary": "bad entity containers",
            }
        ),
        {"facet": facet, "day": day},
    )

    outcome = _load_outcome(tmp_path, facet, day)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 0,
        "add": 0,
        "drop": 0,
        "keep": 0,
        "skipped": 1,
        "relation_unresolved": 0,
    }
    assert _count_sum(outcome) == 1


def test_post_process_persists_resolved_relation_on_observation(tmp_path, monkeypatch):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    _attach_entity(tmp_path, facet, "bob_lee", "Bob Lee")

    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            {
                                "op": "add",
                                "content": "Pairs with Bob Lee on the platform team",
                                "reasoning": "Durable working relationship.",
                                "relation": {
                                    "kind": "works-with",
                                    "target_name": "Bob Lee",
                                    "note": "",
                                },
                            }
                        ],
                    }
                ],
                "summary": "one relational add",
            }
        ),
        {"facet": facet, "day": day},
    )

    observations = load_observations(facet, "alice_johnson")
    assert len(observations) == 1
    assert observations[0]["relation"] == {
        "kind": "works-with",
        "target_entity_id": "bob_lee",
        "target_name": "Bob Lee",
        "note": "",
    }
    outcome = _load_outcome(tmp_path, facet, day)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 0,
        "add": 1,
        "drop": 0,
        "keep": 0,
        "skipped": 0,
        "relation_unresolved": 0,
    }


def test_post_process_preserves_op_with_unresolvable_relation_target(
    tmp_path, monkeypatch, caplog
):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")

    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            {
                                "op": "add",
                                "content": "Reports to someone we cannot identify",
                                "reasoning": "Relational, but the target is unknown.",
                                "relation": {
                                    "kind": "reports-to",
                                    "target_name": "Nobody Visible",
                                    "note": "",
                                },
                            }
                        ],
                    }
                ],
                "summary": "unresolvable relation target",
            }
        ),
        {"facet": facet, "day": day},
    )

    observations = load_observations(facet, "alice_johnson")
    assert len(observations) == 1
    assert observations[0]["content"] == "Reports to someone we cannot identify"
    assert observations[0]["relation"] == {
        "kind": "reports-to",
        "target_entity_id": None,
        "target_name": "Nobody Visible",
        "note": "",
    }
    assert "unresolved relation target" in caplog.text
    outcome = _load_outcome(tmp_path, facet, day)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 0,
        "add": 1,
        "drop": 0,
        "keep": 0,
        "skipped": 0,
        "relation_unresolved": 1,
    }


def test_post_process_preserves_op_with_ambiguous_relation_target(
    tmp_path, monkeypatch
):
    from solstone.think.entities import (
        ResolutionScope,
        load_ambiguities,
        load_entities,
        record_ambiguity_choice,
    )

    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    _attach_entity(tmp_path, facet, "sarah_connor", "Sarah Connor")
    _attach_entity(tmp_path, facet, "sarah_lee", "Sarah Lee")

    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            {
                                "op": "add",
                                "content": "Works with Sarah on a confidential project",
                                "reasoning": "Relational, but Sarah is ambiguous.",
                                "relation": {
                                    "kind": "works-with",
                                    "target_name": "Sarah",
                                    "note": "",
                                },
                            }
                        ],
                    }
                ],
                "summary": "ambiguous relation target",
            }
        ),
        {"facet": facet, "day": day},
    )

    observations = load_observations(facet, "alice_johnson")
    assert len(observations) == 1
    assert observations[0]["relation"] == {
        "kind": "works-with",
        "target_entity_id": None,
        "target_name": "Sarah",
        "note": "",
    }
    outcome = _load_outcome(tmp_path, facet, day)
    assert outcome["relation_unresolved"] == 1
    rows = load_ambiguities()
    assert len(rows) == 1
    assert rows[0]["normalized_query"] == "sarah"

    record_ambiguity_choice(
        "Sarah",
        "sarah_connor",
        load_entities(facet),
        scope=ResolutionScope.facet_scope(facet),
    )
    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            {
                                "op": "add",
                                "content": "Coordinates with Sarah after review",
                                "reasoning": "Choice resolved.",
                                "relation": {
                                    "kind": "works-with",
                                    "target_name": "Sarah",
                                    "note": "",
                                },
                            }
                        ],
                    }
                ],
                "summary": "resolved relation target",
            }
        ),
        {"facet": facet, "day": day},
    )

    observations = load_observations(facet, "alice_johnson")
    assert observations[-1]["relation"]["target_entity_id"] == "sarah_connor"


def test_post_process_drops_op_with_other_relation_kind_and_no_note(
    tmp_path, monkeypatch
):
    _set_journal(monkeypatch, str(tmp_path))
    facet = "work"
    day = "20260304"
    _attach_entity(tmp_path, facet, "alice_johnson", "Alice Johnson")
    _attach_entity(tmp_path, facet, "bob_lee", "Bob Lee")

    post_process(
        json.dumps(
            {
                "entities": [
                    {
                        "entity_id": "alice_johnson",
                        "operations": [
                            {
                                "op": "add",
                                "content": "Has an unusual tie to Bob Lee",
                                "reasoning": "Relational, but the note is blank.",
                                "relation": {
                                    "kind": "other",
                                    "target_name": "Bob Lee",
                                    "note": " ",
                                },
                            }
                        ],
                    }
                ],
                "summary": "other kind without a note",
            }
        ),
        {"facet": facet, "day": day},
    )

    assert load_observations(facet, "alice_johnson") == []
    outcome = _load_outcome(tmp_path, facet, day)
    assert {key: outcome[key] for key in COUNT_KEYS} == {
        "update": 0,
        "add": 0,
        "drop": 0,
        "keep": 0,
        "skipped": 1,
        "relation_unresolved": 0,
    }


# ============================================================================
# Agent config test
# ============================================================================


def test_entity_observer_agent_config(monkeypatch):
    _set_journal(monkeypatch, "tests/fixtures/journal")

    config = get_talent("entities:entity_observer")

    assert config["type"] == "generate"
    assert config.get("output") == "json"
    assert "tier" not in config
    assert config.get("thinking_budget") == 2048
    assert config.get("hook", {}).get("pre") == "entities:entity_observer"
    assert config.get("hook", {}).get("post") == "entities:entity_observer"
    assert "$observer_context" in config["user_instruction"]
