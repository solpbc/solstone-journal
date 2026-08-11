// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Apple Health source seam; the real signature is defined by a later wave.

use crate::ImportSourcesError;

/// The compiled-in source name used by the resolver's Apple pre-empt verdict.
pub const APPLE_HEALTH_SOURCE: &str = "apple_health";

/// A source-layer marker for the native Apple Health body route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RouteAppleHealth;

pub fn reserved_seam() -> Result<(), ImportSourcesError> {
    Err(ImportSourcesError::Unimplemented {
        module: "apple_health",
    })
}
