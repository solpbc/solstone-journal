// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Oura source seam; the real signature is defined by a later wave.

use crate::ImportSourcesError;

/// Oura file imports must use the native sync route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OuraFileSourceRefusal;

impl OuraFileSourceRefusal {
    /// Owner-facing remedy retained from the Python file-import route.
    pub const MESSAGE: &str = solstone_core_import::detect::OURA_SYNC_REMEDY;
}

/// The sources-layer marker always becomes the resolver's single owner-facing error.
impl<AppleError, ModelError> From<OuraFileSourceRefusal>
    for solstone_core_import::ResolutionError<AppleError, ModelError>
{
    fn from(_: OuraFileSourceRefusal) -> Self {
        Self::OuraRequiresSync
    }
}

pub fn reserved_seam() -> Result<(), ImportSourcesError> {
    Err(ImportSourcesError::Unimplemented { module: "oura" })
}
