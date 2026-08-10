// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native, read-only screen-frame browser and extractor for `journal grab`.

mod encode;
mod error;
mod extract;
mod payload;
mod reader;
mod render;
mod request;
mod selection;
mod time;

pub use error::{GrabFailure, GrabFailureClass};
pub use request::{GrabDiagnostics, GrabOutput, GrabRequest, RecordingDiagnostics};

use std::path::Path;

/// Execute one fully-parsed grab request against `journal_root`.
pub fn run(
    journal_root: &Path,
    request: GrabRequest,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<GrabOutput, GrabFailure> {
    payload::run(journal_root, request, diagnostics)
}
