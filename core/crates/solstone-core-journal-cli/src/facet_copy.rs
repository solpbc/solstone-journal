// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::terminal_language;
use solstone_core_facets::{
    FacetStoreError, FacetTrustLockError, FacetWriteError, RetiredRecordError,
};
use solstone_core_journal_io::cli_diagnostics::{path_detail, read_detail};

pub(super) fn publication(error: &solstone_core_journal_io::AtomicWriteError) -> String {
    solstone_core_facets::cli_copy::publication(terminal_language(), error)
}

pub(super) fn trust(error: &FacetTrustLockError) -> String {
    solstone_core_facets::cli_copy::trust(terminal_language(), error)
}

pub(super) fn retired(error: &RetiredRecordError) -> String {
    solstone_core_facets::cli_copy::retired(terminal_language(), error)
}

pub(super) fn store(error: &FacetStoreError) -> String {
    solstone_core_facets::cli_copy::store(terminal_language(), error)
}

pub(super) fn write(error: &FacetWriteError) -> String {
    solstone_core_facets::cli_copy::write(terminal_language(), error)
}

#[cfg(not(target_os = "ios"))]
pub(super) fn hidden(error: &solstone_core_facets::HiddenNamesError) -> String {
    use solstone_core_facets::HiddenNamesError as E;
    match error {
        E::EmptyName => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.hidden.empty",
            &[],
        ),
        E::Damaged { path, detail } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.hidden.damaged",
            [
                ("path", &(path.display())),
                ("detail", &hidden_record(detail)),
            ],
        ),
        E::TrustLock(e) => trust(e),
        E::Path(e) => path_detail(terminal_language(), e),
        E::Facet(e) => write(e),
        E::Write(e) => publication(e),
    }
}

#[cfg(not(target_os = "ios"))]
pub(super) fn link(error: &solstone_core_facets::facet_links::LinkFolderError) -> String {
    use solstone_core_facets::facet_links::LinkFolderError as E;
    match error {
        E::TrustLock(e) => trust(e),
        E::Path(e) => path_detail(terminal_language(), e),
        E::Read(e) => read_detail(terminal_language(), e),
        E::Write(e) => publication(e),
        E::NotUtf8 { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.link.file.encoding",
            [("path", &(path.display()))],
        ),
        E::NotObject { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.json.object",
            [("path", &path.display())],
        ),
        E::Conflict { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.link.conflict",
            [("path", &(path.display()))],
        ),
        E::NotAFile { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.link.not.file",
            [("path", &(path.display()))],
        ),
        E::InvalidName { name } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.link.invalid.name",
            [("name", &(name))],
        ),
        E::IdSpace => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.link.ids.exhausted",
            &[],
        ),
        E::Hook(message) | E::Resolve(message) => message.clone(),
        E::Observations(e) => observation(e),
        E::NeedsRepair {
            entities,
            entity_id,
        } => match entities
            .strip_prefix("facets/")
            .and_then(|s| s.strip_suffix("/entities"))
        {
            Some(facet) => solstone_core_cli_copy::display_for(
                terminal_language(),
                "journal.cli.journal.facet.link.repair.named",
                [("id", &(entity_id)), ("facet", &(facet))],
            ),
            None => solstone_core_cli_copy::display_for(
                terminal_language(),
                "journal.cli.journal.facet.link.repair.destination",
                [("id", &(entity_id))],
            ),
        },
    }
}

#[cfg(not(target_os = "ios"))]
pub(super) fn observation(error: &solstone_core_facets::ObservationStoreError) -> String {
    use solstone_core_facets::ObservationStoreError as E;
    match error {
        E::Path(e) => path_detail(terminal_language(), e),
        E::Read(e) => read_detail(terminal_language(), e),
        E::Other(detail) => detail.clone(),
        E::MalformedObservation {
            source,
            line,
            reason,
        } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.link.observation.malformed",
            [
                ("source", &observation_source(source)),
                ("line", &(line)),
                ("reason", &record_reason(reason)),
            ],
        ),
        E::CorruptObservation { source, detail } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.link.observation.corrupt",
            [
                ("source", &observation_source(source)),
                ("detail", &(detail)),
            ],
        ),
    }
}

#[cfg(not(target_os = "ios"))]
pub(super) fn trash(error: &solstone_core_trash::TrashError) -> String {
    error.message_for(terminal_language())
}
#[cfg(not(target_os = "ios"))]
pub(super) fn repair_note(note: solstone_core_facets::facet_links::RepairNote<'_>) -> String {
    use solstone_core_facets::facet_links::RepairNote;
    match note {
        RepairNote::Error(e) => link(e),
        RepairNote::Occupied(id) => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.link.occupied",
            [("id", &(id))],
        ),
        RepairNote::MissingSuccessor => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.link.successor.missing",
            &[],
        ),
    }
}

#[cfg(not(target_os = "ios"))]
fn hidden_record(error: &solstone_core_facets::hidden_names::HiddenRecordError) -> String {
    use solstone_core_facets::hidden_names::{HiddenRecordError as E, HiddenRowError as Row};
    match error {
        E::Path(e) => path_detail(terminal_language(), e),
        E::External(text) => text.clone(),
        E::NotObject => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.not.object",
            &[],
        ),
        E::UnknownField(key) => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.hidden.field.unknown",
            [("key", &(format!("{key:?}")))],
        ),
        E::MissingList => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.hidden.names.list",
            &[],
        ),
        E::Row { index, issue } => {
            let detail = match issue {
                Row::NotObject => solstone_core_cli_copy::render_for(
                    terminal_language(),
                    "journal.cli.journal.facet.hidden.row.object",
                    &[],
                ),
                Row::UnknownField(key) => solstone_core_cli_copy::display_for(
                    terminal_language(),
                    "journal.cli.journal.facet.hidden.field.unknown",
                    [("key", &(format!("{key:?}")))],
                ),
                Row::NotText(key) => solstone_core_cli_copy::display_for(
                    terminal_language(),
                    "journal.cli.journal.facet.hidden.field.text",
                    [("key", &(key))],
                ),
                Row::EmptyName => solstone_core_cli_copy::render_for(
                    terminal_language(),
                    "journal.cli.journal.facet.hidden.row.empty",
                    &[],
                ),
            };
            solstone_core_cli_copy::display_for(
                terminal_language(),
                "journal.cli.journal.facet.hidden.row.error",
                [("index", &(index)), ("detail", &(detail))],
            )
        }
    }
}

fn observation_source(source: &solstone_core_facets::ObservationErrorSource) -> String {
    solstone_core_facets::cli_copy::observation_source(terminal_language(), source)
}

fn record_reason(reason: &'static str) -> String {
    solstone_core_facets::cli_copy::record_reason(terminal_language(), reason)
}
