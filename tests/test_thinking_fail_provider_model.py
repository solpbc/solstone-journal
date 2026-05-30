# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Provider/model attribution on talent.fail health records."""

from __future__ import annotations

import json
from pathlib import Path


def _capture_jsonl(monkeypatch, mod):
    records: list[dict] = []

    def log(event: str, **fields) -> None:
        records.append({"event": event, **fields})

    monkeypatch.setattr(mod, "_jsonl_log", log)
    monkeypatch.setattr(mod, "emit", lambda *args, **kwargs: None)
    monkeypatch.setattr(mod, "_update_status", lambda **kwargs: None)
    monkeypatch.setattr(mod, "day_log", lambda *args, **kwargs: None)
    return records


def _provider_model(use_id: str) -> tuple[str | None, str | None]:
    if use_id.endswith("timeout"):
        return ("google", "gemini-2.5-pro")
    if use_id.endswith("error"):
        return (None, None)
    return ("openai", "gpt-5")


def _write_activity_record(
    journal: Path,
    day: str,
    facet: str,
    activity_id: str,
) -> None:
    path = journal / "facets" / facet / "activities" / f"{day}.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(
            {
                "id": activity_id,
                "activity": "coding",
                "segments": ["100000_300"],
                "description": "Coding",
                "active_entities": [],
            }
        )
        + "\n",
        encoding="utf-8",
    )


def test_priority_fail_records_provider_model_and_null_when_missing(monkeypatch):
    from solstone.think import thinking

    records = _capture_jsonl(monkeypatch, thinking)
    monkeypatch.setattr(thinking, "read_use_provider_model", _provider_model)
    monkeypatch.setattr(
        thinking,
        "wait_for_uses",
        lambda ids, timeout: ({}, ["use-timeout"]),
    )

    thinking._drain_priority_batch(
        [("use-timeout", "entities", {"type": "cogitate"}, None)],
        "segment",
        "20240101",
        "100000_300",
        stream="default",
    )

    monkeypatch.setattr(
        thinking,
        "wait_for_uses",
        lambda ids, timeout: ({"use-error": "error"}, []),
    )
    thinking._drain_priority_batch(
        [("use-error", "documents", {"type": "cogitate"}, None)],
        "segment",
        "20240101",
        "100000_300",
        stream="default",
    )

    failures = [record for record in records if record["event"] == "talent.fail"]
    assert failures[0]["provider"] == "google"
    assert failures[0]["model"] == "gemini-2.5-pro"
    assert failures[1]["provider"] is None
    assert failures[1]["model"] is None


def test_activity_fail_records_provider_model_on_timeout_and_terminal(
    tmp_path,
    monkeypatch,
):
    from solstone.think import thinking

    journal = tmp_path / "journal"
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    _write_activity_record(journal, "20240101", "work", "coding_100000_300")
    records = _capture_jsonl(monkeypatch, thinking)
    monkeypatch.setattr(thinking, "read_use_provider_model", _provider_model)
    monkeypatch.setattr(
        thinking,
        "get_talent_configs",
        lambda schedule: {
            "timeout": {
                "type": "cogitate",
                "priority": 10,
                "activities": ["coding"],
            },
            "error": {
                "type": "cogitate",
                "priority": 10,
                "activities": ["coding"],
            },
        },
    )
    monkeypatch.setattr(
        thinking,
        "_cortex_request_with_retry",
        lambda **kwargs: f"use-{kwargs['name']}",
    )
    monkeypatch.setattr(
        thinking,
        "wait_for_uses",
        lambda ids, timeout: ({"use-error": "error"}, ["use-timeout"]),
    )

    assert (
        thinking.run_activity_prompts(
            day="20240101",
            activity_id="coding_100000_300",
            facet="work",
            max_concurrency=0,
        )
        is False
    )

    failures = [record for record in records if record["event"] == "talent.fail"]
    assert {
        (record["name"], record["provider"], record["model"]) for record in failures
    } == {
        ("timeout", "google", "gemini-2.5-pro"),
        ("error", None, None),
    }


def test_flush_fail_records_provider_model_on_timeout_and_terminal(monkeypatch):
    from solstone.think import thinking

    records = _capture_jsonl(monkeypatch, thinking)
    monkeypatch.setattr(thinking, "read_use_provider_model", _provider_model)
    monkeypatch.setattr(
        thinking,
        "get_talent_configs",
        lambda schedule: {
            "timeout": {
                "type": "cogitate",
                "priority": 10,
                "hook": {"flush": True},
            },
            "error": {
                "type": "cogitate",
                "priority": 10,
                "hook": {"flush": True},
            },
        },
    )
    monkeypatch.setattr(
        thinking,
        "_cortex_request_with_retry",
        lambda **kwargs: f"use-{kwargs['name']}",
    )
    monkeypatch.setattr(
        thinking,
        "wait_for_uses",
        lambda ids, timeout: ({"use-error": "error"}, ["use-timeout"]),
    )

    assert thinking.run_flush_prompts("20240101", "100000_300", False) is False

    failures = [record for record in records if record["event"] == "talent.fail"]
    assert {
        (record["name"], record["provider"], record["model"]) for record in failures
    } == {
        ("timeout", "google", "gemini-2.5-pro"),
        ("error", None, None),
    }
