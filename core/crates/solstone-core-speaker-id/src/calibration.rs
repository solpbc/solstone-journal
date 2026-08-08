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
