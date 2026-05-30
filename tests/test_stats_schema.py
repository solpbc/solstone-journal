# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib

import pytest


def _minimal_valid_stats(schema_mod):
    return {
        "schema_version": schema_mod.SCHEMA_VERSION,
        "generated_at": "2026-05-30T00:00:00+00:00",
        "day_count": 0,
        "days": {},
        "totals": {field: 0 for field in schema_mod.TOTAL_FIELDS},
        "heatmap": [],
        "tokens": {},
        "talents": {},
        "facets": {},
        "backlog": {
            "window": 30,
            "days": [],
            "pending_days": 0,
            "stuck_days": 0,
            "oldest_pending_day": None,
            "errors": [],
        },
    }


def test_schema_version_is_6():
    schema_mod = importlib.import_module("solstone.think.stats_schema")

    assert schema_mod.SCHEMA_VERSION == 6


def test_validate_passes_on_valid_output(tmp_path, monkeypatch):
    """Build a JournalStats from fixture data, call to_dict(), validate."""
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    schema_mod = importlib.import_module("solstone.think.stats_schema")
    journal = tmp_path
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)

    # Create minimal transcript fixture
    ts_dir = day / "default" / "123456_300"
    ts_dir.mkdir(parents=True)
    (ts_dir / "audio.jsonl").write_text(
        '{"raw": "raw.flac"}\n{"start": "10:00:00", "text": "hello"}\n'
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    js = stats_mod.JournalStats()
    js.scan(str(journal))

    data = js.to_dict()
    errors = schema_mod.validate(data)
    assert errors == [], f"Validation errors: {errors}"


def test_validate_rejects_missing_fields():
    """Incomplete dicts should produce non-empty error lists."""
    schema_mod = importlib.import_module("solstone.think.stats_schema")

    # Empty dict
    errors = schema_mod.validate({})
    assert len(errors) > 0
    assert any("schema_version" in e for e in errors)

    # Missing days
    errors = schema_mod.validate(
        {"schema_version": 2, "generated_at": "2026-04-10T00:00:00+00:00"}
    )
    assert any("days" in e for e in errors)

    # Wrong schema version
    errors = schema_mod.validate(
        {
            "schema_version": 99,
            "generated_at": "x",
            "day_count": 0,
            "days": {},
            "totals": {},
            "heatmap": [],
            "tokens": {},
            "agents": {},
            "facets": {},
        }
    )
    assert any("schema_version" in e for e in errors)


def test_save_json_raises_on_invalid(tmp_path, monkeypatch):
    """save_json() must raise ValueError when validation fails."""
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    js = stats_mod.JournalStats()
    # Corrupt the schema version so validation fails
    original = js.to_dict
    js.to_dict = lambda: {**original(), "schema_version": 99}
    with pytest.raises(ValueError, match="Stats validation failed"):
        js.save_json(str(tmp_path))


def test_validate_rejects_missing_backlog_contract_fields():
    schema_mod = importlib.import_module("solstone.think.stats_schema")
    data = _minimal_valid_stats(schema_mod)

    del data["totals"]["backlog_pending_days"]
    errors = schema_mod.validate(data)
    assert any("backlog_pending_days" in error for error in errors)

    data = _minimal_valid_stats(schema_mod)
    del data["totals"]["backlog_stuck_days"]
    errors = schema_mod.validate(data)
    assert any("backlog_stuck_days" in error for error in errors)

    data = _minimal_valid_stats(schema_mod)
    del data["backlog"]
    errors = schema_mod.validate(data)
    assert any("backlog" in error for error in errors)


def test_day_fields_present_in_scan_day(tmp_path, monkeypatch):
    """Verify every key in DAY_FIELDS appears in scan_day output."""
    stats_mod = importlib.import_module("solstone.think.journal_stats")
    schema_mod = importlib.import_module("solstone.think.stats_schema")
    journal = tmp_path
    day = journal / "chronicle" / "20240101"
    day.mkdir(parents=True)

    # Create transcript and percept fixtures
    ts_dir = day / "default" / "123456_300"
    ts_dir.mkdir(parents=True)
    (ts_dir / "audio.jsonl").write_text(
        '{"raw": "raw.flac"}\n{"start": "10:00:00", "text": "hello"}\n'
    )
    (ts_dir / "screen.jsonl").write_text(
        '{"header": true}\n{"frame_id": 1, "timestamp": "10:00:00"}\n'
    )

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    js = stats_mod.JournalStats()
    day_data = js.scan_day("20240101", str(day))

    stats = day_data["stats"]
    for field in schema_mod.DAY_FIELDS:
        assert field in stats, (
            f"DAY_FIELDS field '{field}' missing from scan_day output"
        )
