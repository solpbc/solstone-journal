# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
from pathlib import Path
from types import SimpleNamespace

import numpy as np
from jsonschema import Draft202012Validator

import solstone.observe.transcribe.gemini as gemini_mod


def _load_schema() -> dict:
    with (
        Path(__file__).resolve().parents[1]
        / "solstone"
        / "observe"
        / "transcribe"
        / "gemini.schema.json"
    ).open(encoding="utf-8") as f:
        return json.load(f)


def test_gemini_schema_file_is_valid_draft_2020_12():
    Draft202012Validator.check_schema(_load_schema())


def test_gemini_schema_accepts_and_rejects_expected_values():
    validator = Draft202012Validator(_load_schema())

    assert validator.is_valid({"segments": []})
    assert validator.is_valid(
        {"segments": [{"start": "01:23", "speaker": "Speaker 1", "text": "hi"}]}
    )
    assert validator.is_valid(
        {
            "segments": [
                {"start": "00:00", "speaker": "Speaker 1", "text": "hello"},
                {"start": "00:05", "speaker": "Speaker 2", "text": "hi back"},
            ]
        }
    )
    assert not validator.is_valid(
        [{"start": "01:23", "speaker": "Speaker 1", "text": "hi"}]
    )
    assert not validator.is_valid(
        {"transcript": [{"start": "01:23", "speaker": "Speaker 1", "text": "hi"}]}
    )
    assert not validator.is_valid({"segments": [], "extra": 1})
    assert not validator.is_valid(
        {"segments": [{"speaker": "Speaker 1", "text": "hi"}]}
    )
    assert not validator.is_valid({"segments": [{"start": "01:23", "text": "hi"}]})
    assert not validator.is_valid(
        {"segments": [{"start": "01:23", "speaker": "Speaker 1"}]}
    )
    assert not validator.is_valid(
        {
            "segments": [
                {
                    "start": "01:23",
                    "speaker": "s",
                    "text": "t",
                    "confidence": 0.9,
                }
            ]
        }
    )
    assert not validator.is_valid(
        {"segments": [{"start": "01:23", "speaker": "Speaker 1", "text": 7}]}
    )
    assert not validator.is_valid(
        {"segments": [{"start": "01:23", "speaker": 7, "text": "hi"}]}
    )
    assert not validator.is_valid(
        {"segments": [{"start": "1:23", "speaker": "Speaker 1", "text": "hi"}]}
    )
    assert not validator.is_valid(
        {"segments": [{"start": "01:23:45", "speaker": "Speaker 1", "text": "hi"}]}
    )
    assert not validator.is_valid(
        {"segments": [{"start": "01-23", "speaker": "Speaker 1", "text": "hi"}]}
    )
    assert not validator.is_valid(
        {"segments": [{"start": 83, "speaker": "Speaker 1", "text": "hi"}]}
    )


def test_transcribe_passes_schema_to_generate(monkeypatch):
    captured = {}

    def fake_generate(**kwargs):
        captured.update(kwargs)
        return json.dumps(
            {"segments": [{"start": "00:00", "speaker": "Speaker 1", "text": "hello"}]}
        )

    monkeypatch.setattr(gemini_mod, "generate", fake_generate)
    monkeypatch.setattr(gemini_mod, "audio_to_flac_bytes", lambda *_args: b"flac")
    monkeypatch.setattr(
        gemini_mod.types.Part,
        "from_bytes",
        staticmethod(lambda data, mime_type: {"data": data, "mime_type": mime_type}),
    )
    monkeypatch.setattr(
        gemini_mod,
        "load_prompt",
        lambda *_args, **_kwargs: SimpleNamespace(text="Prompt"),
    )

    gemini_mod.transcribe(
        np.zeros(16000, dtype=np.float32),
        16000,
        {},
        [(0.0, 1.0)],
    )

    assert captured["json_schema"] is gemini_mod._SCHEMA
