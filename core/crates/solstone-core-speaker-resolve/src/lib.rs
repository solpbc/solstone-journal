// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-backed speaker-attribution resolution.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod bootstrap;
pub mod backfill;
pub mod backfill_operations;
pub mod candidate_tracker;
pub mod discovery_cache;
pub mod direct_voiceprints;
pub mod evidence;
pub mod eligibility;
pub mod identify_cluster;
pub mod identify_target;
pub mod keep_separate;
pub mod identify_operations;
pub mod identify_undo_phases;
pub mod identify_undo;
pub mod identify_forward_phases;
pub mod layer1;
pub mod layer2;
pub mod layer3;
pub mod owner_candidate;
pub mod owner_centroid;
pub mod resolve;
pub mod retroactive_confirm;
pub mod voiceprint_accumulation;
pub mod voiceprint_centroid;
pub mod voiceprint_metadata;

mod person_guard;
