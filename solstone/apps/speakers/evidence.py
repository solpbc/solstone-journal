# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Shared speaker-evidence wire types."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import NamedTuple

from solstone.apps.speakers.encoder_config import SPEAKER_EVIDENCE_VERSION

VALID_SPEAKER_EVIDENCE_DECISIONS = frozenset({"none", "single", "multi"})
logger = logging.getLogger(__name__)


class SpeakerEvidenceDecision(NamedTuple):
    speaker_evidence: str
    multi_window_fraction: float
    mean_window_overlap_share: float


def _read_segment_overlap_fraction(jsonl_path: Path) -> float:
    """Return overlap_fraction from a chronicle JSONL header, or 0.0 if absent."""
    try:
        with jsonl_path.open(encoding="utf-8") as f:
            line = f.readline()
        if not line:
            return 0.0
        header = json.loads(line)
    except FileNotFoundError:
        return 0.0
    except (OSError, json.JSONDecodeError) as exc:
        logger.info("overlap header read failed at %s: %s", jsonl_path, exc)
        return 0.0

    value = header.get("overlap_fraction", 0.0)
    try:
        return float(value)
    except (TypeError, ValueError):
        return 0.0


class SegmentSpeakerEvidence(NamedTuple):
    speaker_evidence: str
    multi_fraction: float | None
    version: str | None


UNKNOWN_SPEAKER_EVIDENCE = SegmentSpeakerEvidence(
    speaker_evidence="unknown",
    multi_fraction=None,
    version=None,
)


def _read_segment_speaker_evidence(jsonl_path: Path) -> SegmentSpeakerEvidence:
    """Return speaker-evidence metadata, or explicit unknown on absent/corrupt input."""
    try:
        with jsonl_path.open(encoding="utf-8") as f:
            line = f.readline()
        if not line:
            return UNKNOWN_SPEAKER_EVIDENCE
        header = json.loads(line)
    except FileNotFoundError:
        return UNKNOWN_SPEAKER_EVIDENCE
    except (OSError, json.JSONDecodeError) as exc:
        logger.info("speaker evidence header read failed at %s: %s", jsonl_path, exc)
        return UNKNOWN_SPEAKER_EVIDENCE

    if not isinstance(header, dict):
        return UNKNOWN_SPEAKER_EVIDENCE

    speaker_evidence = header.get("speaker_evidence")
    version = header.get("speaker_evidence_version")
    if (
        speaker_evidence not in VALID_SPEAKER_EVIDENCE_DECISIONS
        or version != SPEAKER_EVIDENCE_VERSION
    ):
        return UNKNOWN_SPEAKER_EVIDENCE

    try:
        multi_fraction = float(header["speaker_evidence_multi_fraction"])
    except (KeyError, TypeError, ValueError):
        return UNKNOWN_SPEAKER_EVIDENCE

    return SegmentSpeakerEvidence(
        speaker_evidence=speaker_evidence,
        multi_fraction=multi_fraction,
        version=version,
    )


__all__ = [
    "SegmentSpeakerEvidence",
    "SpeakerEvidenceDecision",
    "UNKNOWN_SPEAKER_EVIDENCE",
    "VALID_SPEAKER_EVIDENCE_DECISIONS",
]
