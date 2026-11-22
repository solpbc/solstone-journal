// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::terminal_language;
use solstone_core_facets::{
    FacetStoreError, FacetTrustLockError, FacetWriteError, RetiredRecordError,
};
use solstone_core_journal_io::cli_diagnostics::{
    atomic_message, lock_detail, path_detail, read_detail,
};

pub(super) fn publication(error: &solstone_core_journal_io::AtomicWriteError) -> String {
    atomic_message(error).render(terminal_language())
}

pub(super) fn trust(error: &FacetTrustLockError) -> String {
    match error {
        FacetTrustLockError::Path(e) => path_detail(terminal_language(), e),
        FacetTrustLockError::Lock(e) => lock_detail(terminal_language(), e),
    }
}

pub(super) fn retired(error: &RetiredRecordError) -> String {
    use RetiredRecordError as E;
    match error {
        E::NotObject => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.not.object",
            &[],
        ),
        E::MissingNames => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.missing.names",
            &[],
        ),
        E::External(message) => message.clone(),
    }
}

pub(super) fn store(error: &FacetStoreError) -> String {
    use FacetStoreError as E;
    match error {
        E::Path(e) => path_detail(terminal_language(), e),
        E::Read(e) => read_detail(terminal_language(), e),
        E::DeclarationNotObject { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.declaration.invalid",
            [("path", &(path.display()))],
        ),
        E::EntityLinkNotObject { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.link.invalid",
            [("path", &(path.display()))],
        ),
        E::CorruptCompletionMarker { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.marker.invalid",
            [("path", &(path.display()))],
        ),
        E::MalformedObservation {
            source,
            line,
            reason,
        } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.malformed.observation",
            [
                ("source", &(observation_source(source))),
                ("line", &(line)),
                ("reason", &(record_reason(reason))),
            ],
        ),
        E::MalformedActivityDefinition { path, line, reason } => {
            solstone_core_cli_copy::display_for(
                terminal_language(),
                "journal.cli.journal.facet.record.malformed.activity",
                [
                    ("path", &(path.display())),
                    ("line", &(line)),
                    ("reason", &(record_reason(reason))),
                ],
            )
        }
    }
}

pub(super) fn write(error: &FacetWriteError) -> String {
    use FacetWriteError as E;
    match error {
        E::TrustLock(e) => trust(e),
        E::Read(e) => store(e),
        E::FacetId(e) => facet_id(e),
        E::AlreadyExists { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.declaration.exists",
            [("path", &(path.display()))],
        ),
        E::DeclarationMissing { path } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.declaration.missing",
            [("path", &(path.display()))],
        ),
        E::DeclarationDamaged { detail } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.declaration.damaged",
            [("detail", &(detail))],
        ),
        E::DeclarationUnreadable { detail } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.declaration.unreadable",
            [("detail", &(detail))],
        ),
        E::DeclarationWrite(e) | E::EntityLinkWrite(e) | E::ContentWrite(e) => publication(e),
        E::EntityLinkRemoval(e) => path_detail(terminal_language(), e),
        E::NameRetired { name } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.name.retired",
            [("name", &(name))],
        ),
        E::RetiredFileDamaged { detail } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.retired.damaged",
            [("detail", &(retired(detail)))],
        ),
        E::RetiredEntryConflict { name } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.retired.conflict",
            [("name", &(name))],
        ),
        E::LastEnabledFacet { facet } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.minimum",
            [("facet", &(facet))],
        ),
        #[cfg(not(target_os = "ios"))]
        E::Trash(e) => trash(e),
        #[cfg(target_os = "ios")]
        E::Trash(e) => e.to_string(),
    }
}

pub(super) fn facet_id(error: &solstone_core_facets::FacetIdError) -> String {
    use solstone_core_facets::FacetIdError as E;
    match error {
        E::TrustLock(e) => trust(e),
        E::Store(e) => store(e),
        E::Write(e) => write(e),
        E::Random(solstone_core_journal_io::IdentifierMintError::Entropy(e)) => {
            solstone_core_cli_copy::display_for(
                terminal_language(),
                "journal.cli.journal.facet.record.id.entropy",
                [("error", &(e))],
            )
        }
        E::CollisionExhausted => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.id.exhausted",
            &[],
        ),
        E::DuplicateIdOnDisk { id } => solstone_core_cli_copy::display_for(
            terminal_language(),
            "journal.cli.journal.facet.record.id.duplicate",
            [("id", &(id))],
        ),
    }
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
    use solstone_core_facets::ObservationErrorSource as E;
    match source {
        E::Path(path) => path.display().to_string(),
        E::CapturedSnapshot => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.source.snapshot",
            &[],
        ),
    }
}

fn record_reason(reason: &'static str) -> String {
    match reason {
        "invalid JSON" => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.reason.json",
            &[],
        ),
        "expected an object" => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.reason.object",
            &[],
        ),
        "expected an object with nonblank string content" => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.reason.content",
            &[],
        ),
        "expected an object with nonblank string content or relation" => {
            solstone_core_cli_copy::render_for(
                terminal_language(),
                "journal.cli.journal.facet.record.reason.relation",
                &[],
            )
        }
        "id must be an unsigned integer" => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.reason.id",
            &[],
        ),
        "invalid history field" => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.reason.history",
            &[],
        ),
        "invalid retired field" => solstone_core_cli_copy::render_for(
            terminal_language(),
            "journal.cli.journal.facet.record.reason.retired",
            &[],
        ),
        other => other.to_owned(),
    }
}
