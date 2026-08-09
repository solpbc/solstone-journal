// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};

use crate::candidate;
use crate::ledger_event::sequence_location;
use crate::{
    BodyDigest, BodyEnvelope, BodyLedgerEvent, BodyRowEventError, BodyRowEventErrorKind,
    Coordinate, PresentationRow,
};

const MAX_ROW_FRAME_BYTES: usize = 1_048_576;

/// Validates one normalized-row JSONL frame against its ledger event.
pub fn validate_body_row_event(
    envelope: &BodyEnvelope,
    row_frame: &[u8],
    event: &BodyLedgerEvent,
) -> Result<BodyLedgerEvent, BodyRowEventError> {
    if row_frame.len() > MAX_ROW_FRAME_BYTES {
        return Err(row_event_error(event, BodyRowEventErrorKind::Oversized));
    }

    if row_frame.is_empty()
        || !row_frame.ends_with(b"\n")
        || row_frame.len() == 1
        || row_frame[..row_frame.len() - 1].contains(&b'\n')
    {
        return Err(row_event_error(
            event,
            BodyRowEventErrorKind::InvalidFraming,
        ));
    }

    let row_digest = format!("sha256:{:x}", Sha256::digest(row_frame));
    let row_sha256 = BodyDigest::from_bytes(row_digest.as_bytes())
        .expect("SHA-256 output is always a valid body digest");
    if &row_sha256 != event.row_sha256() {
        return Err(row_event_error(
            event,
            BodyRowEventErrorKind::RowDigestMismatch,
        ));
    }

    let value = crate::parser::parse(&row_frame[..row_frame.len() - 1])
        .map_err(|parse_error| row_event_error(event, BodyRowEventErrorKind::Parse(parse_error)))?;
    let coordinate = Coordinate::new(event.bundle_id().as_str(), event.shard(), event.line());
    let row = PresentationRow::new(&value, &coordinate).map_err(|candidate_error| {
        row_event_error(event, BodyRowEventErrorKind::Candidate(candidate_error))
    })?;
    let candidate = candidate::project(&row, coordinate).map_err(|candidate_error| {
        row_event_error(event, BodyRowEventErrorKind::Candidate(candidate_error))
    })?;

    let shard_index = match sequence_location(envelope, event.sequence()) {
        Some((index, _)) => index as u64,
        None => 0,
    };
    let reconstructed = BodyLedgerEvent::new(
        envelope,
        event.sequence(),
        shard_index,
        event.line(),
        event.row_sha256().clone(),
        event.value_hash().clone(),
        &candidate,
    )
    .map_err(|event_error| row_event_error(event, BodyRowEventErrorKind::Event(event_error)))?;

    if reconstructed != *event {
        return Err(row_event_error(event, BodyRowEventErrorKind::EventMismatch));
    }

    Ok(reconstructed)
}

fn row_event_error(event: &BodyLedgerEvent, kind: BodyRowEventErrorKind) -> BodyRowEventError {
    BodyRowEventError::new(event.bundle_id().clone(), event.sequence(), kind)
}
