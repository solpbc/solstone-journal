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
    use std::error::Error;

    use solstone_core_body_source::BundleId;

    use super::{BodyDedupeError, BodyDedupeErrorField};

    #[test]
    fn body_dedupe_error_field_all_is_declaration_ordered() {
        let mut sorted = BodyDedupeErrorField::ALL.to_vec();
        sorted.sort();
        assert_eq!(sorted, BodyDedupeErrorField::ALL.to_vec());

        for field in BodyDedupeErrorField::ALL {
            let spelling = match field {
                BodyDedupeErrorField::SourceRecordId => "source_record_id",
                BodyDedupeErrorField::RecordType => "record_type",
                BodyDedupeErrorField::StartTime => "start_time",
                BodyDedupeErrorField::EndTime => "end_time",
                BodyDedupeErrorField::RawRef => "raw_ref",
            };
            assert_eq!(field.as_str(), spelling);
        }
    }

    #[test]
    fn body_dedupe_error_is_bounded_at_the_maximum_checked_coordinate() {
        let bundle = BundleId::from_bytes(b"body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ")
            .expect("maximum bundle id is valid");
        let error = BodyDedupeError::new(bundle, u64::MAX, BodyDedupeErrorField::SourceRecordId);
        let expected = "body-dedupe[body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ]#E18446744073709551615 invalid_text: source_record_id";

        assert_eq!(error.to_string(), expected);
        assert_eq!(format!("{error:?}"), expected);
        assert!(expected.is_ascii());
        assert!(expected.len() <= 256);
        assert!(Error::source(&error).is_none());
    }
}
