// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    BodyDay, BodyDigest, BodyEnvelope, BodySourceFamily, BodyString, BundleId, FieldState,
    LedgerCandidate, LedgerEventError, LedgerEventErrorCode, LedgerEventErrorField, LedgerSchema,
};

pub(crate) const LEDGER_EVENT_SCHEMA: &str = "solstone.body.ledger_event.v1";

/// Typed event-frame values which do not have a normalized-row counterpart.
pub(crate) struct DecodedLedgerEventParts {
    pub(crate) bundle_id: BundleId,
    pub(crate) row_schema: LedgerSchema,
    pub(crate) shard: BodyString,
    pub(crate) normalized_ref: BodyString,
    pub(crate) row_sha256: BodyDigest,
    pub(crate) dedupe_key: BodyString,
    pub(crate) source_family: BodySourceFamily,
    pub(crate) source_record_id: Option<BodyString>,
    pub(crate) record_type: BodyString,
    pub(crate) start_time: BodyString,
    pub(crate) end_time: Option<BodyString>,
    pub(crate) day: BodyString,
    pub(crate) value_hash: BodyDigest,
    pub(crate) raw_ref: Option<BodyString>,
}

struct LedgerEventInputs<'a> {
    row_schema: LedgerSchema,
    wire_shard: Option<&'a BodyString>,
    import_id: Option<&'a BodyString>,
    month: Option<&'a BodyString>,
    day: &'a BodyString,
    normalized_ref: Option<&'a BodyString>,
    dedupe_key: &'a BodyString,
    raw_ref: Option<&'a BodyString>,
    source_record_id: Option<&'a BodyString>,
    record_type: &'a BodyString,
    start_time: &'a BodyString,
    end_time: Option<&'a BodyString>,
}

/// Checked native values for one body-ledger event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyLedgerEvent {
    schema: &'static str,
    bundle_id: BundleId,
    sequence: u64,
    row_schema: LedgerSchema,
    shard: String,
    line: u64,
    normalized_ref: BodyString,
    row_sha256: BodyDigest,
    dedupe_key: BodyDigest,
    source_family: BodySourceFamily,
    source_record_id: Option<BodyString>,
    record_type: BodyString,
    start_time: BodyString,
    end_time: Option<BodyString>,
    day: BodyDay,
    value_hash: BodyDigest,
    raw_ref: Option<BodyString>,
}

impl BodyLedgerEvent {
    /// Binds one projected normalized row to a checked body-envelope ledger location.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        envelope: &BodyEnvelope,
        sequence: u64,
        shard_index: u64,
        line: u64,
        row_sha256: BodyDigest,
        value_hash: BodyDigest,
        candidate: &LedgerCandidate,
    ) -> Result<BodyLedgerEvent, LedgerEventError> {
        build_ledger_event(
            envelope,
            sequence,
            shard_index,
            line,
            row_sha256,
            value_hash,
            LedgerEventInputs {
                row_schema: candidate.schema(),
                wire_shard: None,
                import_id: present(candidate.import_id()),
                month: present(candidate.month()),
                day: candidate.day(),
                normalized_ref: present(candidate.normalized_ref()),
                dedupe_key: candidate.dedupe_key(),
                raw_ref: present(candidate.raw_ref()),
                source_record_id: present(candidate.source_record_id()),
                record_type: candidate.record_type(),
                start_time: candidate.start_date(),
                end_time: present(candidate.end_date()),
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_decoded_parts(
        envelope: &BodyEnvelope,
        sequence: u64,
        shard_index: u64,
        line: u64,
        parts: DecodedLedgerEventParts,
    ) -> Result<BodyLedgerEvent, LedgerEventError> {
        let import_id = parts.bundle_id.to_body_string();
        // Ledger frames carry no month; this derived value trivially satisfies stage 6.
        let month = sequence_location(envelope, sequence)
            .and_then(|(index, _)| envelope.shards().get(index))
            .map(|shard| shard.month().to_body_string());
        let event = build_ledger_event(
            envelope,
            sequence,
            shard_index,
            line,
            parts.row_sha256,
            parts.value_hash,
            LedgerEventInputs {
                row_schema: parts.row_schema,
                wire_shard: Some(&parts.shard),
                import_id: Some(&import_id),
                month: month.as_ref(),
                day: &parts.day,
                normalized_ref: Some(&parts.normalized_ref),
                dedupe_key: &parts.dedupe_key,
                raw_ref: parts.raw_ref.as_ref(),
                source_record_id: parts.source_record_id.as_ref(),
                record_type: &parts.record_type,
                start_time: &parts.start_time,
                end_time: parts.end_time.as_ref(),
            },
        )?;
        if parts.source_family != envelope.source_family() {
            return Err(ledger_error(
                envelope,
                sequence,
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::SourceFamily,
            ));
        }
        Ok(event)
    }

    pub fn schema(&self) -> &str {
        self.schema
    }
    pub fn bundle_id(&self) -> &BundleId {
        &self.bundle_id
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn row_schema(&self) -> LedgerSchema {
        self.row_schema
    }
    pub fn shard(&self) -> &str {
        &self.shard
    }
    pub fn line(&self) -> u64 {
        self.line
    }
    pub fn normalized_ref(&self) -> &BodyString {
        &self.normalized_ref
    }
    pub fn row_sha256(&self) -> &BodyDigest {
        &self.row_sha256
    }
    pub fn dedupe_key(&self) -> &BodyDigest {
        &self.dedupe_key
    }
    pub fn source_family(&self) -> BodySourceFamily {
        self.source_family
    }
    pub fn source_record_id(&self) -> Option<&BodyString> {
        self.source_record_id.as_ref()
    }
    pub fn record_type(&self) -> &BodyString {
        &self.record_type
    }
    pub fn start_time(&self) -> &BodyString {
        &self.start_time
    }
    pub fn end_time(&self) -> Option<&BodyString> {
        self.end_time.as_ref()
    }
    pub fn day(&self) -> &BodyDay {
        &self.day
    }
    pub fn value_hash(&self) -> &BodyDigest {
        &self.value_hash
    }
    pub fn raw_ref(&self) -> Option<&BodyString> {
        self.raw_ref.as_ref()
    }
}

pub(crate) fn sequence_location(envelope: &BodyEnvelope, sequence: u64) -> Option<(usize, u64)> {
    let mut remaining = sequence;
    for (index, shard) in envelope.shards().iter().enumerate() {
        if remaining <= shard.rows() {
            return Some((index, remaining));
        }
        remaining -= shard.rows();
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn build_ledger_event(
    envelope: &BodyEnvelope,
    sequence: u64,
    shard_index: u64,
    line: u64,
    row_sha256: BodyDigest,
    value_hash: BodyDigest,
    inputs: LedgerEventInputs<'_>,
) -> Result<BodyLedgerEvent, LedgerEventError> {
    if sequence == 0 || sequence > envelope.row_count() {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::InvalidSequence,
            LedgerEventErrorField::Sequence,
        ));
    }
    let (expected_shard_index, expected_line) = sequence_location(envelope, sequence)
        .expect("a checked envelope covers every in-range row sequence");
    if usize::try_from(shard_index).ok() != Some(expected_shard_index) {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Shard,
        ));
    }
    let shard = &envelope.shards()[expected_shard_index];
    if inputs
        .wire_shard
        .is_some_and(|wire_shard| !body_string_matches(wire_shard, shard.path()))
    {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Shard,
        ));
    }
    if line != expected_line {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Line,
        ));
    }
    if inputs.row_schema == LedgerSchema::NormalizedV1
        || inputs.row_schema.expected_family() != envelope.source_family().as_str()
    {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::IncompatibleField,
            LedgerEventErrorField::RowSchema,
        ));
    }
    if inputs.import_id != Some(&envelope.bundle_id().to_body_string()) {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::BundleId,
        ));
    }
    if inputs.month != Some(&shard.month().to_body_string()) {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Shard,
        ));
    }
    let day = BodyDay::from_body_string(inputs.day).map_err(|_| {
        ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::Day,
        )
    })?;
    if envelope.days().binary_search(&day).is_err() || day.month() != *shard.month() {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Day,
        ));
    }
    let expected_normalized_ref = format!(
        "imports/{}/{}#L{}",
        envelope.bundle_id().as_str(),
        shard.path(),
        line
    );
    let Some(normalized_ref) = inputs.normalized_ref else {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::NormalizedRef,
        ));
    };
    if !body_string_matches(normalized_ref, &expected_normalized_ref) {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::NormalizedRef,
        ));
    }
    let dedupe_key = BodyDigest::from_body_string(inputs.dedupe_key).map_err(|_| {
        ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::DedupeKey,
        )
    })?;
    if inputs
        .raw_ref
        .is_some_and(|raw_ref| !raw_ref_is_valid(raw_ref, envelope.bundle_id()))
    {
        return Err(ledger_error(
            envelope,
            sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::RawRef,
        ));
    }

    Ok(BodyLedgerEvent {
        schema: LEDGER_EVENT_SCHEMA,
        bundle_id: envelope.bundle_id().clone(),
        sequence,
        row_schema: inputs.row_schema,
        shard: shard.path().to_owned(),
        line,
        normalized_ref: normalized_ref.clone(),
        row_sha256,
        dedupe_key,
        source_family: envelope.source_family(),
        source_record_id: inputs.source_record_id.cloned(),
        record_type: inputs.record_type.clone(),
        start_time: inputs.start_time.clone(),
        end_time: inputs.end_time.cloned(),
        day,
        value_hash,
        raw_ref: inputs.raw_ref.cloned(),
    })
}

fn ledger_error(
    envelope: &BodyEnvelope,
    sequence: u64,
    code: LedgerEventErrorCode,
    field: LedgerEventErrorField,
) -> LedgerEventError {
    LedgerEventError::new(Some(envelope.bundle_id().clone()), code, field, sequence)
}

fn body_string_matches(value: &BodyString, literal: &str) -> bool {
    value
        .code_points()
        .iter()
        .copied()
        .eq(literal.bytes().map(u32::from))
}

fn present(state: &FieldState<BodyString>) -> Option<&BodyString> {
    match state {
        FieldState::Absent | FieldState::Null => None,
        FieldState::Present(value) => Some(value),
    }
}

fn raw_ref_is_valid(raw_ref: &BodyString, bundle_id: &BundleId) -> bool {
    let prefix: Vec<u32> = format!("imports/{}/raw/", bundle_id.as_str())
        .bytes()
        .map(u32::from)
        .collect();
    let code_points = raw_ref.code_points();
    if !code_points.starts_with(&prefix) {
        return false;
    }
    let remainder = &code_points[prefix.len()..];
    !remainder.is_empty()
        && remainder
            .split(|code_point| *code_point == u32::from(b'/'))
            .all(|component| {
                !component.is_empty()
                    && component != [u32::from(b'.')]
                    && component != [u32::from(b'.'), u32::from(b'.')]
                    && !component.contains(&0)
            })
}
