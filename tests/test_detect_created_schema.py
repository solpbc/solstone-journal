# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import importlib
import json
from pathlib import Path

from jsonschema import Draft202012Validator

import solstone.think.models as models

detect_created_mod = importlib.import_module("solstone.think.detect_created")

DETECT_CREATED_SCHEMA_PATH = (
    Path(__file__).resolve().parents[1]
    / "solstone"
    / "think"
    / "detect_created.schema.json"
)


def _load_detect_created_schema() -> dict:
    return json.loads(DETECT_CREATED_SCHEMA_PATH.read_text(encoding="utf-8"))


def test_detect_created_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_detect_created_schema())


def test_detect_created_schema_accepts_and_rejects_expected_values():
    schema = _load_detect_created_schema()
    validator = Draft202012Validator(schema)
    valid = {
        "day": "20240315",
        "time": "143052",
        "confidence": "high",
        "source": "QuickTime:CreateDate",
        "utc": True,
    }

    assert validator.is_valid(valid)
    assert not validator.is_valid(
        {
            "day": "20240315",
            "time": "143052",
            "confidence": "high",
            "source": "QuickTime:CreateDate",
        }
    )
    assert not validator.is_valid({**valid, "day": "2024-03-15"})
    assert not validator.is_valid({**valid, "time": "14:30:52"})
    assert not validator.is_valid({**valid, "confidence": "certain"})
    assert not validator.is_valid({**valid, "extra": "x"})
    assert not validator.is_valid({**valid, "source": 42})


def test_detect_created_passes_schema_to_generate(monkeypatch):
    captured = {}

    def fake_generate(**kwargs):
        captured.update(kwargs)
        return (
            '{"day": "20240315", "time": "143052", "confidence": "high", '
            '"source": "QuickTime:CreateDate", "utc": false}'
        )

    monkeypatch.setattr(models, "generate", fake_generate)
    monkeypatch.setattr(
        detect_created_mod,
        "_extract_metadata",
        lambda path: "QuickTime Create Date : 2024:03:15 14:30:52",
    )

    result = detect_created_mod.detect_created("/dev/null")

    assert captured["json_schema"] is detect_created_mod._SCHEMA
    assert result == {
        "day": "20240315",
        "time": "143052",
        "confidence": "high",
        "source": "QuickTime:CreateDate",
        "utc": False,
    }
