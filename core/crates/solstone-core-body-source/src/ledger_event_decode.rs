// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decodes ledger frames into frame-native typed parts rather than synthesizing a
//! `LedgerCandidate`, because the wire format models `BodyLedgerEvent` directly.
//! Envelope and reference validation is shared with `BodyLedgerEvent::new` through
//! crate-private helpers, leaving one validation grammar rather than two.

use crate::ledger_event::sequence_location;
use crate::ledger_event_projection::{project_body_ledger_event, reject_unknown_top_level_keys};
use crate::ledger_event_scan::scan_body_ledger_event;
use crate::{
    BodyEnvelope, BodyLedgerEvent, LedgerEventError, LedgerEventErrorCode, LedgerEventErrorField,
};

/// Decodes one exact canonical body-ledger event JSONL frame into checked values.
pub fn decode_body_ledger_event(
    frame: &[u8],
    envelope: &BodyEnvelope,
    expected_sequence: u64,
) -> Result<BodyLedgerEvent, LedgerEventError> {
    if expected_sequence == 0 || expected_sequence > envelope.row_count() {
        return Err(LedgerEventError::new(
            Some(envelope.bundle_id().clone()),
            LedgerEventErrorCode::InvalidSequence,
            LedgerEventErrorField::Sequence,
            expected_sequence,
        ));
    }

    let scanned = scan_body_ledger_event(frame, envelope.bundle_id(), expected_sequence)?;
    reject_unknown_top_level_keys(scanned.object(), envelope.bundle_id(), expected_sequence)?;
    let (sequence, line, parts) =
        project_body_ledger_event(scanned.object(), envelope, expected_sequence)?;
    let (shard_index, _) = sequence_location(envelope, expected_sequence)
        .expect("a checked envelope covers every in-range row sequence");
    BodyLedgerEvent::from_decoded_parts(envelope, sequence, shard_index as u64, line, parts)
}
