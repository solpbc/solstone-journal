// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Entity-backed speaker-attribution resolution.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

pub mod evidence;
pub mod layer1;
pub mod layer2;
pub mod layer3;
pub mod owner_candidate;
pub mod owner_centroid;
pub mod resolve;
pub mod voiceprint_accumulation;
pub mod voiceprint_centroid;

mod person_guard;
