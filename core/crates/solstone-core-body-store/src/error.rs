// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use solstone_core_body_source::BundleId;

/// The closed set of fields which can contain non-scalar body text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BodyDedupeErrorField {
    SourceRecordId,
    RecordType,
    StartTime,
    EndTime,
    RawRef,
}

impl BodyDedupeErrorField {
    /// Returns this field's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SourceRecordId => "source_record_id",
            Self::RecordType => "record_type",
            Self::StartTime => "start_time",
            Self::EndTime => "end_time",
            Self::RawRef => "raw_ref",
        }
    }

    pub const ALL: [Self; 5] = [
        Self::SourceRecordId,
        Self::RecordType,
        Self::StartTime,
        Self::EndTime,
        Self::RawRef,
    ];
}

/// A contextual refusal while replaying a validated body observation.
#[derive(Clone, PartialEq, Eq)]
pub struct BodyDedupeError {
    bundle: BundleId,
    sequence: u64,
    field: BodyDedupeErrorField,
}

impl BodyDedupeError {
    pub(crate) fn new(bundle: BundleId, sequence: u64, field: BodyDedupeErrorField) -> Self {
        Self {
            bundle,
            sequence,
            field,
        }
    }

    /// Returns the bundle associated with the failed observation.
    pub fn bundle(&self) -> &BundleId {
        &self.bundle
    }

    /// Returns the event sequence associated with the failed observation.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Returns the field which contained invalid text.
    pub fn field(&self) -> BodyDedupeErrorField {
        self.field
    }
}

impl fmt::Display for BodyDedupeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "body-dedupe[{}]#E{} invalid_text: {}",
            self.bundle.as_str(),
            self.sequence,
            self.field.as_str()
        )
    }
}

impl fmt::Debug for BodyDedupeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodyDedupeError {}

#[cfg(test)]
mod tests {
    use super::BodyDedupeErrorField;

    #[test]
    fn body_dedupe_error_field_all_is_declaration_ordered() {
        let mut sorted = BodyDedupeErrorField::ALL.to_vec();
        sorted.sort();
        assert_eq!(sorted, BodyDedupeErrorField::ALL.to_vec());
    }
}
