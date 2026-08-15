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


def _activity_record():
    return {
        "id": "meeting_090000_300",
        "activity": "meeting",
        "segments": ["090000_300"],
        "level_avg": 1.0,
        "description": "Team sync",
        "active_entities": ["AN", "Alex"],
        "created_at": 1,
    }


def test_participation_post_hook_merges_fields_and_preserves_active_entities(
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
            }
        ],
    )
    append_activity_record(facet, day, _activity_record())

    post_process(
        json.dumps(
            {
                "participation": [
                    {
                        "name": "AN",
                        "role": "attendee",
                        "source": "voice",
                        "confidence": 0.91,
                        "context": "Spoke during the meeting",
                        "entity_id": None,
                    },
                    {
                        "name": "Alex",
                        "role": "mentioned",
                        "source": "transcript",
                        "confidence": 0.42,
                        "context": "Mentioned as a collaborator",
                        "entity_id": None,
                    },
                ],
                "participation_confidence": 0.77,
            }
        ),
        {"facet": facet, "day": day, "activity": {"id": "meeting_090000_300"}},
    )

    record = load_activity_records(facet, day)[0]
    assert record["active_entities"] == ["AN", "Alex"]
    assert record["participation_confidence"] == 0.77
    assert record["participation"][0]["entity_id"] == "avery_nguyen"
    assert record["participation"][1]["entity_id"] is None
    assert record["title"] == "Team sync"
    assert record["details"] == ""
    assert record["hidden"] is False


def test_participation_post_hook_leaves_file_unchanged_on_malformed_json(
    tmp_path, monkeypatch, caplog
):
    from solstone.talent.participation import post_process
    from solstone.think.activities import append_activity_record

    facet = "work"
    day = "20260418"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    append_activity_record(facet, day, _activity_record())
    record_path = tmp_path / "facets" / facet / "activities" / f"{day}.jsonl"
    before = record_path.read_bytes()

    post_process(
        "{not valid json",
        {"facet": facet, "day": day, "activity": {"id": "meeting_090000_300"}},
    )

    assert record_path.read_bytes() == before
    assert "failed to parse JSON" in caplog.text


def test_participation_post_hook_requires_activity_context(
    tmp_path, monkeypatch, caplog
):
    from solstone.talent.participation import post_process
    from solstone.think.activities import append_activity_record

    facet = "work"
    day = "20260418"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    append_activity_record(facet, day, _activity_record())
    record_path = tmp_path / "facets" / facet / "activities" / f"{day}.jsonl"
    before = record_path.read_bytes()

    post_process(
        json.dumps({"participation": []}),
        {"facet": facet, "day": day},
    )

    assert record_path.read_bytes() == before
    assert "missing activity context" in caplog.text
