// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Literal calibration pins for values transcribed by the P6 speaker-resolve port.

use solstone_core_speaker_resolve::bootstrap::NAME_MERGE_THRESHOLD;
use solstone_core_speaker_resolve::candidate_tracker::{
    CONFIRM_MIN_DURATION_S, CONFIRM_MIN_INTERVALS, CONFIRM_MIN_SEGMENTS,
    CONSOLIDATE_MERGE_THRESHOLD, CONSOLIDATE_MIN_INTERVALS, CONSOLIDATE_SUGGEST_MIN,
    MERGE_THRESHOLD, SOLO_CLUSTER_MIN_COSINE, SPLIT_THRESHOLD, STABILITY_THRESHOLD,
};

#[test]
fn ac24_calibration_literals_match_python_sources_of_truth() {
    // `solstone/apps/speakers/encoder_config.py`; VP_OUTLIER_* are already pinned by
    // P5's `ac25_calibration_matches_encoder_config_source_of_truth` against the
    // shared `solstone-core-speaker-id` canonical declarations.
    assert_eq!(SOLO_CLUSTER_MIN_COSINE, 0.43);
    assert_eq!(MERGE_THRESHOLD, 0.72);
    assert_eq!(SPLIT_THRESHOLD, 0.55);
    assert_eq!(STABILITY_THRESHOLD, 0.25);
    assert_eq!(CONSOLIDATE_MIN_INTERVALS, 30);
    assert_eq!(CONSOLIDATE_MERGE_THRESHOLD, 0.65);
    assert_eq!(CONSOLIDATE_SUGGEST_MIN, 0.45);
    assert_eq!(CONFIRM_MIN_SEGMENTS, 2);
    assert_eq!(CONFIRM_MIN_INTERVALS, 5);
    assert_eq!(CONFIRM_MIN_DURATION_S, 25.0);

    // `solstone/apps/speakers/bootstrap.py:53`.
    assert_eq!(NAME_MERGE_THRESHOLD, 0.90);
}
