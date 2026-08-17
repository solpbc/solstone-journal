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

#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub mod test_hooks {
    use std::path::Path;

    use crate::error::GrabFailure;
    use crate::extract;

    // rustc E0364/E0365: a public module cannot `pub use` `pub(crate)` items.
    // extract::decode_frames / extract::RgbFrame stay crate-private.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct RgbFrame {
        pub width: u32,
        pub height: u32,
        pub pixels: Vec<u8>,
    }

    pub fn decode_frames(path: &Path, ids: &[i64]) -> Result<Vec<Option<RgbFrame>>, GrabFailure> {
        Ok(extract::decode_frames(path, ids)?
            .into_iter()
            .map(|frame| {
                frame.map(|frame| RgbFrame {
                    width: frame.width,
                    height: frame.height,
                    pixels: frame.pixels,
                })
            })
            .collect())
    }
}

use std::path::Path;

/// Execute one fully-parsed grab request against `journal_root`.
pub fn run(
    journal_root: &Path,
    request: GrabRequest,
    diagnostics: &mut dyn GrabDiagnostics,
) -> Result<GrabOutput, GrabFailure> {
    payload::run(journal_root, request, diagnostics)
}
