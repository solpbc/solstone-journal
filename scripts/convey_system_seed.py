#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Seed deterministic journal states for the convey-system corpus."""

from __future__ import annotations

import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

PAST_DAYS = ("20260214", "20260315", "20260403")
POPULATED_DAY = "20260403"
PHASES = (
    "corrupt",
    "established_empty",
    "established_populated",
    "populated_single_failure",
    "stats_absent",
    "stats_unparseable",
)


def _write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _write_jsonl(path: Path, entries: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(entry, sort_keys=True) + "\n" for entry in entries),
        encoding="utf-8",
    )


def _capture_timestamp(capture_day: str) -> int:
    parsed = datetime.strptime(capture_day, "%Y%m%d").replace(tzinfo=timezone.utc)
    return int(parsed.timestamp() * 1000) + 12_000


def _established_config(*, disabled_chat: bool = False) -> dict[str, Any]:
    config: dict[str, Any] = {
        "setup": {"completed_at": 1_700_000_000_000},
        "agent": {"name": "Corpus Agent", "name_status": "named"},
        "identity": {"name": "Corpus Owner", "timezone": "UTC"},
    }
    if disabled_chat:
        config["talent_overrides"] = {"chat": {"disabled": True}}
    return config


def _write_config(root: Path, config: dict[str, Any]) -> None:
    _write_json(root / "config" / "journal.json", config)


def _write_stats(root: Path) -> None:
    stats_path = root / "stats.json"
    _write_json(
        stats_path,
        {
            "backlog": {
                "generated_at": "2026-04-03T12:00:00Z",
                "items": [],
                "status": "clear",
            },
            "summary": {"completed": 2, "pending": 1},
        },
    )
    os.utime(stats_path, (1_700_000_000, 1_700_000_000))


def _write_markers(root: Path) -> None:
    markers = (
        ("20260214", None, 1_700_000_100),
        ("20260315", 1_700_000_200, 1_700_000_300),
        ("20260403", 1_700_000_500, 1_700_000_400),
    )
    for day, daily_mtime, stream_mtime in markers:
        health = root / "chronicle" / day / "health"
        health.mkdir(parents=True, exist_ok=True)
        stream = health / "stream.updated"
        stream.touch()
        os.utime(stream, (stream_mtime, stream_mtime))
        if daily_mtime is not None:
            daily = health / "daily.updated"
            daily.touch()
            os.utime(daily, (daily_mtime, daily_mtime))


def _write_tokens(root: Path) -> None:
    _write_jsonl(
        root / "tokens" / f"{POPULATED_DAY}.jsonl",
        [
            {
                "context": "talent.system.daily_digest",
                "model": "gpt-5.5",
                "segment": "090000_60",
                "timestamp": "2026-04-03T09:00:00Z",
                "type": "generate",
                "usage": {
                    "cached_tokens": 300,
                    "input_tokens": 1_000,
                    "output_tokens": 200,
                    "reasoning_tokens": 0,
                    "total_tokens": 1_200,
                },
            },
            {
                "context": "talent.system.review",
                "model": "claude-sonnet-4-6",
                "segment": "100000_120",
                "timestamp": "2026-04-03T10:00:00Z",
                "type": "cogitate",
                "usage": {
                    "cached_tokens": 0,
                    "input_tokens": 800,
                    "output_tokens": 400,
                    "reasoning_tokens": 100,
                    "total_tokens": 1_200,
                },
            },
        ],
    )


def _request_event(use_id: str, *, name: str, day: str, ts: int, facet: str) -> dict[str, Any]:
    return {
        "day": day,
        "event": "request",
        "facet": facet,
        "name": name,
        "prompt": "Corpus prompt",
        "provider": "openai",
        "ts": ts,
        "use_id": use_id,
    }


def _write_runs(root: Path, capture_day: str, failures: int) -> None:
    talents = root / "talents"
    completed_id = "1710000000000"
    pending_id = "1710000060000"
    malformed_id = "1710000120000"
    _write_jsonl(
        talents / "daily" / f"{completed_id}.jsonl",
        [
            _request_event(
                completed_id,
                name="daily_digest",
                day=POPULATED_DAY,
                ts=1_710_000_000_000,
                facet="work",
            ),
            {"event": "start", "model": "gpt-5.5", "provider": "openai", "ts": 1_710_000_000_100},
            {
                "event": "finish",
                "ts": 1_710_000_001_000,
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
            },
        ],
    )
    _write_jsonl(
        talents / "review" / f"{pending_id}_active.jsonl",
        [
            _request_event(
                pending_id,
                name="review",
                day=POPULATED_DAY,
                ts=1_710_000_060_000,
                facet="work",
            )
        ],
    )
    malformed = talents / "summary" / f"{malformed_id}.jsonl"
    malformed.parent.mkdir(parents=True, exist_ok=True)
    malformed.write_text("", encoding="utf-8")

    capture_ts = _capture_timestamp(capture_day)
    populated_day_index: list[dict[str, Any]] = [
        {
            "facet": "work",
            "name": "daily_digest",
            "provider": "openai",
            "status": "completed",
            "ts": 1_710_000_000_000,
            "use_id": completed_id,
        }
    ]
    capture_day_index: list[dict[str, Any]] = []
    for index in range(failures):
        capture_day_index.append(
            {
                "name": f"failed_talent_{index + 1}",
                "provider": "openai",
                "reason_code": "provider_error",
                "status": "error",
                "ts": capture_ts + index,
                "use_id": f"capture-failure-{index + 1}",
            }
        )
    _write_jsonl(talents / f"{POPULATED_DAY}.jsonl", populated_day_index)
    _write_jsonl(talents / f"{capture_day}.jsonl", capture_day_index)


def _write_health_log(root: Path) -> None:
    log_path = root / POPULATED_DAY / "health" / "health.log"
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.write_text("2026-04-03 corpus health log\n", encoding="utf-8")


def _write_output(root: Path) -> None:
    output = root / "chronicle" / POPULATED_DAY / "talents" / "example-output.md"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("# Corpus output\n\nDeterministic fixture content.\n", encoding="utf-8")


def _seed_populated(root: Path, capture_day: str, *, failures: int) -> None:
    _write_config(root, _established_config(disabled_chat=failures == 1))
    _write_stats(root)
    _write_markers(root)
    _write_tokens(root)
    _write_runs(root, capture_day, failures)
    _write_health_log(root)
    _write_output(root)


def seed_phase(root: Path, phase: str, capture_day: str) -> None:
    """Write the exact journal state for one corpus phase."""
    if phase not in PHASES:
        raise ValueError(f"unknown convey-system phase: {phase}")
    root.mkdir(parents=True, exist_ok=True)
    if phase == "corrupt":
        config = root / "config" / "journal.json"
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_text("{not json\n", encoding="utf-8")
    elif phase == "established_empty":
        _write_config(root, _established_config())
    elif phase == "established_populated":
        _seed_populated(root, capture_day, failures=3)
    elif phase == "populated_single_failure":
        _seed_populated(root, capture_day, failures=1)
    elif phase == "stats_absent":
        _write_config(root, _established_config())
        _write_markers(root)
        _write_tokens(root)
        _write_runs(root, capture_day, failures=0)
        _write_health_log(root)
        _write_output(root)
    elif phase == "stats_unparseable":
        _write_config(root, _established_config())
        _write_markers(root)
        _write_tokens(root)
        _write_runs(root, capture_day, failures=0)
        _write_health_log(root)
        _write_output(root)
        (root / "stats.json").write_bytes(b"not valid json\n")
        (root / "talents" / "20990101.jsonl").mkdir(parents=True, exist_ok=True)
