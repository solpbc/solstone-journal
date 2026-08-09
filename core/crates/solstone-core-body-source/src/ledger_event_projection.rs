// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::ledger_event::{
    DecodedLedgerEventParts, LEDGER_EVENT_SCHEMA, body_string_matches, day_matches,
    normalized_ref_matches, raw_ref_is_valid, row_schema_is_compatible, sequence_location,
};
use crate::whitespace::is_python_whitespace;
use crate::{
    BodyDay, BodyDigest, BodyEnvelope, BodyObject, BodySourceFamily, BodyString, BodyValue,
    BundleId, LedgerEventError, LedgerEventErrorCode, LedgerEventErrorField, LedgerSchema,
};

#[derive(Clone, Copy)]
pub(crate) enum LedgerEventTopLevelKey {
    Schema,
    BundleId,
    Sequence,
    RowSchema,
    Shard,
    Line,
    NormalizedRef,
    RowSha256,
    DedupeKey,
    SourceFamily,
    SourceRecordId,
    RecordType,
    StartTime,
    EndTime,
    Day,
    ValueHash,
    RawRef,
}

impl LedgerEventTopLevelKey {
    const ALL: [Self; 17] = [
        Self::Schema,
        Self::BundleId,
        Self::Sequence,
        Self::RowSchema,
        Self::Shard,
        Self::Line,
        Self::NormalizedRef,
        Self::RowSha256,
        Self::DedupeKey,
        Self::SourceFamily,
        Self::SourceRecordId,
        Self::RecordType,
        Self::StartTime,
        Self::EndTime,
        Self::Day,
        Self::ValueHash,
        Self::RawRef,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::BundleId => "bundle_id",
            Self::Sequence => "sequence",
            Self::RowSchema => "row_schema",
            Self::Shard => "shard",
            Self::Line => "line",
            Self::NormalizedRef => "normalized_ref",
            Self::RowSha256 => "row_sha256",
            Self::DedupeKey => "dedupe_key",
            Self::SourceFamily => "source_family",
            Self::SourceRecordId => "source_record_id",
            Self::RecordType => "record_type",
            Self::StartTime => "start_time",
            Self::EndTime => "end_time",
            Self::Day => "day",
            Self::ValueHash => "value_hash",
            Self::RawRef => "raw_ref",
        }
    }

    fn from_body_string(value: &BodyString) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|key| body_string_matches(value, key.as_str()))
    }
}

/// Rejects every top-level key outside the closed ledger-event vocabulary.
pub(crate) fn reject_unknown_top_level_keys(
    object: &BodyObject,
    bundle_id: &BundleId,
    expected_sequence: u64,
) -> Result<(), LedgerEventError> {
    if object
        .keys()
        .any(|key| LedgerEventTopLevelKey::from_body_string(key).is_none())
    {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::UnknownField,
            LedgerEventErrorField::Ledger,
        ));
    }
    Ok(())
}

/// Projects checked event fields in precedence order, not encoder key order.
pub(crate) fn project_body_ledger_event(
    object: &BodyObject,
    envelope: &BodyEnvelope,
    expected_sequence: u64,
) -> Result<(u64, u64, DecodedLedgerEventParts), LedgerEventError> {
    let bundle_id = envelope.bundle_id();
    let schema = required_string(
        object,
        LedgerEventTopLevelKey::Schema,
        bundle_id,
        expected_sequence,
    )?;
    if !body_string_matches(schema, LEDGER_EVENT_SCHEMA) {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::Schema,
        ));
    }

    let frame_bundle_id = BundleId::from_body_string(required_string(
        object,
        LedgerEventTopLevelKey::BundleId,
        bundle_id,
        expected_sequence,
    )?)
    .map_err(|_| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::BundleId,
        )
    })?;
    if &frame_bundle_id != bundle_id {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::BundleId,
        ));
    }

    let sequence = body_u64(
        required_value(
            object,
            LedgerEventTopLevelKey::Sequence,
            bundle_id,
            expected_sequence,
        )?,
        bundle_id,
        expected_sequence,
        LedgerEventErrorField::Sequence,
    )?;
    if sequence != expected_sequence {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidSequence,
            LedgerEventErrorField::Sequence,
        ));
    }

    let row_schema = LedgerSchema::from_body_string(required_string(
        object,
        LedgerEventTopLevelKey::RowSchema,
        bundle_id,
        expected_sequence,
    )?)
    .ok_or_else(|| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::RowSchema,
        )
    })?;
    if !row_schema_is_compatible(envelope, row_schema) {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::IncompatibleField,
            LedgerEventErrorField::RowSchema,
        ));
    }

    let (expected_shard_index, expected_line) = sequence_location(envelope, expected_sequence)
        .expect("a checked envelope covers every in-range row sequence");
    let expected_shard = &envelope.shards()[expected_shard_index];

    let shard = required_string(
        object,
        LedgerEventTopLevelKey::Shard,
        bundle_id,
        expected_sequence,
    )?
    .clone();
    if !body_string_matches(&shard, expected_shard.path()) {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Shard,
        ));
    }
    let line = body_u64(
        required_value(
            object,
            LedgerEventTopLevelKey::Line,
            bundle_id,
            expected_sequence,
        )?,
        bundle_id,
        expected_sequence,
        LedgerEventErrorField::Line,
    )?;
    if line != expected_line {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Line,
        ));
    }
    let normalized_ref = required_string(
        object,
        LedgerEventTopLevelKey::NormalizedRef,
        bundle_id,
        expected_sequence,
    )?
    .clone();
    if !normalized_ref_matches(envelope, expected_shard.path(), line, &normalized_ref) {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::NormalizedRef,
        ));
    }
    let row_sha256 = BodyDigest::from_body_string(required_string(
        object,
        LedgerEventTopLevelKey::RowSha256,
        bundle_id,
        expected_sequence,
    )?)
    .map_err(|_| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::RowSha256,
        )
    })?;
    let dedupe_key = required_string(
        object,
        LedgerEventTopLevelKey::DedupeKey,
        bundle_id,
        expected_sequence,
    )?
    .clone();
    BodyDigest::from_body_string(&dedupe_key).map_err(|_| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::DedupeKey,
        )
    })?;
    let source_family = BodySourceFamily::from_body_string(required_string(
        object,
        LedgerEventTopLevelKey::SourceFamily,
        bundle_id,
        expected_sequence,
    )?)
    .map_err(|_| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::SourceFamily,
        )
    })?;
    if source_family != envelope.source_family() {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::SourceFamily,
        ));
    }
    let source_record_id = nullable_string(
        object,
        LedgerEventTopLevelKey::SourceRecordId,
        bundle_id,
        expected_sequence,
    )?;
    let record_type = required_nonblank_string(
        object,
        LedgerEventTopLevelKey::RecordType,
        bundle_id,
        expected_sequence,
    )?;
    let start_time = required_nonblank_string(
        object,
        LedgerEventTopLevelKey::StartTime,
        bundle_id,
        expected_sequence,
    )?;
    let end_time = nullable_string(
        object,
        LedgerEventTopLevelKey::EndTime,
        bundle_id,
        expected_sequence,
    )?;
    let day = required_string(
        object,
        LedgerEventTopLevelKey::Day,
        bundle_id,
        expected_sequence,
    )?
    .clone();
    let checked_day = BodyDay::from_body_string(&day).map_err(|_| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::Day,
        )
    })?;
    if !day_matches(envelope, expected_shard.month(), &checked_day) {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Day,
        ));
    }
    let value_hash = BodyDigest::from_body_string(required_string(
        object,
        LedgerEventTopLevelKey::ValueHash,
        bundle_id,
        expected_sequence,
    )?)
    .map_err(|_| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::ValueHash,
        )
    })?;
    let raw_ref = nullable_string(
        object,
        LedgerEventTopLevelKey::RawRef,
        bundle_id,
        expected_sequence,
    )?;
    if raw_ref
        .as_ref()
        .is_some_and(|raw_ref| !raw_ref_is_valid(raw_ref, bundle_id))
    {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::RawRef,
        ));
    }

    Ok((
        sequence,
        line,
        DecodedLedgerEventParts {
            bundle_id: frame_bundle_id,
            row_schema,
            shard,
            normalized_ref,
            row_sha256,
            dedupe_key,
            source_family,
            source_record_id,
            record_type,
            start_time,
            end_time,
            day,
            value_hash,
            raw_ref,
        },
    ))
}

fn body_u64(
    value: &BodyValue,
    bundle_id: &BundleId,
    expected_sequence: u64,
    field: LedgerEventErrorField,
) -> Result<u64, LedgerEventError> {
    let BodyValue::Integer(value) = value else {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::WrongType,
            field,
        ));
    };
    if value.is_negative() {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            field,
        ));
    }
    value.digits().parse::<u64>().map_err(|_| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            field,
        )
    })
}

fn required_value<'a>(
    object: &'a BodyObject,
    key: LedgerEventTopLevelKey,
    bundle_id: &BundleId,
    expected_sequence: u64,
) -> Result<&'a BodyValue, LedgerEventError> {
    field_value(object, key).ok_or_else(|| {
        error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::MissingField,
            field(key),
        )
    })
}

fn required_string<'a>(
    object: &'a BodyObject,
    key: LedgerEventTopLevelKey,
    bundle_id: &BundleId,
    expected_sequence: u64,
) -> Result<&'a BodyString, LedgerEventError> {
    match field_value(object, key) {
        None => Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::MissingField,
            field(key),
        )),
        Some(BodyValue::String(value)) => Ok(value),
        Some(_) => Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::WrongType,
            field(key),
        )),
    }
}

fn required_nonblank_string(
    object: &BodyObject,
    key: LedgerEventTopLevelKey,
    bundle_id: &BundleId,
    expected_sequence: u64,
) -> Result<BodyString, LedgerEventError> {
    let value = required_string(object, key, bundle_id, expected_sequence)?;
    if value
        .code_points()
        .iter()
        .copied()
        .all(is_python_whitespace)
    {
        return Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::InvalidField,
            field(key),
        ));
    }
    Ok(value.clone())
}

fn nullable_string(
    object: &BodyObject,
    key: LedgerEventTopLevelKey,
    bundle_id: &BundleId,
    expected_sequence: u64,
) -> Result<Option<BodyString>, LedgerEventError> {
    match field_value(object, key) {
        None => Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::MissingField,
            field(key),
        )),
        Some(BodyValue::Null) => Ok(None),
        Some(BodyValue::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(error(
            bundle_id,
            expected_sequence,
            LedgerEventErrorCode::WrongType,
            field(key),
        )),
    }
}

fn field_value(object: &BodyObject, key: LedgerEventTopLevelKey) -> Option<&BodyValue> {
    let key = BodyString::from_code_points(key.as_str().bytes().map(u32::from).collect())
        .expect("ASCII ledger-event key is a valid body string");
    object.get(&key)
}

fn field(key: LedgerEventTopLevelKey) -> LedgerEventErrorField {
    match key {
        LedgerEventTopLevelKey::Schema => LedgerEventErrorField::Schema,
        LedgerEventTopLevelKey::BundleId => LedgerEventErrorField::BundleId,
        LedgerEventTopLevelKey::Sequence => LedgerEventErrorField::Sequence,
        LedgerEventTopLevelKey::RowSchema => LedgerEventErrorField::RowSchema,
        LedgerEventTopLevelKey::Shard => LedgerEventErrorField::Shard,
        LedgerEventTopLevelKey::Line => LedgerEventErrorField::Line,
        LedgerEventTopLevelKey::NormalizedRef => LedgerEventErrorField::NormalizedRef,
        LedgerEventTopLevelKey::RowSha256 => LedgerEventErrorField::RowSha256,
        LedgerEventTopLevelKey::DedupeKey => LedgerEventErrorField::DedupeKey,
        LedgerEventTopLevelKey::SourceFamily => LedgerEventErrorField::SourceFamily,
        LedgerEventTopLevelKey::SourceRecordId => LedgerEventErrorField::SourceRecordId,
        LedgerEventTopLevelKey::RecordType => LedgerEventErrorField::RecordType,
        LedgerEventTopLevelKey::StartTime => LedgerEventErrorField::StartTime,
        LedgerEventTopLevelKey::EndTime => LedgerEventErrorField::EndTime,
        LedgerEventTopLevelKey::Day => LedgerEventErrorField::Day,
        LedgerEventTopLevelKey::ValueHash => LedgerEventErrorField::ValueHash,
        LedgerEventTopLevelKey::RawRef => LedgerEventErrorField::RawRef,
    }
}

fn error(
    bundle_id: &BundleId,
    expected_sequence: u64,
    code: LedgerEventErrorCode,
    field: LedgerEventErrorField,
) -> LedgerEventError {
    LedgerEventError::new(Some(bundle_id.clone()), code, field, expected_sequence)
}
