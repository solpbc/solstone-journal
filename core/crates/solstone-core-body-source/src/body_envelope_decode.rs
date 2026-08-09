// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::body_envelope_projection::{project_body_envelope, reject_unknown_top_level_keys};
use crate::body_envelope_scan::scan_body_envelope;
use crate::{BodyEnvelope, EnvelopeError};

/// Decodes exact canonical body-envelope JSONL bytes into checked values.
pub fn decode_body_envelope(input: &[u8]) -> Result<BodyEnvelope, EnvelopeError> {
    let scanned = scan_body_envelope(input)?;
    reject_unknown_top_level_keys(scanned.object())?;
    project_body_envelope(scanned.object())
}
