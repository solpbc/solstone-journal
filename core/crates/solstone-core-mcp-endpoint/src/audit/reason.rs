// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};
use solstone_core_convey_http::window_catalog::text;

/// Only the producer of a journal-authored reason can attach its identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    Unenforceable,
    NoRead,
    PermissionChanged,
    WholePlace,
    PersonOrActivity,
    OutsideScope,
    WeekWhole,
    IndexAbsent,
    IndexUnreadable,
    IndexLocked,
    EmptyIndex,
    NotIndexed,
    FileUnreadable,
    ReferenceMissing,
    AuditUnavailable,
    InvalidArguments,
    OperationConflict,
    OutsidePermission,
    Paused,
    ReceiptUnconfirmed,
    CursorUnavailable,
    OriginalEncoding,
    ResponsePreparation,
    ResponseSerialization,
    OriginalUnconfirmed,
    RecordValidation,
    RecordRead,
    DifferentBytes,
    ReservationPublication,
    ReadTranscripts,
    ReadMessages,
    ReadTransactions,
    ReadEntities,
    ReadProfilePics,
    ReadFacets,
    ReadScreenText,
    ReadScreenshots,
    ReadBody,
    ReadWomensHealth,
    ReadNotes,
    ReadDocuments,
    ReadCalendar,
    ReadPlaces,
    ReadPhotos,
    MissingEntities,
    MissingFacets,
    MissingTranscripts,
    MissingWeek,
    MissingActivitySource,
    MissingSegment,
    MissingSearch,
    MissingSuggestion,
}

impl Reason {
    pub(crate) fn render(self, locale: &str) -> String {
        if matches!(self, Self::ReadTransactions) {
            return "This connection does not have permission to read transactions.".to_owned();
        }
        let id = match self {
            Self::Unenforceable => "journal.window.agents.activity.reason.unenforceable",
            Self::NoRead => "journal.window.agents.activity.reason.no.read",
            Self::PermissionChanged => "journal.window.agents.activity.reason.permission.changed",
            Self::WholePlace => "journal.window.agents.activity.reason.whole.place",
            Self::PersonOrActivity => "journal.window.agents.activity.reason.person.or.activity",
            Self::OutsideScope => "journal.window.agents.activity.reason.outside.scope",
            Self::WeekWhole => "journal.window.agents.activity.reason.week.whole",
            Self::IndexAbsent => "journal.window.agents.activity.reason.index.absent",
            Self::IndexUnreadable => "journal.window.agents.activity.reason.index.unreadable",
            Self::IndexLocked => "journal.window.agents.activity.reason.index.locked",
            Self::EmptyIndex => "journal.window.agents.activity.reason.empty.index",
            Self::NotIndexed => "journal.window.agents.activity.reason.not.indexed",
            Self::FileUnreadable => "journal.window.agents.activity.reason.file.unreadable",
            Self::ReferenceMissing => "journal.window.agents.activity.reason.reference.missing",
            Self::AuditUnavailable => "journal.window.agents.activity.reason.audit.unavailable",
            Self::InvalidArguments => "journal.window.agents.activity.reason.invalid.arguments",
            Self::OperationConflict => "journal.window.agents.activity.reason.operation.conflict",
            Self::OutsidePermission => "journal.window.agents.activity.reason.outside.permission",
            Self::Paused => "journal.window.agents.activity.reason.paused",
            Self::ReceiptUnconfirmed => "journal.window.agents.activity.reason.receipt.unconfirmed",
            Self::CursorUnavailable => "journal.window.agents.activity.reason.cursor.unavailable",
            Self::OriginalEncoding => "journal.window.agents.activity.reason.original.encoding",
            Self::ResponsePreparation => {
                "journal.window.agents.activity.reason.response.preparation"
            }
            Self::ResponseSerialization => {
                "journal.window.agents.activity.reason.response.serialization"
            }
            Self::OriginalUnconfirmed => {
                "journal.window.agents.activity.reason.original.unconfirmed"
            }
            Self::RecordValidation => "journal.window.agents.activity.reason.record.validation",
            Self::RecordRead => "journal.window.agents.activity.reason.record.read",
            Self::DifferentBytes => "journal.window.agents.activity.reason.different.bytes",
            Self::ReservationPublication => {
                "journal.window.agents.activity.reason.reservation.publication"
            }
            Self::ReadTranscripts => "journal.window.agents.activity.reason.read.transcripts",
            Self::ReadMessages => "journal.window.agents.activity.reason.read.messages",
            Self::ReadTransactions => "journal.window.agents.activity.reason.read.transactions",
            Self::ReadEntities => "journal.window.agents.activity.reason.read.entities",
            Self::ReadProfilePics => "journal.window.agents.activity.reason.read.profile.pics",
            Self::ReadFacets => "journal.window.agents.activity.reason.read.facets",
            Self::ReadScreenText => "journal.window.agents.activity.reason.read.screen.text",
            Self::ReadScreenshots => "journal.window.agents.activity.reason.read.screenshots",
            Self::ReadBody => "journal.window.agents.activity.reason.read.body",
            Self::ReadWomensHealth => "journal.window.agents.activity.reason.read.womens.health",
            Self::ReadNotes => "journal.window.agents.activity.reason.read.notes",
            Self::ReadDocuments => "journal.window.agents.activity.reason.read.documents",
            Self::ReadCalendar => "journal.window.agents.activity.reason.read.calendar",
            Self::ReadPlaces => "journal.window.agents.activity.reason.read.places",
            Self::ReadPhotos => "journal.window.agents.activity.reason.read.photos",
            Self::MissingEntities => "journal.window.agents.activity.reason.missing.entities",
            Self::MissingFacets => "journal.window.agents.activity.reason.missing.facets",
            Self::MissingTranscripts => "journal.window.agents.activity.reason.missing.transcripts",
            Self::MissingWeek => "journal.window.agents.activity.reason.missing.week",
            Self::MissingActivitySource => {
                "journal.window.agents.activity.reason.missing.activity.source"
            }
            Self::MissingSegment => "journal.window.agents.activity.reason.missing.segment",
            Self::MissingSearch => "journal.window.agents.activity.reason.missing.search",
            Self::MissingSuggestion => "journal.window.agents.activity.reason.missing.suggestion",
        };
        text(locale, id)
    }
}
