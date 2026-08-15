# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json


def _write_detected_entities(tmp_path, facet: str, day: str, rows: list[dict]) -> None:
    entities_path = tmp_path / "facets" / facet / "entities" / f"{day}.jsonl"
    entities_path.parent.mkdir(parents=True, exist_ok=True)
    entities_path.write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows),
        encoding="utf-8",
    )


def test_participation_post_hook_resolves_entity_ids_without_mutating_entities(
    tmp_path, monkeypatch
):
    from solstone.talent.participation import post_process
    from solstone.think.activities import append_activity_record, load_activity_records

    facet = "work"
    day = "20260418"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    _write_detected_entities(
        tmp_path,
        facet,
        day,
        [
            {
                "id": "avery_nguyen",
                "type": "Person",
                "name": "Avery Nguyen",
                "aka": ["AN"],
            },
            {
                "id": "other_person",
                "type": "Person",
                "name": "Other Person",
            },
        ],
    )

    entities_dir = tmp_path / "facets" / facet / "entities"
    snapshot_before = {p.name: p.stat().st_size for p in entities_dir.iterdir()}

    append_activity_record(
        facet,
        day,
        {
            "id": "meeting_090000_300",
            "activity": "meeting",
            "segments": ["090000_300"],
            "level_avg": 1.0,
            "description": "Team sync",
            "active_entities": ["AN", "Alex"],
            "created_at": 1,
        },
    )

    result = json.dumps(
        {
            "participation": [
                {
                    "name": "AN",
                    "role": "attendee",
                    "source": "voice",
                    "confidence": 0.98,
                    "context": "Spoke during the meeting",
                    "entity_id": "fake_id",
                },
                {
                    "name": "Alex",
                    "role": "mentioned",
                    "source": "transcript",
                    "confidence": 0.55,
                    "context": "Mentioned as a follow-up owner",
                    "entity_id": "fake_id",
                },
            ]
        }
    )

    post_process(
        result,
        {"facet": facet, "day": day, "activity": {"id": "meeting_090000_300"}},
    )

    snapshot_after = {p.name: p.stat().st_size for p in entities_dir.iterdir()}
    assert snapshot_after == snapshot_before

    record = load_activity_records(facet, day)[0]
    assert record["participation"][0]["entity_id"] == "avery_nguyen"
    assert record["participation"][1]["entity_id"] is None
    assert record["title"] == "Team sync"
    assert record["details"] == ""
    assert record["hidden"] is False
