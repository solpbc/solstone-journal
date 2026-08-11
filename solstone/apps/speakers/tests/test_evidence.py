# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json

from solstone.apps.speakers.evidence import (
    UNKNOWN_SPEAKER_EVIDENCE,
    _read_segment_overlap_fraction,
    _read_segment_speaker_evidence,
)
from solstone.apps.speakers.encoder_config import SPEAKER_EVIDENCE_VERSION


def test_read_segment_speaker_evidence_returns_header_fields(tmp_path):
    path = tmp_path / "audio.jsonl"
    path.write_text(
        json.dumps(
            {
                "speaker_evidence": "multi",
                "speaker_evidence_multi_fraction": 0.125,
                "speaker_evidence_version": SPEAKER_EVIDENCE_VERSION,
            }
        )
        + "\n",
        encoding="utf-8",
    )

    result = _read_segment_speaker_evidence(path)

    assert result.speaker_evidence == "multi"
    assert result.multi_fraction == 0.125
    assert result.version == SPEAKER_EVIDENCE_VERSION


def test_read_segment_speaker_evidence_unknown_for_absent_or_corrupt_header(tmp_path):
    missing = tmp_path / "missing.jsonl"
    corrupt = tmp_path / "corrupt.jsonl"
    corrupt.write_text("{not-json}\n", encoding="utf-8")
    absent = tmp_path / "absent.jsonl"
    absent.write_text(json.dumps({"raw": "audio.flac"}) + "\n", encoding="utf-8")
    wrong_version = tmp_path / "wrong-version.jsonl"
    wrong_version.write_text(
        json.dumps(
            {
                "speaker_evidence": "multi",
                "speaker_evidence_multi_fraction": 0.5,
                "speaker_evidence_version": "other",
            }
        )
        + "\n",
        encoding="utf-8",
    )

    assert _read_segment_speaker_evidence(missing) == UNKNOWN_SPEAKER_EVIDENCE
    assert _read_segment_speaker_evidence(corrupt) == UNKNOWN_SPEAKER_EVIDENCE
    assert _read_segment_speaker_evidence(absent) == UNKNOWN_SPEAKER_EVIDENCE
    assert _read_segment_speaker_evidence(wrong_version) == UNKNOWN_SPEAKER_EVIDENCE


def test_read_segment_overlap_fraction_legacy_absent_and_corrupt_return_zero(tmp_path):
    missing = tmp_path / "missing.jsonl"
    corrupt = tmp_path / "corrupt.jsonl"
    corrupt.write_text("{not-json}\n", encoding="utf-8")
    absent = tmp_path / "absent.jsonl"
    absent.write_text(json.dumps({"raw": "audio.flac"}) + "\n", encoding="utf-8")

    assert _read_segment_overlap_fraction(missing) == 0.0
    assert _read_segment_overlap_fraction(corrupt) == 0.0
    assert _read_segment_overlap_fraction(absent) == 0.0
