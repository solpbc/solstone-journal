// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ICS source seam; the real signature is defined by a later wave.

use crate::ImportSourcesError;

pub fn reserved_seam() -> Result<(), ImportSourcesError> {
    Err(ImportSourcesError::Unimplemented { module: "ics" })
}
