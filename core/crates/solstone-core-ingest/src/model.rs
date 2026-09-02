// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

/// Closed vocabulary for every ingest refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasonCode {
    ProtocolVersionRequired,
    ProtocolVersionMalformed,
    ProtocolVersionLegacy,
    ProtocolVersionFuture,
    LinkedDeviceRequired,
    LegacyObserverField,
    LegacyStreamField,
    SourceNotUtf8,
    SourceTooLong,
    SourceContainsNul,
    SourceContainsPathSeparator,
    SourceContainsDot,
    SourceInvalidCharacter,
    DayInvalid,
    SegmentInvalid,
    FieldMissing,
    FieldDuplicate,
    EnvelopeInvalid,
    FileMetadataInvalid,
    FileNameMismatch,
    FileNameInvalid,
    FileNameDuplicate,
    MultipartMalformed,
    MultipartPartTooLarge,
    MultipartTooManyParts,
    MultipartTooManyFiles,
    MultipartTooManyHeaders,
    MultipartFilenameTooLong,
    ContentConflict,
    SegmentAllocationFailed,
    JournalWriteFailed,
    EventAppendFailed,
    StreamAdvanceFailed,
    StreamMarkerBumpFailed,
    LocationLockUnavailable,
    NotifyFailed,
    JournalReadFailed,
    MalformedEvidenceRow,
    AmbiguousSegmentFileName,
    ForeignStreamBinding,
    PairingIdentityUnavailable,
    StreamBindingIncomplete,
}

impl ReasonCode {
    #[cfg(test)]
    const ALL: [Self; 42] = [
        Self::ProtocolVersionRequired,
        Self::ProtocolVersionMalformed,
        Self::ProtocolVersionLegacy,
        Self::ProtocolVersionFuture,
        Self::LinkedDeviceRequired,
        Self::LegacyObserverField,
        Self::LegacyStreamField,
        Self::SourceNotUtf8,
        Self::SourceTooLong,
        Self::SourceContainsNul,
        Self::SourceContainsPathSeparator,
        Self::SourceContainsDot,
        Self::SourceInvalidCharacter,
        Self::DayInvalid,
        Self::SegmentInvalid,
        Self::FieldMissing,
        Self::FieldDuplicate,
        Self::EnvelopeInvalid,
        Self::FileMetadataInvalid,
        Self::FileNameMismatch,
        Self::FileNameInvalid,
        Self::FileNameDuplicate,
        Self::MultipartMalformed,
        Self::MultipartPartTooLarge,
        Self::MultipartTooManyParts,
        Self::MultipartTooManyFiles,
        Self::MultipartTooManyHeaders,
        Self::MultipartFilenameTooLong,
        Self::ContentConflict,
        Self::SegmentAllocationFailed,
        Self::JournalWriteFailed,
        Self::EventAppendFailed,
        Self::StreamAdvanceFailed,
        Self::StreamMarkerBumpFailed,
        Self::LocationLockUnavailable,
        Self::NotifyFailed,
        Self::JournalReadFailed,
        Self::MalformedEvidenceRow,
        Self::AmbiguousSegmentFileName,
        Self::ForeignStreamBinding,
        Self::PairingIdentityUnavailable,
        Self::StreamBindingIncomplete,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProtocolVersionRequired => "protocol_version_required",
            Self::ProtocolVersionMalformed => "protocol_version_malformed",
            Self::ProtocolVersionLegacy => "protocol_version_legacy",
            Self::ProtocolVersionFuture => "protocol_version_future",
            Self::LinkedDeviceRequired => "linked_device_required",
            Self::LegacyObserverField => "legacy_observer_field",
            Self::LegacyStreamField => "legacy_stream_field",
            Self::SourceNotUtf8 => "source_not_utf8",
            Self::SourceTooLong => "source_too_long",
            Self::SourceContainsNul => "source_contains_nul",
            Self::SourceContainsPathSeparator => "source_contains_path_separator",
            Self::SourceContainsDot => "source_contains_dot",
            Self::SourceInvalidCharacter => "source_invalid_character",
            Self::DayInvalid => "day_invalid",
            Self::SegmentInvalid => "segment_invalid",
            Self::FieldMissing => "field_missing",
            Self::FieldDuplicate => "field_duplicate",
            Self::EnvelopeInvalid => "envelope_invalid",
            Self::FileMetadataInvalid => "file_metadata_invalid",
            Self::FileNameMismatch => "file_name_mismatch",
            Self::FileNameInvalid => "file_name_invalid",
            Self::FileNameDuplicate => "file_name_duplicate",
            Self::MultipartMalformed => "multipart_malformed",
            Self::MultipartPartTooLarge => "multipart_part_too_large",
            Self::MultipartTooManyParts => "multipart_too_many_parts",
            Self::MultipartTooManyFiles => "multipart_too_many_files",
            Self::MultipartTooManyHeaders => "multipart_too_many_headers",
            Self::MultipartFilenameTooLong => "multipart_filename_too_long",
            Self::ContentConflict => "content_conflict",
            Self::SegmentAllocationFailed => "segment_allocation_failed",
            Self::JournalWriteFailed => "journal_write_failed",
            Self::EventAppendFailed => "event_append_failed",
            Self::StreamAdvanceFailed => "stream_advance_failed",
            Self::StreamMarkerBumpFailed => "stream_marker_bump_failed",
            Self::LocationLockUnavailable => "location_lock_unavailable",
            Self::NotifyFailed => "notify_failed",
            Self::JournalReadFailed => "journal_read_failed",
            Self::MalformedEvidenceRow => "malformed_evidence_row",
            Self::AmbiguousSegmentFileName => "ambiguous_segment_file_name",
            Self::ForeignStreamBinding => "foreign_stream_binding",
            Self::PairingIdentityUnavailable => "pairing_identity_unavailable",
            Self::StreamBindingIncomplete => "stream_binding_incomplete",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::ReasonCode;

    #[test]
    fn reason_code_strings_are_closed_and_distinct() {
        let values: HashSet<_> = ReasonCode::ALL
            .into_iter()
            .map(ReasonCode::as_str)
            .collect();
        assert_eq!(values.len(), ReasonCode::ALL.len());
    }
}

#[derive(Clone, Debug)]
pub struct IncomingFile {
    pub submitted: String,
    pub bytes: Vec<u8>,
    pub descriptor_extra: Map<String, Value>,
}
