// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Attribution calibration mirrored from `encoder_config.py`.

/// Compatibility pin only; runtime owner decisions use the persisted centroid record.
pub const OWNER_THRESHOLD: f32 = 0.43;
/// Compatibility pin only; runtime owner decisions use the persisted centroid record.
pub const OWNER_MARGIN_MIN: f32 = 0.05;
pub const ACOUSTIC_HIGH: f32 = 0.36;
pub const ACOUSTIC_MEDIUM: f32 = 0.22;
pub const ACOUSTIC_MARGIN_MIN: f32 = 0.05;
pub const CC_COVERAGE_GATE: f32 = 0.45;
pub const CC_CONFIDENCE_GATE: f32 = 0.28;
pub const VP_DECAY_LAMBDA: f64 = std::f64::consts::LN_2 / 120.0;
pub const VP_OUTLIER_MIN_SIMILARITY: f32 = 0.18;
pub const VP_OUTLIER_MIN_SAMPLES: usize = 5;
pub const NOISY_FLYWHEEL_OVERLAP_MAX: f32 = 0.10;
pub const OWNER_BOOTSTRAP_PROVISIONAL_GUARD_MIN_TAGS: usize = 5;
pub const OWNER_REBUILD_MIN_CENTROID_AGREEMENT: f32 = 0.80;
pub const OWNER_REBUILD_MIN_CLUSTER_SIZE_RATIO: f32 = 0.80;
pub const OWNER_REBUILD_MAX_COHESION_DROP: f32 = 0.05;

/// Name-resolution threshold from `solstone/think/entities/matching.py`.
/// This is not one of the encoder-config values pinned by AC25.
pub const RESOLUTION_FUZZY_THRESHOLD: f64 = 90.0;
