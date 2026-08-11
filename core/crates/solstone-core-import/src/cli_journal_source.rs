// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Journal-source CLI seam; the real signature is defined by a later wave.

use crate::ImportError;

pub fn reserved_seam() -> Result<(), ImportError> {
    Err(ImportError::Unimplemented {
        module: "cli_journal_source",
    })
}
