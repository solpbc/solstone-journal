# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import json
from pathlib import Path

CONTRACT_FIELDS = [
    ("days", "stats.days"),
    ("totals", "stats.totals"),
    ("heatmap", "stats.heatmap"),
    ("totals.day_bytes", "totals.day_bytes"),
    ("totals.pending_segments", "totals.pending_segments"),
    ("totals.backlog_pending_days", "totals.backlog_pending_days"),
    ("totals.backlog_stuck_days", "totals.backlog_stuck_days"),
    ("days.*.day_bytes", "dayData.day_bytes"),
    ("totals.total_transcript_duration", "total_transcript_duration"),
    ("totals.total_percept_duration", "total_percept_duration"),
    ("totals.transcript_sessions", "transcript_sessions"),
    ("tokens.by_model", "tokens.by_model"),
    ("tokens.by_day", "tokens.by_day"),
    ("facets.counts_by_day", "facets.counts_by_day"),
    ("talents.counts_by_day", "talents.counts_by_day"),
    ("days.*.transcript_duration", "transcript_duration"),
    ("days.*.percept_duration", "percept_duration"),
    ("tokens.by_day.*.*.input_tokens", "input_tokens"),
    ("tokens.by_day.*.*.output_tokens", "output_tokens"),
    ("tokens.by_day.*.*.reasoning_tokens", "reasoning_tokens"),
    ("tokens.by_model.*.total_tokens", "total_tokens"),
]

JS_PATH = (
    Path(__file__).resolve().parent.parent
    / "solstone"
    / "apps"
    / "stats"
    / "static"
    / "dashboard.js"
)


def _resolve_path(data, path):
    stack = [(data, path.split("."))]
    while stack:
        current, parts = stack.pop()
        if not parts:
            return True
        if not isinstance(current, dict):
            continue
        part, rest = parts[0], parts[1:]
        if part == "*":
            stack.extend((value, rest) for value in current.values())
        elif part in current:
            stack.append((current[part], rest))
    return False


def _build_journal(base_path):
    journal = base_path
    day = journal / "chronicle" / "20240101"
    seg1 = day / "default" / "123456_300"
    seg2 = day / "default" / "134500_300"
    seg1.mkdir(parents=True)
    seg2.mkdir(parents=True)
    (day / "talents").mkdir(parents=True)

    audio_lines = [
        {"raw": "raw.flac"},
        {"start": "10:00:00", "text": "hello"},
        {"start": "10:05:00", "text": "world"},
    ]
    (seg1 / "audio.jsonl").write_text(
        "\n".join(json.dumps(line) for line in audio_lines) + "\n"
    )

    screen_lines = [
        {"raw": "screen.webm"},
        {"frame_id": 1, "timestamp": 1000.0, "text": "frame one"},
        {"frame_id": 2, "timestamp": 1060.0, "text": "frame two"},
    ]
    (seg1 / "screen.jsonl").write_text(
        "\n".join(json.dumps(line) for line in screen_lines) + "\n"
    )

    (seg2 / "audio.flac").write_bytes(b"fLaC")
    (day / "talents" / "schedule.json").write_text("[]")

    facet_dir = journal / "facets" / "work"
    facet_dir.mkdir(parents=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Work"}))
    activities_dir = facet_dir / "activities"
    activities_dir.mkdir(parents=True)
    activity = {
        "id": "meeting_090000_300",
        "activity": "meeting",
        "segments": ["090000_300"],
        "description": "daily sync",
    }
    (activities_dir / "20240101.jsonl").write_text(json.dumps(activity) + "\n")

    tokens_dir = journal / "tokens"
    tokens_dir.mkdir()
    token = {
        "timestamp": 1704067200.0,
        "timestamp_str": "20240101_120000",
        "model": "test-model",
        "context": "test",
        "usage": {
            "input_tokens": 100,
            "output_tokens": 50,
            "cached_tokens": 10,
            "reasoning_tokens": 5,
            "total_tokens": 165,
        },
    }
    (tokens_dir / "20240101.jsonl").write_text(json.dumps(token) + "\n")

    return journal


def _scan_output(journal, stats_mod):
    js = stats_mod.JournalStats()
    js.scan(str(journal))
    return js.to_dict()


def test_generated_stats_pass_schema(tmp_path, monkeypatch):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    schema_mod = importlib.import_module("solstone.think.stats_schema")
    journal = _build_journal(tmp_path)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    output = _scan_output(journal, stats_mod)

    errors = schema_mod.validate(output)
    assert errors == [], f"Validation errors: {errors}"
    assert output["schema_version"] == schema_mod.SCHEMA_VERSION


def test_contract_fields_exist_in_output(tmp_path, monkeypatch):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = _build_journal(tmp_path)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    output = _scan_output(journal, stats_mod)

    for python_path, _ in CONTRACT_FIELDS:
        assert _resolve_path(output, python_path), (
            f"{python_path} missing from stats output"
        )


def test_contract_fields_referenced_in_js():
    js_source = JS_PATH.read_text()

    for _, js_ref in CONTRACT_FIELDS:
        assert js_ref in js_source, (
            f"{js_ref} not found in dashboard.js — contract field may be stale"
        )


def test_segments_awaiting_thinking_repair_card_referenced():
    js_source = JS_PATH.read_text()

    assert "segments_pending_think" in js_source
    assert "segments awaiting thinking" in js_source


def test_all_day_fields_have_nonzero_values(tmp_path, monkeypatch):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    schema_mod = importlib.import_module("solstone.think.stats_schema")
    journal = _build_journal(tmp_path)
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    output = _scan_output(journal, stats_mod)
    day_entry = next(iter(output["days"].values()))

    for field in schema_mod.DAY_FIELDS:
        assert day_entry[field] > 0, f"{field} should be non-zero in fixture output"


def test_schema_rejects_missing_required_key():
    schema_mod = importlib.import_module("solstone.think.stats_schema")
    output = {
        "schema_version": schema_mod.SCHEMA_VERSION,
        "generated_at": "2026-04-10T00:00:00+00:00",
        "day_count": 1,
        "days": {},
        "totals": {},
        "heatmap": [],
        "tokens": {},
        "talents": {},
        "facets": {},
    }
    del output["totals"]

    errors = schema_mod.validate(output)

    assert errors, "validate() should reject missing required keys"
    assert any("totals" in error for error in errors)


def test_schema_rejects_wrong_version():
    schema_mod = importlib.import_module("solstone.think.stats_schema")

    errors = schema_mod.validate(
        {
            "schema_version": 99,
            "generated_at": "2026-04-10T00:00:00+00:00",
            "day_count": 0,
            "days": {},
            "totals": {},
            "heatmap": [],
            "tokens": {},
            "talents": {},
            "facets": {},
        }
    )

    assert errors, "validate() should reject the wrong schema version"
    assert any("schema_version" in error for error in errors)
