# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import json
from pathlib import Path

from jsonschema import Draft202012Validator

import solstone.think.models as models

mod = importlib.import_module("solstone.think.detect_transcript")

DETECT_TRANSCRIPT_SEGMENT_SCHEMA_PATH = (
    Path(__file__).resolve().parents[1]
    / "solstone"
    / "think"
    / "detect_transcript_segment.schema.json"
)
DETECT_TRANSCRIPT_JSON_SCHEMA_PATH = (
    Path(__file__).resolve().parents[1]
    / "solstone"
    / "think"
    / "detect_transcript_json.schema.json"
)


def _load_detect_transcript_segment_schema() -> dict:
    return json.loads(DETECT_TRANSCRIPT_SEGMENT_SCHEMA_PATH.read_text(encoding="utf-8"))


def _load_detect_transcript_json_schema() -> dict:
    return json.loads(DETECT_TRANSCRIPT_JSON_SCHEMA_PATH.read_text(encoding="utf-8"))


def test_detect_transcript_segment_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_detect_transcript_segment_schema())


def test_detect_transcript_json_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_detect_transcript_json_schema())


def test_detect_transcript_segment_schema_accepts_and_rejects_expected_values():
    schema = _load_detect_transcript_segment_schema()
    validator = Draft202012Validator(schema)
    valid = [{"start_at": "12:34:56", "line": 1}]

    assert validator.is_valid(valid)
    assert not validator.is_valid([{"start_at": "12:34:56"}])
    assert not validator.is_valid([{"start_at": "12:34", "line": 1}])
    assert not validator.is_valid([{"start_at": "12:34:56", "line": "1"}])
    assert not validator.is_valid([{"start_at": "12:34:56", "line": 0}])
    assert not validator.is_valid([{"start_at": "12:34:56", "line": 1, "extra": "x"}])


def test_detect_transcript_json_schema_accepts_and_rejects_expected_values():
    schema = _load_detect_transcript_json_schema()
    validator = Draft202012Validator(schema)
    valid = {
        "entries": [{"start": "12:34:56", "speaker": "Alice", "text": "Hello"}],
        "topics": "planning, budget",
        "setting": "workplace",
    }

    assert validator.is_valid(valid)
    assert validator.is_valid({**valid, "topics": "", "setting": ""})
    assert not validator.is_valid(
        {"topics": "planning, budget", "setting": "workplace"}
    )
    assert not validator.is_valid(
        {
            **valid,
            "entries": [{"start": "12:34", "speaker": "Alice", "text": "Hello"}],
        }
    )
    assert not validator.is_valid(
        {
            **valid,
            "entries": [{"start": "12:34:56", "speaker": 1, "text": "Hello"}],
        }
    )
    assert not validator.is_valid(
        {
            **valid,
            "entries": [{"start": "12:34:56", "speaker": "Alice", "text": 7}],
        }
    )
    assert not validator.is_valid({**valid, "extra": "x"})
    assert not validator.is_valid(
        {
            **valid,
            "entries": [
                {
                    "start": "12:34:56",
                    "speaker": "Alice",
                    "text": "Hello",
                    "extra": "x",
                }
            ],
        }
    )


def test_detect_transcript_segment_passes_schema_to_generate(monkeypatch):
    captured = {}

    def fake_generate(**kwargs):
        captured.update(kwargs)
        return '[{"start_at": "12:00:00", "line": 1}]'

    monkeypatch.setattr(models, "generate", fake_generate)

    result = mod.detect_transcript_segment("01\n02\n", "12:00:00")

    assert captured["json_schema"] is mod._SEGMENT_SCHEMA
    assert result
    assert all(isinstance(item, tuple) and len(item) == 2 for item in result)


def test_detect_transcript_json_passes_schema_to_generate(monkeypatch):
    captured = {}

    def fake_generate(**kwargs):
        captured.update(kwargs)
        return (
            '{"entries": [{"start": "12:00:00", "speaker": "Alice", "text": "Hello"}], '
            '"topics": "planning", "setting": "workplace"}'
        )

    monkeypatch.setattr(models, "generate", fake_generate)

    result = mod.detect_transcript_json("some text", "12:00:00")

    assert captured["json_schema"] is mod._JSON_SCHEMA
    assert result == {
        "entries": [{"start": "12:00:00", "speaker": "Alice", "text": "Hello"}],
        "topics": "planning",
        "setting": "workplace",
    }
