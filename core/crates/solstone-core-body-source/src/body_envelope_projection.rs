// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::apple_summary_plan::APPLE_SUMMARY_SCHEMA;
use crate::envelope_ledger::LEDGER_PATH;
use crate::manifest_binding::BODY_SOURCE_SCHEMA_VALUE;
use crate::{
    AppleSummaryPlan, BodyDay, BodyDigest, BodyEnvelope, BodyMonth, BodyObject, BodyRawRetention,
    BodySourceFamily, BodySourceHash, BodyString, BodyValue, BundleId, EnvelopeError,
    EnvelopeErrorCode, EnvelopeErrorField, EnvelopeLedger, EnvelopeShard,
};

const SHARD_KEYS: [&str; 4] = ["path", "bytes", "rows", "sha256"];
const LEDGER_KEYS: [&str; 4] = ["path", "bytes", "events", "sha256"];
const SUMMARY_PLAN_KEYS: [&str; 2] = ["schema", "days"];
const NORMALIZED_PREFIX: &[u8] = b"normalized/";
const JSONL_SUFFIX: &[u8] = b".jsonl";

#[derive(Clone, Copy)]
pub(crate) enum EnvelopeTopLevelKey {
    Schema,
    BundleId,
    SourceFamily,
    SourceHash,
    RawRetention,
    RowCount,
    Days,
    Shards,
    Ledger,
    SummaryPlan,
}

impl EnvelopeTopLevelKey {
    const ALL: [Self; 10] = [
        Self::Schema,
        Self::BundleId,
        Self::SourceFamily,
        Self::SourceHash,
        Self::RawRetention,
        Self::RowCount,
        Self::Days,
        Self::Shards,
        Self::Ledger,
        Self::SummaryPlan,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::BundleId => "bundle_id",
            Self::SourceFamily => "source_family",
            Self::SourceHash => "source_hash",
            Self::RawRetention => "raw_retention",
            Self::RowCount => "row_count",
            Self::Days => "days",
            Self::Shards => "shards",
            Self::Ledger => "ledger",
            Self::SummaryPlan => "summary_plan",
        }
    }

    fn from_body_string(value: &BodyString) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|key| body_string_matches(value, key.as_str()))
    }
}

/// Rejects every top-level key outside the closed envelope vocabulary.
pub(crate) fn reject_unknown_top_level_keys(object: &BodyObject) -> Result<(), EnvelopeError> {
    if object
        .keys()
        .any(|key| EnvelopeTopLevelKey::from_body_string(key).is_none())
    {
        return Err(error(
            None,
            EnvelopeErrorCode::UnknownField,
            EnvelopeErrorField::Envelope,
            None,
        ));
    }
    Ok(())
}

/// Projects scanned envelope fields into their checked aggregate.
pub(crate) fn project_body_envelope(object: &BodyObject) -> Result<BodyEnvelope, EnvelopeError> {
    let schema = required_string(
        object,
        EnvelopeTopLevelKey::Schema.as_str(),
        None,
        EnvelopeErrorField::Schema,
        None,
    )?;
    if !body_string_matches(schema, BODY_SOURCE_SCHEMA_VALUE) {
        return Err(error(
            None,
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::Schema,
            None,
        ));
    }

    let bundle_id = BundleId::from_body_string(required_string(
        object,
        EnvelopeTopLevelKey::BundleId.as_str(),
        None,
        EnvelopeErrorField::BundleId,
        None,
    )?)
    .map_err(|_| {
        error(
            None,
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::BundleId,
            None,
        )
    })?;

    let source_family = BodySourceFamily::from_body_string(required_string(
        object,
        EnvelopeTopLevelKey::SourceFamily.as_str(),
        Some(&bundle_id),
        EnvelopeErrorField::SourceFamily,
        None,
    )?)
    .map_err(|_| {
        error(
            Some(&bundle_id),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::SourceFamily,
            None,
        )
    })?;

    let source_hash = BodySourceHash::from_body_string_for_family(
        required_string(
            object,
            EnvelopeTopLevelKey::SourceHash.as_str(),
            Some(&bundle_id),
            EnvelopeErrorField::SourceHash,
            None,
        )?,
        &source_family,
    )
    .map_err(|_| {
        error(
            Some(&bundle_id),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::SourceHash,
            None,
        )
    })?;

    let raw_retention = BodyRawRetention::from_body_string(required_string(
        object,
        EnvelopeTopLevelKey::RawRetention.as_str(),
        Some(&bundle_id),
        EnvelopeErrorField::RawRetention,
        None,
    )?)
    .map_err(|_| {
        error(
            Some(&bundle_id),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::RawRetention,
            None,
        )
    })?;

    let row_count = body_u64(
        required_value(
            object,
            EnvelopeTopLevelKey::RowCount.as_str(),
            &bundle_id,
            EnvelopeErrorField::RowCount,
            None,
        )?,
        &bundle_id,
        EnvelopeErrorField::RowCount,
        None,
    )?;

    let days = project_days(
        required_value(
            object,
            EnvelopeTopLevelKey::Days.as_str(),
            &bundle_id,
            EnvelopeErrorField::Days,
            None,
        )?,
        &bundle_id,
        EnvelopeErrorField::Days,
    )?;

    let shards = project_shards(
        required_value(
            object,
            EnvelopeTopLevelKey::Shards.as_str(),
            &bundle_id,
            EnvelopeErrorField::Shards,
            None,
        )?,
        &bundle_id,
    )?;

    let ledger = project_ledger(
        required_value(
            object,
            EnvelopeTopLevelKey::Ledger.as_str(),
            &bundle_id,
            EnvelopeErrorField::Ledger,
            None,
        )?,
        &bundle_id,
    )?;

    let summary_plan = project_summary_plan(
        required_value(
            object,
            EnvelopeTopLevelKey::SummaryPlan.as_str(),
            &bundle_id,
            EnvelopeErrorField::SummaryPlan,
            None,
        )?,
        &bundle_id,
    )?;

    BodyEnvelope::new(
        bundle_id,
        source_family,
        source_hash,
        raw_retention,
        row_count,
        days,
        shards,
        ledger,
        summary_plan,
    )
}

fn project_shards(
    value: &BodyValue,
    bundle: &BundleId,
) -> Result<Vec<EnvelopeShard>, EnvelopeError> {
    let BodyValue::Array(values) = value else {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::WrongType,
            EnvelopeErrorField::Shards,
            None,
        ));
    };

    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let index = index as u64;
            let BodyValue::Object(object) = value else {
                return Err(error(
                    Some(bundle),
                    EnvelopeErrorCode::WrongType,
                    EnvelopeErrorField::Shards,
                    Some(index),
                ));
            };
            reject_unknown_keys(
                object,
                &SHARD_KEYS,
                bundle,
                EnvelopeErrorField::Shards,
                Some(index),
            )?;
            project_shard(object, bundle, index)
        })
        .collect()
}

fn project_shard(
    object: &BodyObject,
    bundle: &BundleId,
    index: u64,
) -> Result<EnvelopeShard, EnvelopeError> {
    let month = month_from_shard_path(
        required_string(
            object,
            "path",
            Some(bundle),
            EnvelopeErrorField::ShardPath,
            Some(index),
        )?,
        bundle,
        index,
    )?;
    let bytes = body_u64(
        required_value(
            object,
            "bytes",
            bundle,
            EnvelopeErrorField::ShardBytes,
            Some(index),
        )?,
        bundle,
        EnvelopeErrorField::ShardBytes,
        Some(index),
    )?;
    let rows = body_u64(
        required_value(
            object,
            "rows",
            bundle,
            EnvelopeErrorField::ShardRows,
            Some(index),
        )?,
        bundle,
        EnvelopeErrorField::ShardRows,
        Some(index),
    )?;
    let sha256 = BodyDigest::from_body_string(required_string(
        object,
        "sha256",
        Some(bundle),
        EnvelopeErrorField::ShardSha256,
        Some(index),
    )?)
    .map_err(|_| {
        error(
            Some(bundle),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::ShardSha256,
            Some(index),
        )
    })?;
    EnvelopeShard::new(bundle, index, month, bytes, rows, sha256)
}

fn project_ledger(value: &BodyValue, bundle: &BundleId) -> Result<EnvelopeLedger, EnvelopeError> {
    let BodyValue::Object(object) = value else {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::WrongType,
            EnvelopeErrorField::Ledger,
            None,
        ));
    };
    reject_unknown_keys(
        object,
        &LEDGER_KEYS,
        bundle,
        EnvelopeErrorField::Ledger,
        None,
    )?;

    let path = required_string(
        object,
        "path",
        Some(bundle),
        EnvelopeErrorField::LedgerPath,
        None,
    )?;
    if !body_string_matches(path, LEDGER_PATH) {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::LedgerPath,
            None,
        ));
    }
    let bytes = body_u64(
        required_value(
            object,
            "bytes",
            bundle,
            EnvelopeErrorField::LedgerBytes,
            None,
        )?,
        bundle,
        EnvelopeErrorField::LedgerBytes,
        None,
    )?;
    let events = body_u64(
        required_value(
            object,
            "events",
            bundle,
            EnvelopeErrorField::LedgerEvents,
            None,
        )?,
        bundle,
        EnvelopeErrorField::LedgerEvents,
        None,
    )?;
    let sha256 = BodyDigest::from_body_string(required_string(
        object,
        "sha256",
        Some(bundle),
        EnvelopeErrorField::LedgerSha256,
        None,
    )?)
    .map_err(|_| {
        error(
            Some(bundle),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::LedgerSha256,
            None,
        )
    })?;
    EnvelopeLedger::new(bundle, bytes, events, sha256)
}

fn project_summary_plan(
    value: &BodyValue,
    bundle: &BundleId,
) -> Result<Option<AppleSummaryPlan>, EnvelopeError> {
    if matches!(value, BodyValue::Null) {
        return Ok(None);
    }
    let BodyValue::Object(object) = value else {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::WrongType,
            EnvelopeErrorField::SummaryPlan,
            None,
        ));
    };
    reject_unknown_keys(
        object,
        &SUMMARY_PLAN_KEYS,
        bundle,
        EnvelopeErrorField::SummaryPlan,
        None,
    )?;

    let schema = required_string(
        object,
        "schema",
        Some(bundle),
        EnvelopeErrorField::SummarySchema,
        None,
    )?;
    if !body_string_matches(schema, APPLE_SUMMARY_SCHEMA) {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::SummarySchema,
            None,
        ));
    }
    let days = project_days(
        required_value(
            object,
            "days",
            bundle,
            EnvelopeErrorField::SummaryDays,
            None,
        )?,
        bundle,
        EnvelopeErrorField::SummaryDays,
    )?;
    AppleSummaryPlan::new(bundle, days).map(Some)
}

fn project_days(
    value: &BodyValue,
    bundle: &BundleId,
    field: EnvelopeErrorField,
) -> Result<Vec<BodyDay>, EnvelopeError> {
    let BodyValue::Array(values) = value else {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::WrongType,
            field,
            None,
        ));
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let index = index as u64;
            let BodyValue::String(value) = value else {
                return Err(error(
                    Some(bundle),
                    EnvelopeErrorCode::WrongType,
                    field,
                    Some(index),
                ));
            };
            BodyDay::from_body_string(value).map_err(|_| {
                error(
                    Some(bundle),
                    EnvelopeErrorCode::InvalidField,
                    field,
                    Some(index),
                )
            })
        })
        .collect()
}

/// Converts one already-present JSON integer into a bounded `u64`.
pub(crate) fn body_u64(
    value: &BodyValue,
    bundle: &BundleId,
    field: EnvelopeErrorField,
    index: Option<u64>,
) -> Result<u64, EnvelopeError> {
    let BodyValue::Integer(value) = value else {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::WrongType,
            field,
            index,
        ));
    };
    if value.is_negative() {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::InvalidField,
            field,
            index,
        ));
    }
    value
        .digits()
        .parse::<u64>()
        .map_err(|_| error(Some(bundle), EnvelopeErrorCode::InvalidField, field, index))
}

fn month_from_shard_path(
    value: &BodyString,
    bundle: &BundleId,
    index: u64,
) -> Result<BodyMonth, EnvelopeError> {
    let mut bytes = Vec::with_capacity(value.code_points().len());
    for code_point in value.code_points() {
        let Ok(byte) = u8::try_from(*code_point) else {
            return Err(invalid_shard_path(bundle, index));
        };
        if !byte.is_ascii() {
            return Err(invalid_shard_path(bundle, index));
        }
        bytes.push(byte);
    }
    let month_end = NORMALIZED_PREFIX.len() + 7;
    if bytes.len() != month_end + JSONL_SUFFIX.len()
        || !bytes.starts_with(NORMALIZED_PREFIX)
        || !bytes.ends_with(JSONL_SUFFIX)
    {
        return Err(invalid_shard_path(bundle, index));
    }
    BodyMonth::from_bytes(&bytes[NORMALIZED_PREFIX.len()..month_end])
        .map_err(|_| invalid_shard_path(bundle, index))
}

fn invalid_shard_path(bundle: &BundleId, index: u64) -> EnvelopeError {
    error(
        Some(bundle),
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardPath,
        Some(index),
    )
}

fn reject_unknown_keys(
    object: &BodyObject,
    allowed: &[&str],
    bundle: &BundleId,
    field: EnvelopeErrorField,
    index: Option<u64>,
) -> Result<(), EnvelopeError> {
    if object.keys().any(|key| {
        !allowed
            .iter()
            .any(|allowed_key| body_string_matches(key, allowed_key))
    }) {
        return Err(error(
            Some(bundle),
            EnvelopeErrorCode::UnknownField,
            field,
            index,
        ));
    }
    Ok(())
}

fn required_value<'a>(
    object: &'a BodyObject,
    key: &str,
    bundle: &BundleId,
    field: EnvelopeErrorField,
    index: Option<u64>,
) -> Result<&'a BodyValue, EnvelopeError> {
    field_value(object, key)
        .ok_or_else(|| error(Some(bundle), EnvelopeErrorCode::MissingField, field, index))
}

fn required_string<'a>(
    object: &'a BodyObject,
    key: &str,
    bundle: Option<&BundleId>,
    field: EnvelopeErrorField,
    index: Option<u64>,
) -> Result<&'a BodyString, EnvelopeError> {
    match field_value(object, key) {
        None => Err(error(bundle, EnvelopeErrorCode::MissingField, field, index)),
        Some(BodyValue::String(value)) => Ok(value),
        Some(_) => Err(error(bundle, EnvelopeErrorCode::WrongType, field, index)),
    }
}

fn field_value<'a>(object: &'a BodyObject, key: &str) -> Option<&'a BodyValue> {
    let key = BodyString::from_code_points(key.bytes().map(u32::from).collect())
        .expect("ASCII envelope key is a valid body string");
    object.get(&key)
}

fn body_string_matches(value: &BodyString, literal: &str) -> bool {
    value
        .code_points()
        .iter()
        .copied()
        .eq(literal.bytes().map(u32::from))
}

fn error(
    bundle: Option<&BundleId>,
    code: EnvelopeErrorCode,
    field: EnvelopeErrorField,
    index: Option<u64>,
) -> EnvelopeError {
    EnvelopeError::new(bundle.cloned(), code, field, index)
}
