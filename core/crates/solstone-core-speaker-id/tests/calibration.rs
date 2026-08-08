// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_speaker_id::calibration;

#[test]
fn ac25_calibration_matches_encoder_config_source_of_truth() {
    // Source of truth: solstone/apps/speakers/encoder_config.py.
    assert_eq!(calibration::OWNER_THRESHOLD, 0.43);
    assert_eq!(calibration::OWNER_MARGIN_MIN, 0.05);
    assert_eq!(calibration::ACOUSTIC_HIGH, 0.36);
    assert_eq!(calibration::ACOUSTIC_MEDIUM, 0.22);
    assert_eq!(calibration::ACOUSTIC_MARGIN_MIN, 0.05);
    assert_eq!(calibration::CC_COVERAGE_GATE, 0.45);
    assert_eq!(calibration::CC_CONFIDENCE_GATE, 0.28);
    assert_eq!(calibration::VP_DECAY_LAMBDA, std::f64::consts::LN_2 / 120.0);
}
