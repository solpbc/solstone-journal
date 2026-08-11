# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Test the constant-import contract for encoder_config."""

import ast
import math
from pathlib import Path

from solstone.apps.speakers import attribution, candidate_tracker, encoder_config


def test_locked_constants():
    assert encoder_config.ENCODER_ID == "wespeaker-resnet34-256"
    assert (
        encoder_config.WESPEAKER_MODEL_SHA256
        == "5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94"
    )
    assert encoder_config.OWNER_THRESHOLD == 0.43
    assert encoder_config.OWNER_MARGIN_MIN == 0.05
    assert encoder_config.SOLO_CLUSTER_MIN_COSINE == 0.43
    assert encoder_config.ACOUSTIC_HIGH == 0.36
    assert encoder_config.ACOUSTIC_MARGIN_MIN == 0.05
    assert encoder_config.ACOUSTIC_MEDIUM == 0.22
    assert encoder_config.VP_DECAY_LAMBDA == math.log(2) / 120
    assert encoder_config.VP_OUTLIER_MIN_SIMILARITY == 0.18
    assert encoder_config.VP_OUTLIER_MIN_SAMPLES == 5
    assert encoder_config.CC_COVERAGE_GATE == 0.45
    assert encoder_config.CC_CONFIDENCE_GATE == 0.28
    assert encoder_config.OWNER_BOOTSTRAP_MIN_STMTS == 30
    assert encoder_config.OWNER_BOOTSTRAP_MIN_MEDIAN_DURATION_S == 1.5
    assert encoder_config.OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25 == 0.30
    assert encoder_config.OWNER_BOOTSTRAP_STRONG_EVIDENCE_MIN_STMTS == 100
    assert encoder_config.OWNER_BOOTSTRAP_MIN_INTRA_COSINE_P25_STRONG == 0.15
    assert encoder_config.OWNER_BOOTSTRAP_EVIDENCE_TIER_STANDARD == "standard"
    assert encoder_config.OWNER_BOOTSTRAP_EVIDENCE_TIER_STRONG == "strong"
    assert encoder_config.OWNER_BOOTSTRAP_PROVISIONAL_GUARD_MIN_TAGS == 5
    assert encoder_config.NOISY_FLYWHEEL_OVERLAP_MAX == 0.10
    assert encoder_config.SLOT_ACTIVE_MIN_SHARE == 0.10
    assert encoder_config.SPEAKER_EVIDENCE_MULTI_MIN == 0.05
    assert encoder_config.SPEAKER_EVIDENCE_SINGLE_MAX == 0.05
    assert encoder_config.DIARIZE_MIN_OVERLAP == 0.05
    assert encoder_config.SPEAKER_EVIDENCE_VERSION == "windowed-slots-v1"
    assert encoder_config.OVERLAP_DETECTOR_ID == "pyannote-segmentation-3.0-onnx"
    assert (
        encoder_config.OVERLAP_DETECTOR_SHA256
        == "057ee564753071c0b09b5b611648b50ac188d50846bff5f01e9f7bbf1591ea25"
    )
    assert encoder_config.MERGE_THRESHOLD == 0.72
    assert encoder_config.SPLIT_THRESHOLD == 0.55
    assert encoder_config.STABILITY_THRESHOLD == 0.25
    assert encoder_config.CONSOLIDATE_MIN_INTERVALS == 30
    assert encoder_config.CONSOLIDATE_MERGE_THRESHOLD == 0.65
    assert encoder_config.CONSOLIDATE_SUGGEST_MIN == 0.45
    assert encoder_config.CONFIRM_MIN_SEGMENTS == 2
    assert encoder_config.CONFIRM_MIN_INTERVALS == 5
    assert encoder_config.CONFIRM_MIN_DURATION_S == 25.0


def _module_assignment_targets(path: Path) -> set[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    targets: set[str] = set()
    for node in tree.body:
        if isinstance(node, ast.Assign):
            for target in node.targets:
                if isinstance(target, ast.Name):
                    targets.add(target.id)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            targets.add(node.target.id)
    return targets


def test_candidate_tracker_constants_are_not_assigned_in_tracker_module():
    moved_constants = {
        "MERGE_THRESHOLD",
        "SPLIT_THRESHOLD",
        "STABILITY_THRESHOLD",
        "CONSOLIDATE_MIN_INTERVALS",
        "CONSOLIDATE_MERGE_THRESHOLD",
        "CONSOLIDATE_SUGGEST_MIN",
        "CONFIRM_MIN_SEGMENTS",
        "CONFIRM_MIN_INTERVALS",
        "CONFIRM_MIN_DURATION_S",
    }
    targets = _module_assignment_targets(Path(candidate_tracker.__file__))

    assert moved_constants.isdisjoint(targets)


def test_solo_cluster_min_cosine_is_independently_assigned():
    targets = _module_assignment_targets(Path(encoder_config.__file__))

    assert "OWNER_THRESHOLD" in targets
    assert "SOLO_CLUSTER_MIN_COSINE" in targets


def test_attribution_imports_acoustic_constants():
    assert attribution.ACOUSTIC_HIGH is encoder_config.ACOUSTIC_HIGH
    assert attribution.ACOUSTIC_MARGIN_MIN is encoder_config.ACOUSTIC_MARGIN_MIN
    assert attribution.ACOUSTIC_MEDIUM is encoder_config.ACOUSTIC_MEDIUM
    assert attribution.VP_DECAY_LAMBDA is encoder_config.VP_DECAY_LAMBDA
    assert (
        attribution.VP_OUTLIER_MIN_SIMILARITY
        is encoder_config.VP_OUTLIER_MIN_SIMILARITY
    )
    assert attribution.VP_OUTLIER_MIN_SAMPLES is encoder_config.VP_OUTLIER_MIN_SAMPLES
    assert attribution.CC_COVERAGE_GATE is encoder_config.CC_COVERAGE_GATE
    assert attribution.CC_CONFIDENCE_GATE is encoder_config.CC_CONFIDENCE_GATE
