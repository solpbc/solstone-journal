// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::StoreError;
use solstone_core_format::paths::JournalPathError;
use solstone_core_indexer::{discovery::DiscoveryError, edges::EdgeError};

impl StoreError {
    pub fn cli_detail(&self, locale: &str) -> String {
        self.diagnostic().render(locale)
    }

    pub(crate) fn diagnostic(&self) -> solstone_core_cli_copy::MessageDetail {
        match self {
            Self::Discovery(error) => discovery_detail(error),
            Self::Edge(error) => edge_message(error),
            Self::EdgeFileFailed(messages) => messages.clone(),
            Self::Io(error) => io_message(error),
            Self::Sql(error) => error.to_string().into(),
            Self::Path(error) => path_detail(error),
            Self::OutsideJournal(path) => solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.store.outside",
                [("path", &(path.display()))],
            ),
            Self::NonUtf8Path(path) => solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.store.encoding",
                [("path", &(path.display()))],
            ),
            Self::MissingFile(path) => solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.store.missing",
                [("path", &(path.display()))],
            ),
            Self::MemoryOriginalUnavailable { path, detail } => {
                solstone_core_cli_copy::MessageDetail::message(
                    "journal.cli.journal.indexer.store.original",
                    [("path", &(path.display())), ("detail", &(detail))],
                )
            }
            Self::PathLookupRequired { cause: None } => "indexer path-lookup --apply".into(),
            Self::PathLookupRequired { cause: Some(cause) } => {
                solstone_core_cli_copy::MessageDetail::message(
                    "journal.cli.journal.indexer.store.lookup.cause",
                    [("cause", &(cause))],
                )
            }
            Self::WriterBusy { operation, cause } => {
                solstone_core_cli_copy::MessageDetail::message(
                    "journal.cli.journal.indexer.store.busy",
                    [("operation", &(operation)), ("cause", &(cause))],
                )
            }
            Self::WriterAdmission { operation, cause } => {
                solstone_core_cli_copy::MessageDetail::message(
                    "journal.cli.journal.indexer.store.admission",
                    [("operation", &(operation)), ("cause", &(cause))],
                )
            }
            Self::IndexGenerationNewer { found, known } => {
                solstone_core_cli_copy::MessageDetail::message(
                    "journal.cli.journal.indexer.store.generation",
                    [("found", &(found)), ("known", &(known))],
                )
            }
        }
    }
}

fn path_detail(error: &JournalPathError) -> solstone_core_cli_copy::MessageDetail {
    match error {
        JournalPathError::Empty => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.rel.empty",
            [],
        ),
        JournalPathError::Absolute => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.rel.absolute",
            [],
        ),
        JournalPathError::Backslash => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.rel.backslash",
            [],
        ),
        JournalPathError::InvalidComponent => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.rel.invalidcomponent",
            [],
        ),
        JournalPathError::Trash => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.rel.trash",
            [],
        ),
    }
}

fn discovery_detail(error: &DiscoveryError) -> solstone_core_cli_copy::MessageDetail {
    match error {
        DiscoveryError::NonUtf8Root(path) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.discovery.root",
            [("path", &(path.display()))],
        ),
        DiscoveryError::NonUtf8Relative(path) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.discovery.relative",
            [("path", &(path.display()))],
        ),
        DiscoveryError::Pattern(error) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.discovery.pattern",
            [("error", &(error))],
        ),
        DiscoveryError::Glob(error) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.discovery.listing",
            [("path", &error.path().display()), ("error", &error.error())],
        ),
        DiscoveryError::Listing { path, error } => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.discovery.listing",
            [("path", &(path.display())), ("error", &(error))],
        ),
        DiscoveryError::StripPrefix { path, root } => {
            solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.discovery.prefix",
                [("path", &(path.display())), ("root", &(root.display()))],
            )
        }
        DiscoveryError::JournalPath(error) => path_detail(error),
    }
}

pub(crate) fn edge_message(error: &EdgeError) -> solstone_core_cli_copy::MessageDetail {
    match error {
        EdgeError::Io(error) => error.to_string().into(),
        EdgeError::JournalConfigCorrupt { path, .. } => {
            solstone_core_cli_copy::MessageDetail::window(
                "journal.window.facets.awareness.response.detail.unreadable.settings",
                [("settings_path", &path.display())],
            )
        }
        EdgeError::UnknownKind(kind) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.kind",
            [("kind", &(format!("{kind:?}")))],
        ),
        EdgeError::MissingSrc => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.src",
            [],
        ),
        EdgeError::MissingDst => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.dst",
            [],
        ),
        EdgeError::InvalidDay(day) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.day",
            [("day", &(format!("{day:?}")))],
        ),
        EdgeError::InvalidObservationPath(path) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.path",
            [("path", &(path))],
        ),
        EdgeError::MissingObservationRelationKind => {
            solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.edge.relation",
                [],
            )
        }
        EdgeError::InvalidObservationTargetEntityId => {
            solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.edge.target",
                [],
            )
        }
        EdgeError::InvalidJsonPayload { source, value_type } => {
            solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.edge.payload",
                [("source", &(source)), ("value_type", &(value_type))],
            )
        }
        EdgeError::MalformedJson { path, error } => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.json",
            [("path", &(path)), ("error", &(error))],
        ),
        EdgeError::InvalidEdgeKindType { source, value_type } => {
            solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.edge.type",
                [("source", &(source)), ("value_type", &(value_type))],
            )
        }
        EdgeError::UnsupportedEdgeValue { field, value_type } => {
            solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.edge.unsupported",
                [("field", &(field)), ("value_type", &(value_type))],
            )
        }
        EdgeError::UnrepresentableEdgeStringValue { field, value_type } => {
            solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.edge.string",
                [("field", &(field)), ("value_type", &(value_type))],
            )
        }
        EdgeError::EdgeIntegerOutOfRange { field } => {
            solstone_core_cli_copy::MessageDetail::message(
                "journal.cli.journal.indexer.edge.integer",
                [("field", &(field))],
            )
        }
        EdgeError::EventTimestampOutOfRange => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.event",
            [],
        ),
        EdgeError::InvalidSegmentDay(day) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.segment.day",
            [("day", &(format!("{day:?}")))],
        ),
        EdgeError::InvalidSegmentKey(segment) => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.segment.key",
            [("segment", &(format!("{segment:?}")))],
        ),
        EdgeError::SegmentTimestampOutOfRange => solstone_core_cli_copy::MessageDetail::message(
            "journal.cli.journal.indexer.edge.segment.time",
            [],
        ),
    }
}

pub(crate) fn io_message(error: &std::io::Error) -> solstone_core_cli_copy::MessageDetail {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<solstone_core_cli_copy::MessageDetail>())
        .cloned()
        .unwrap_or_else(|| error.to_string().into())
}
