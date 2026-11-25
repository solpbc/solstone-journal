// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native Experiences talent runtime: deterministic pre-filter, two-stage
//! daily detector (`experiences_detect_facet` + `experiences_detect`),
//! narrative composer (`experience_compose`), and backend-agnostic runners.

pub mod compose;
pub mod detect;
pub mod prefilter;
pub mod runner;

pub use compose::{ComposeAudit, ComposePacket, ComposePreState};
pub use detect::{DetectedCandidate, Stage1APreState, Stage1BPreState};
pub use prefilter::{ActiveFacetInfo, DayPool, OwnerIdentity, PoolItem, humanize_stream_label};
pub use runner::{ComposeRunResult, DetectDayResult, run_compose_experience, run_detect_day};
