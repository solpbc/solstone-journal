# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import json
import logging
import os

import pytest


def _write_jsonl(path, events):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for event in events:
            handle.write(json.dumps(event) + "\n")


def _segment_event(
    event,
    segment,
    name=None,
    ts=1,
    **extra,
):
    record = {"event": event, "ts": ts, "mode": "segment", "segment": segment}
    if name is not None:
        record["name"] = name
    record.update(extra)
    return record


def _dispatch(segment, name, ts=1):
    return _segment_event("talent.dispatch", segment, name, ts)


def _complete(segment, name, ts=1):
    return _segment_event("talent.complete", segment, name, ts, state="finish")


def _sense_complete(segment, density="active", ts=1):
    return _segment_event("sense.complete", segment, ts=ts, density=density)


def _complete_segment_events(segment):
    return [
        _dispatch(segment, "sense", 10),
        _complete(segment, "sense", 11),
        _sense_complete(segment, "active", 12),
        _dispatch(segment, "entities", 13),
        _complete(segment, "entities", 14),
        _dispatch(segment, "documents", 15),
        _complete(segment, "documents", 16),
    ]


def _seed_screen_segment(journal, day, segment="123456_300"):
    segment_dir = journal / "chronicle" / day / "default" / segment
    segment_dir.mkdir(parents=True, exist_ok=True)
    (segment_dir / "screen.webm").write_bytes(b"raw")
    (segment_dir / "screen.jsonl").write_text(
        json.dumps({"raw": "screen.webm", "type": "screencast"})
        + "\n"
        + json.dumps({"timestamp": 0, "content": {}})
        + "\n",
        encoding="utf-8",
    )
    return segment_dir


def test_scan_day(tmp_path, monkeypatch):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = tmp_path
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)

    # Create an audio jsonl file in segment directory (already processed)
    ts_dir = day / "default" / "123456_300"
    ts_dir.mkdir(parents=True)
    (ts_dir / "audio.jsonl").write_text(
        '{"raw": "raw.flac"}\n'
        '{"start": "10:00:00", "text": "hello"}\n'
        '{"start": "10:01:00", "text": "world"}\n'
    )

    # Create unprocessed media files in a second segment directory (no jsonl output yet)
    ts_dir2 = day / "default" / "134500_300"
    ts_dir2.mkdir(parents=True)
    (ts_dir2 / "audio.flac").write_bytes(b"RIFF")
    (ts_dir2 / "center_DP-1_screen.webm").write_bytes(b"WEBM")

    (day / "entities.md").write_text("")
    (day / "talents").mkdir()
    (day / "talents" / "flow.md").write_text("")

    facet_dir = journal / "facets" / "work"
    facet_dir.mkdir(parents=True)
    (facet_dir / "facet.json").write_text(json.dumps({"title": "Work"}))
    activities_dir = facet_dir / "activities"
    activities_dir.mkdir(parents=True)
    activity = {
        "id": "meeting_000000_300",
        "activity": "meeting",
        "segments": ["000000_300"],
        "description": "Project sync",
    }
    (activities_dir / "20240101.jsonl").write_text(json.dumps(activity) + "\n")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    js = stats_mod.JournalStats()
    day_data = js.scan_day("20240101", str(day))
    js._apply_day_stats("20240101", day_data)
    assert js.days["20240101"]["transcript_sessions"] == 1
    assert js.days["20240101"]["transcript_segments"] == 2
    assert (
        js.days["20240101"]["pending_segments"] == 1
    )  # Both files belong to same segment
    assert js.agent_counts["meeting"] == 1
    assert js.facet_counts["work"] == 1
    assert js.facet_minutes["work"] == 5.0
    assert js.heatmap[0][0] == 5
    assert js.days["20240101"]["day_bytes"] > 0


def test_segments_pending_think_fold_failure_logs_and_defaults_zero(
    tmp_path,
    monkeypatch,
    caplog,
):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = tmp_path
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)

    def fail_classify(*_args, **_kwargs):
        raise RuntimeError("fold exploded")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(stats_mod, "classify_segment_completion", fail_classify)
    caplog.set_level(logging.WARNING)

    js = stats_mod.JournalStats()
    day_data = js.scan_day("20240101", str(day))

    assert day_data["stats"]["segments_pending_think"] == 0
    assert "segments_pending_think under-reported" in caplog.text


def test_cache_invalidates_on_health_event(tmp_path, monkeypatch):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = tmp_path
    day_name = "20240101"
    day = journal / "chronicle" / day_name
    segment = "123456_300"
    _seed_screen_segment(journal, day_name, segment)
    _write_jsonl(
        day / "health" / "001_segment.jsonl",
        [_sense_complete(segment, "active", 1)],
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    js1 = stats_mod.JournalStats()
    js1.scan(str(journal), verbose=False, use_cache=True)
    assert js1.days[day_name]["segments_pending_think"] == 1

    cache_file = day / "stats.json"
    new_health = day / "health" / "002_segment.jsonl"
    _write_jsonl(new_health, _complete_segment_events(segment))
    # Force a strict mtime increase so cache invalidation is stable on coarse FS.
    newer = cache_file.stat().st_mtime + 2
    os.utime(new_health, (newer, newer))

    js2 = stats_mod.JournalStats()
    js2.scan(str(journal), verbose=False, use_cache=True)

    assert js2.days[day_name]["segments_pending_think"] == 0


def test_token_usage(tmp_path, monkeypatch):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    schema_mod = importlib.import_module("solstone.think.stats_schema")
    journal = tmp_path
    day1 = journal / "chronicle" / "20240101"
    day1.mkdir(parents=True)
    day2 = journal / "chronicle" / "20240102"
    day2.mkdir(parents=True)

    # Create tokens directory with test token files
    tokens_dir = journal / "tokens"
    tokens_dir.mkdir()

    # Create token files for different models on the same day (using new normalized format)
    token1 = {
        "timestamp": 1704067200.0,
        "timestamp_str": "20240101_120000",
        "model": "gemini-2.5-flash",
        "context": "test_context",
        "usage": {
            "input_tokens": 100,
            "output_tokens": 50,
            "cached_tokens": 10,
            "reasoning_tokens": 5,
            "total_tokens": 165,
        },
    }

    token2 = {
        "timestamp": 1704070800.0,
        "timestamp_str": "20240101_130000",
        "model": "gemini-2.5-flash",
        "context": "test_context2",
        "usage": {
            "input_tokens": 200,
            "output_tokens": 100,
            "cached_tokens": 20,
            "reasoning_tokens": 10,
            "total_tokens": 330,
        },
    }

    token3 = {
        "timestamp": 1704074400.0,
        "timestamp_str": "20240101_140000",
        "model": "claude-3-opus",
        "context": "test_context3",
        "usage": {"input_tokens": 500, "output_tokens": 250, "total_tokens": 750},
    }

    # Token from different day (new normalized format)
    token4 = {
        "timestamp": 1704153600.0,
        "timestamp_str": "20240102_120000",
        "model": "gemini-2.5-flash",
        "context": "test_context4",
        "usage": {
            "input_tokens": 1000,
            "output_tokens": 500,
            "total_tokens": 1500,
        },
    }

    # Write tokens as JSONL format (one per line in daily file)
    (tokens_dir / "20240101.jsonl").write_text(
        json.dumps(token1)
        + "\n"
        + json.dumps(token2)
        + "\n"
        + json.dumps(token3)
        + "\n"
    )
    (tokens_dir / "20240102.jsonl").write_text(json.dumps(token4) + "\n")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    js = stats_mod.JournalStats()
    js.scan(str(journal))

    # Check token usage for the day
    assert "20240101" in js.token_usage
    assert "gemini-2.5-flash" in js.token_usage["20240101"]
    assert "claude-3-opus" in js.token_usage["20240101"]

    # Check gemini totals for the day (sum of token1 and token2)
    gemini_usage = js.token_usage["20240101"]["gemini-2.5-flash"]
    assert gemini_usage["input_tokens"] == 300  # 100 + 200
    assert gemini_usage["output_tokens"] == 150  # 50 + 100
    assert gemini_usage["cached_tokens"] == 30  # 10 + 20
    assert gemini_usage["reasoning_tokens"] == 15  # 5 + 10
    assert gemini_usage["total_tokens"] == 495  # 165 + 330

    # Check claude totals for the day
    claude_usage = js.token_usage["20240101"]["claude-3-opus"]
    assert claude_usage["input_tokens"] == 500
    assert claude_usage["output_tokens"] == 250
    assert claude_usage["total_tokens"] == 750

    # Check overall model totals
    assert (
        js.token_totals["gemini-2.5-flash"]["input_tokens"] == 1300
    )  # 300 from day1 + 1000 from day2
    assert js.token_totals["claude-3-opus"]["input_tokens"] == 500

    # Test JSON output includes token usage
    data = js.to_dict()
    assert data["schema_version"] == schema_mod.SCHEMA_VERSION
    assert "generated_at" in data
    assert data["day_count"] == 2
    assert "backlog" in data
    assert "backlog_pending_days" in data["totals"]
    assert "backlog_stuck_days" in data["totals"]
    assert "tokens" in data
    assert "by_day" in data["tokens"]
    assert "total_transcript_duration" in data["totals"]
    assert "total_percept_duration" in data["totals"]
    assert (
        data["tokens"]["by_day"]["20240101"]["gemini-2.5-flash"]["total_tokens"] == 495
    )


def test_caching(tmp_path, monkeypatch):
    """Test that per-day caching works correctly."""
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = tmp_path
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)

    # Create an audio jsonl file in segment directory
    ts_dir = day / "default" / "123456_300"
    ts_dir.mkdir(parents=True)
    (ts_dir / "audio.jsonl").write_text(
        '{"raw": "raw.flac"}\n'
        '{"start": "10:00:00", "text": "hello"}\n'
        '{"start": "10:01:00", "text": "world"}\n'
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    # First scan - should create cache
    js1 = stats_mod.JournalStats()
    js1.scan(str(journal), verbose=False, use_cache=True)
    assert js1.days["20240101"]["transcript_sessions"] == 1
    assert (day / "stats.json").exists()

    # Load cache and verify contents
    with open(day / "stats.json") as f:
        cached = json.load(f)
    assert cached["schema_version"] == stats_mod.SCHEMA_VERSION
    assert cached["stats"]["transcript_sessions"] == 1
    assert cached["stats"]["transcript_segments"] == 2

    # Second scan - should use cache
    js2 = stats_mod.JournalStats()
    js2.scan(str(journal), verbose=False, use_cache=True)
    assert js2.days["20240101"]["transcript_sessions"] == 1
    assert js2.days["20240101"]["transcript_segments"] == 2

    # Third scan with --no-cache - should re-scan
    js3 = stats_mod.JournalStats()
    js3.scan(str(journal), verbose=False, use_cache=False)
    assert js3.days["20240101"]["transcript_sessions"] == 1


def test_old_day_cache_without_schema_version_recomputes_and_overwrites(
    tmp_path,
    monkeypatch,
):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = tmp_path
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)
    segment = day / "default" / "123456_300"
    segment.mkdir(parents=True)
    (segment / "audio.jsonl").write_text(
        '{"raw": "raw.flac"}\n{"start": "10:00:00", "text": "hello"}\n'
    )
    cache_file = day / "stats.json"
    cache_file.write_text(
        json.dumps({"stats": {"transcript_sessions": 99}}),
        encoding="utf-8",
    )
    newer = max(path.stat().st_mtime for path in day.rglob("*")) + 2
    os.utime(cache_file, (newer, newer))

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    js = stats_mod.JournalStats()
    js.scan(str(journal), verbose=False, use_cache=True)

    assert js.days["20240101"]["transcript_sessions"] == 1
    payload = json.loads(cache_file.read_text(encoding="utf-8"))
    assert payload["schema_version"] == stats_mod.SCHEMA_VERSION


def test_current_schema_full_day_cache_is_reused(tmp_path, monkeypatch):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    schema_mod = importlib.import_module("solstone.think.stats_schema")
    journal = tmp_path
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)
    stats = {field: 0 for field in schema_mod.DAY_FIELDS}
    stats["transcript_sessions"] = 7
    cache_file = day / "stats.json"
    cache_file.write_text(
        json.dumps(
            {
                "schema_version": schema_mod.SCHEMA_VERSION,
                "stats": stats,
                "agent_data": {},
                "facet_data": {},
                "heatmap_data": {},
            }
        ),
        encoding="utf-8",
    )

    def fail_scan_day(*_args, **_kwargs):
        raise AssertionError("cache should have been reused")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(stats_mod.JournalStats, "scan_day", fail_scan_day)
    js = stats_mod.JournalStats()
    js.scan(str(journal), verbose=False, use_cache=True)

    assert js.days["20240101"]["transcript_sessions"] == 7


def test_day_cache_recompute_failure_leaves_prior_file_intact(tmp_path, monkeypatch):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = tmp_path
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)
    cache_file = day / "stats.json"
    original = {"stats": {"transcript_sessions": 99}}
    cache_file.write_text(json.dumps(original), encoding="utf-8")

    def fail_scan_day(*_args, **_kwargs):
        raise RuntimeError("scan failed")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    monkeypatch.setattr(stats_mod.JournalStats, "scan_day", fail_scan_day)
    js = stats_mod.JournalStats()

    with pytest.raises(RuntimeError, match="scan failed"):
        js.scan(str(journal), verbose=False, use_cache=True)

    assert json.loads(cache_file.read_text(encoding="utf-8")) == original


def test_root_stats_contains_backlog_contract_fields():
    stats_mod = importlib.import_module("solstone.think.journal_stats")

    data = stats_mod.JournalStats().to_dict()

    assert data["totals"]["backlog_pending_days"] == 0
    assert data["totals"]["backlog_stuck_days"] == 0
    assert data["backlog"] == {
        "window": stats_mod.BACKLOG_DEFAULT_WINDOW,
        "days": [],
        "pending_days": 0,
        "stuck_days": 0,
        "oldest_pending_day": None,
        "errors": [],
        "degraded": False,
    }


def test_backlog_derivation_failure_marks_stats_degraded(tmp_path, monkeypatch, caplog):
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

    def fail_backlog_view():
        raise RuntimeError("backlog unavailable")

    monkeypatch.setattr(stats_mod, "read_backlog_view", fail_backlog_view)
    caplog.set_level(logging.ERROR, logger=stats_mod.__name__)

    js = stats_mod.JournalStats()
    js.scan(str(tmp_path), verbose=False, use_cache=False)
    data = js.to_dict()

    assert js.backlog_view is not None
    assert js.backlog_view.degraded is True
    assert data["backlog"]["degraded"] is True
    assert any(
        "backlog derivation failed; stats will be flagged degraded"
        in record.getMessage()
        for record in caplog.records
    )


def test_token_usage_new_format(tmp_path, monkeypatch):
    """Test that the new unified token format is properly handled."""
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = tmp_path
    day1 = journal / "chronicle" / "20240101"
    day1.mkdir(parents=True)

    # Create tokens directory with new format token files
    tokens_dir = journal / "tokens"
    tokens_dir.mkdir()

    # New format: input_tokens, output_tokens, reasoning_tokens
    token_new = {
        "timestamp": 1704067200.0,
        "timestamp_str": "20240101_120000",
        "model": "gemini-2.5-flash",
        "context": "models._log_token_usage:241",
        "usage": {
            "input_tokens": 1716,
            "output_tokens": 3710,
            "total_tokens": 10114,
            "reasoning_tokens": 4688,
        },
    }

    # Write token as JSONL format
    (tokens_dir / "20240101.jsonl").write_text(json.dumps(token_new) + "\n")

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    js = stats_mod.JournalStats()
    js.scan(str(journal))

    # Check token usage is properly parsed
    assert "20240101" in js.token_usage
    assert "gemini-2.5-flash" in js.token_usage["20240101"]

    # Check new format fields are present
    gemini_usage = js.token_usage["20240101"]["gemini-2.5-flash"]
    assert gemini_usage["input_tokens"] == 1716
    assert gemini_usage["output_tokens"] == 3710
    assert gemini_usage["total_tokens"] == 10114
    assert gemini_usage["reasoning_tokens"] == 4688

    # Check overall model totals
    assert js.token_totals["gemini-2.5-flash"]["input_tokens"] == 1716
    assert js.token_totals["gemini-2.5-flash"]["output_tokens"] == 3710
    assert js.token_totals["gemini-2.5-flash"]["reasoning_tokens"] == 4688


def test_process_token_entry_counts_all_int_usage_fields(tmp_path, monkeypatch):
    """Int-valued fields in usage are all counted; top-level metadata is ignored."""
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    journal = tmp_path
    day1 = journal / "chronicle" / "20240101"
    day1.mkdir(parents=True)

    tokens_dir = journal / "tokens"
    tokens_dir.mkdir()

    token_entry_with_duration = {
        "timestamp": 1704067200.0,
        "model": "gemini-2.5-flash",
        "context": "talent.system.meetings",
        "type": "cogitate",
        "usage": {
            "input_tokens": 100,
            "output_tokens": 50,
            "total_tokens": 150,
            "duration_ms": 3000,
        },
    }

    token_entry_without_duration = {
        "timestamp": 1704067300.0,
        "model": "gemini-2.5-pro",
        "context": "think.detect_transcript.detect",
        "type": "generate",
        "usage": {
            "input_tokens": 80,
            "output_tokens": 20,
            "total_tokens": 100,
        },
    }

    (tokens_dir / "20240101.jsonl").write_text(
        json.dumps(token_entry_with_duration)
        + "\n"
        + json.dumps(token_entry_without_duration)
        + "\n"
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    js = stats_mod.JournalStats()
    js.scan(str(journal))

    usage_with_duration = js.token_usage["20240101"]["gemini-2.5-flash"]
    assert usage_with_duration["input_tokens"] == 100
    assert usage_with_duration["output_tokens"] == 50
    assert usage_with_duration["total_tokens"] == 150
    assert usage_with_duration["duration_ms"] == 3000
    assert "type" not in usage_with_duration

    usage_without_duration = js.token_usage["20240101"]["gemini-2.5-pro"]
    assert usage_without_duration["input_tokens"] == 80
    assert usage_without_duration["output_tokens"] == 20
    assert usage_without_duration["total_tokens"] == 100
    assert "duration_ms" not in usage_without_duration
    assert "type" not in usage_without_duration
