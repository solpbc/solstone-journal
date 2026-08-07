// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    BodyObject, BodyString, BodyValue, CandidateError, CandidateErrorCode, CandidateErrorField,
    Coordinate, PresentationRow,
};

/// A normalized-row schema accepted for ledger replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerSchema {
    AppleHealthV1,
    OuraV1,
    NormalizedV1,
}

impl LedgerSchema {
    /// Returns this schema's exact wire value.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AppleHealthV1 => "solstone.health.apple_health.v1",
            Self::OuraV1 => "solstone.health.oura.v1",
            Self::NormalizedV1 => "solstone.health.normalized.v1",
        }
    }

    /// Resolves an exact, case-sensitive schema wire value.
    pub fn from_exact(value: &str) -> Option<Self> {
        match value {
            "solstone.health.apple_health.v1" => Some(Self::AppleHealthV1),
            "solstone.health.oura.v1" => Some(Self::OuraV1),
            "solstone.health.normalized.v1" => Some(Self::NormalizedV1),
            _ => None,
        }
    }

    /// Returns the sole source family compatible with this schema.
    pub const fn expected_family(&self) -> &'static str {
        match self {
            Self::AppleHealthV1 | Self::NormalizedV1 => "apple_health",
            Self::OuraV1 => "oura_api",
        }
    }
}

/// A JSON field whose absence and explicit null remain distinct.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldState<T> {
    Absent,
    Null,
    Present(T),
}

/// The optional value field, whose present state retains every body value.
#[derive(Clone, Debug, PartialEq)]
pub enum ValueState {
    Absent,
    Present(BodyValue),
}

/// A validated normalized body row suitable for ledger replay.
#[derive(Clone, Debug, PartialEq)]
pub struct LedgerCandidate {
    schema: LedgerSchema,
    source_family: BodyString,
    record_type: BodyString,
    dedupe_key: BodyString,
    start_date: BodyString,
    day: BodyString,
    kind: FieldState<BodyString>,
    import_id: FieldState<BodyString>,
    month: FieldState<BodyString>,
    end_date: FieldState<BodyString>,
    source_record_id: FieldState<BodyString>,
    source_name: FieldState<BodyString>,
    source_version: FieldState<BodyString>,
    unit: FieldState<BodyString>,
    normalized_ref: FieldState<BodyString>,
    raw_ref: FieldState<BodyString>,
    metadata: FieldState<BodyObject>,
    value: ValueState,
}

impl LedgerCandidate {
    pub fn schema(&self) -> LedgerSchema {
        self.schema
    }

    pub fn source_family(&self) -> &BodyString {
        &self.source_family
    }

    pub fn record_type(&self) -> &BodyString {
        &self.record_type
    }

    pub fn dedupe_key(&self) -> &BodyString {
        &self.dedupe_key
    }

    pub fn start_date(&self) -> &BodyString {
        &self.start_date
    }

    pub fn day(&self) -> &BodyString {
        &self.day
    }

    pub fn kind(&self) -> &FieldState<BodyString> {
        &self.kind
    }

    pub fn import_id(&self) -> &FieldState<BodyString> {
        &self.import_id
    }

    pub fn month(&self) -> &FieldState<BodyString> {
        &self.month
    }

    pub fn end_date(&self) -> &FieldState<BodyString> {
        &self.end_date
    }

    pub fn source_record_id(&self) -> &FieldState<BodyString> {
        &self.source_record_id
    }

    pub fn source_name(&self) -> &FieldState<BodyString> {
        &self.source_name
    }

    pub fn source_version(&self) -> &FieldState<BodyString> {
        &self.source_version
    }

    pub fn unit(&self) -> &FieldState<BodyString> {
        &self.unit
    }

    pub fn normalized_ref(&self) -> &FieldState<BodyString> {
        &self.normalized_ref
    }

    pub fn raw_ref(&self) -> &FieldState<BodyString> {
        &self.raw_ref
    }

    pub fn metadata(&self) -> &FieldState<BodyObject> {
        &self.metadata
    }

    pub fn value(&self) -> &ValueState {
        &self.value
    }
}

/// Projects a lossless presentation row into a validated ledger candidate.
pub fn project(
    row: &PresentationRow,
    coordinate: Coordinate,
) -> Result<LedgerCandidate, CandidateError> {
    let object = row.object();

    let schema = field_value(object, "schema")
        .and_then(|value| match value {
            BodyValue::String(value) => schema_from_body_string(value),
            _ => None,
        })
        .ok_or_else(|| {
            CandidateError::new(
                &coordinate,
                CandidateErrorCode::UnsupportedSchema,
                CandidateErrorField::Schema,
            )
        })?;

    let source_family = required_nonblank_string(object, "source_family").map_err(|code| {
        CandidateError::new(&coordinate, code, CandidateErrorField::SourceFamily)
    })?;
    if !body_string_matches(source_family, schema.expected_family()) {
        return Err(CandidateError::new(
            &coordinate,
            CandidateErrorCode::IncompatibleField,
            CandidateErrorField::SourceFamily,
        ));
    }
    let record_type = required_nonblank_string(object, "record_type")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::RecordType))?;
    let dedupe_key = required_nonblank_string(object, "dedupe_key")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::DedupeKey))?;
    let start_date = required_nonblank_string(object, "start_date")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::StartDate))?;
    let day = required_string(object, "day")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::Day))?;

    let kind = optional_string(object, "kind")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::Kind))?;
    let import_id = optional_string(object, "import_id")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::ImportId))?;
    let month = optional_string(object, "month")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::Month))?;
    let end_date = optional_string(object, "end_date")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::EndDate))?;
    let source_record_id = optional_string(object, "source_record_id").map_err(|code| {
        CandidateError::new(&coordinate, code, CandidateErrorField::SourceRecordId)
    })?;
    let source_name = optional_string(object, "source_name")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::SourceName))?;
    let source_version = optional_string(object, "source_version").map_err(|code| {
        CandidateError::new(&coordinate, code, CandidateErrorField::SourceVersion)
    })?;
    let unit = optional_string(object, "unit")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::Unit))?;
    let normalized_ref = optional_string(object, "normalized_ref").map_err(|code| {
        CandidateError::new(&coordinate, code, CandidateErrorField::NormalizedRef)
    })?;
    let raw_ref = optional_string(object, "raw_ref")
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::RawRef))?;
    let metadata = optional_metadata(object)
        .map_err(|code| CandidateError::new(&coordinate, code, CandidateErrorField::Metadata))?;
    let value = match field_value(object, "value") {
        None => ValueState::Absent,
        Some(value) => ValueState::Present(value.clone()),
    };

    Ok(LedgerCandidate {
        schema,
        source_family: source_family.clone(),
        record_type: record_type.clone(),
        dedupe_key: dedupe_key.clone(),
        start_date: start_date.clone(),
        day: day.clone(),
        kind,
        import_id,
        month,
        end_date,
        source_record_id,
        source_name,
        source_version,
        unit,
        normalized_ref,
        raw_ref,
        metadata,
        value,
    })
}

fn field_value<'a>(object: &'a BodyObject, name: &str) -> Option<&'a BodyValue> {
    let key = BodyString::from_code_points(name.bytes().map(u32::from).collect())
        .expect("ASCII field name is a valid body string");
    object.get(&key)
}

fn schema_from_body_string(value: &BodyString) -> Option<LedgerSchema> {
    [
        LedgerSchema::AppleHealthV1,
        LedgerSchema::OuraV1,
        LedgerSchema::NormalizedV1,
    ]
    .into_iter()
    .find(|schema| body_string_matches(value, schema.as_str()))
}

fn required_nonblank_string<'a>(
    object: &'a BodyObject,
    name: &str,
) -> Result<&'a BodyString, CandidateErrorCode> {
    let value = required_string(object, name)?;
    if is_blank(value) {
        Err(CandidateErrorCode::BlankField)
    } else {
        Ok(value)
    }
}

fn required_string<'a>(
    object: &'a BodyObject,
    name: &str,
) -> Result<&'a BodyString, CandidateErrorCode> {
    match field_value(object, name) {
        None => Err(CandidateErrorCode::MissingField),
        Some(BodyValue::String(value)) => Ok(value),
        Some(BodyValue::Null) | Some(_) => Err(CandidateErrorCode::WrongType),
    }
}

fn optional_string(
    object: &BodyObject,
    name: &str,
) -> Result<FieldState<BodyString>, CandidateErrorCode> {
    match field_value(object, name) {
        None => Ok(FieldState::Absent),
        Some(BodyValue::Null) => Ok(FieldState::Null),
        Some(BodyValue::String(value)) => Ok(FieldState::Present(value.clone())),
        Some(_) => Err(CandidateErrorCode::WrongType),
    }
}

fn optional_metadata(object: &BodyObject) -> Result<FieldState<BodyObject>, CandidateErrorCode> {
    match field_value(object, "metadata") {
        None => Ok(FieldState::Absent),
        Some(BodyValue::Null) => Ok(FieldState::Null),
        Some(BodyValue::Object(value)) => Ok(FieldState::Present(value.clone())),
        Some(_) => Err(CandidateErrorCode::WrongType),
    }
}

fn body_string_matches(value: &BodyString, literal: &str) -> bool {
    value
        .code_points()
        .iter()
        .copied()
        .eq(literal.bytes().map(u32::from))
}

fn is_blank(value: &BodyString) -> bool {
    value
        .code_points()
        .iter()
        .copied()
        .all(is_python_whitespace)
}

const fn is_python_whitespace(code_point: u32) -> bool {
    matches!(
        code_point,
        0x0009..=0x000d
            | 0x001c..=0x001f
            | 0x0020
            | 0x0085
            | 0x00a0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
    )
}
