// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    BodyDay, BodyDigest, BodyEnvelope, BodySourceFamily, BodyString, BundleId, FieldState,
    LedgerCandidate, LedgerEventError, LedgerEventErrorCode, LedgerEventErrorField, LedgerSchema,
};

const LEDGER_EVENT_SCHEMA: &str = "solstone.body.ledger_event.v1";

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
        let bundle = Some(envelope.bundle_id().clone());
        let err = |code, field| LedgerEventError::new(bundle.clone(), code, field, sequence);

        if sequence == 0 || sequence > envelope.row_count() {
            return Err(err(
                LedgerEventErrorCode::InvalidSequence,
                LedgerEventErrorField::Sequence,
            ));
        }
        let Some(shard) = usize::try_from(shard_index)
            .ok()
            .and_then(|index| envelope.shards().get(index))
        else {
            return Err(err(
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::Shard,
            ));
        };
        if line == 0 || line > shard.rows() {
            return Err(err(
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::Line,
            ));
        }
        if candidate.schema() == LedgerSchema::NormalizedV1
            || candidate.schema().expected_family() != envelope.source_family().as_str()
        {
            return Err(err(
                LedgerEventErrorCode::IncompatibleField,
                LedgerEventErrorField::RowSchema,
            ));
        }
        if !matches!(
            candidate.import_id(),
            FieldState::Present(value) if value == &envelope.bundle_id().to_body_string()
        ) {
            return Err(err(
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::BundleId,
            ));
        }
        if !matches!(
            candidate.month(),
            FieldState::Present(value) if value == &shard.month().to_body_string()
        ) {
            return Err(err(
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::Shard,
            ));
        }
        let day = BodyDay::from_body_string(candidate.day()).map_err(|_| {
            err(
                LedgerEventErrorCode::InvalidField,
                LedgerEventErrorField::Day,
            )
        })?;
        if envelope.days().binary_search(&day).is_err() || day.month() != *shard.month() {
            return Err(err(
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
        let FieldState::Present(normalized_ref) = candidate.normalized_ref() else {
            return Err(err(
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::NormalizedRef,
            ));
        };
        if !body_string_matches(normalized_ref, &expected_normalized_ref) {
            return Err(err(
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::NormalizedRef,
            ));
        }
        let dedupe_key = BodyDigest::from_body_string(candidate.dedupe_key()).map_err(|_| {
            err(
                LedgerEventErrorCode::InvalidField,
                LedgerEventErrorField::DedupeKey,
            )
        })?;
        if let FieldState::Present(raw_ref) = candidate.raw_ref()
            && !raw_ref_is_valid(raw_ref, envelope.bundle_id())
        {
            return Err(err(
                LedgerEventErrorCode::InvalidField,
                LedgerEventErrorField::RawRef,
            ));
        }

        Ok(Self {
            schema: LEDGER_EVENT_SCHEMA,
            bundle_id: envelope.bundle_id().clone(),
            sequence,
            row_schema: candidate.schema(),
            shard: shard.path().to_owned(),
            line,
            normalized_ref: normalized_ref.clone(),
            row_sha256,
            dedupe_key,
            source_family: envelope.source_family(),
            source_record_id: present_or_none(candidate.source_record_id()),
            record_type: candidate.record_type().clone(),
            start_time: candidate.start_date().clone(),
            end_time: present_or_none(candidate.end_date()),
            day,
            value_hash,
            raw_ref: present_or_none(candidate.raw_ref()),
        })
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

fn body_string_matches(value: &BodyString, literal: &str) -> bool {
    value
        .code_points()
        .iter()
        .copied()
        .eq(literal.bytes().map(u32::from))
}

fn present_or_none(state: &FieldState<BodyString>) -> Option<BodyString> {
    match state {
        FieldState::Absent | FieldState::Null => None,
        FieldState::Present(value) => Some(value.clone()),
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
