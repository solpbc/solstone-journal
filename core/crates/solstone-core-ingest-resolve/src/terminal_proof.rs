// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::File;
use std::io::Read;

use serde_json::Value;
use solstone_core_processing_record::{TerminalProofOutcome, evaluate_terminal_proof, vocab};
use solstone_core_segment::{ContentName, SegmentDir, TerminalProofVerifier};

/// Read-only terminal-processing verifier bound to one resolved segment.
pub struct SegmentTerminalProof<'a> {
    segment: &'a SegmentDir,
}

impl<'a> SegmentTerminalProof<'a> {
    /// Bind terminal-proof checks to `segment` without creating any path.
    pub fn new(segment: &'a SegmentDir) -> Self {
        Self { segment }
    }
}

impl TerminalProofVerifier for SegmentTerminalProof<'_> {
    fn has_terminal_proof(&self, name: &ContentName, size: u64) -> bool {
        let Some(expected_handler) = expected_handler(name.as_str()) else {
            return false;
        };
        let recorded_path = self.segment.path().join(name.as_str());
        let sidecar_path = recorded_path.with_extension("jsonl");
        let mut sidecar = match File::open(&sidecar_path) {
            Ok(file) => file,
            Err(_) => return false,
        };
        let mut first_window = Vec::with_capacity(vocab::MAX_FIRST_ROW_BYTES);
        if sidecar
            .by_ref()
            .take(vocab::MAX_FIRST_ROW_BYTES as u64)
            .read_to_end(&mut first_window)
            .is_err()
        {
            return false;
        }
        let Some(newline) = first_window.iter().position(|byte| *byte == b'\n') else {
            return false;
        };
        let Ok(first_line) = std::str::from_utf8(&first_window[..newline]) else {
            return false;
        };
        let Ok(Value::Object(row)) = serde_json::from_str::<Value>(first_line) else {
            return false;
        };
        matches!(
            evaluate_terminal_proof(row.get("_solstone_processing"), expected_handler, size,),
            TerminalProofOutcome::Held
        )
    }
}

pub(crate) fn is_media_name(name: &str) -> bool {
    expected_handler(name).is_some()
}

/// Delegates to the shared handler map.
///
/// ⚠ This was a second private copy of that map. It is kept as a thin adapter only
/// because callers here pass a whole filename while the shared map takes an
/// extension.
fn expected_handler(name: &str) -> Option<&'static str> {
    solstone_core_processing_record::expected_handler(name.rsplit_once('.')?.1)
}
