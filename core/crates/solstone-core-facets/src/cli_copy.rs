// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Facet diagnostics shared by command-line and owner-response presenters.

use crate::{FacetStoreError, FacetTrustLockError, FacetWriteError, RetiredRecordError};
use solstone_core_journal_io::cli_diagnostics::{
    atomic_message, lock_detail, path_detail, read_detail,
};

pub fn publication(locale: &str, error: &solstone_core_journal_io::AtomicWriteError) -> String {
    atomic_message(error).render(locale)
}

pub fn trust(locale: &str, error: &FacetTrustLockError) -> String {
    match error {
        FacetTrustLockError::Path(e) => path_detail(locale, e),
        FacetTrustLockError::Lock(e) => lock_detail(locale, e),
    }
}

pub fn retired(locale: &str, error: &RetiredRecordError) -> String {
    use RetiredRecordError as E;
    match error {
        E::NotObject => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.not.object",
            &[],
        ),
        E::MissingNames => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.missing.names",
            &[],
        ),
        E::External(message) => message.clone(),
    }
}

pub fn store(locale: &str, error: &FacetStoreError) -> String {
    use FacetStoreError as E;
    match error {
        E::Path(e) => path_detail(locale, e),
        E::Read(e) => read_detail(locale, e),
        E::DeclarationNotObject { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.declaration.invalid",
            [("path", &(path.display()))],
        ),
        E::EntityLinkNotObject { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.link.invalid",
            [("path", &(path.display()))],
        ),
        E::CorruptCompletionMarker { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.marker.invalid",
            [("path", &(path.display()))],
        ),
        E::MalformedObservation {
            source,
            line,
            reason,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.malformed.observation",
            [
                ("source", &(observation_source(locale, source))),
                ("line", &(line)),
                ("reason", &(record_reason(locale, reason))),
            ],
        ),
        E::MalformedActivityDefinition { path, line, reason } => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.facet.record.malformed.activity",
                [
                    ("path", &(path.display())),
                    ("line", &(line)),
                    ("reason", &(record_reason(locale, reason))),
                ],
            )
        }
    }
}

pub fn write(locale: &str, error: &FacetWriteError) -> String {
    use FacetWriteError as E;
    match error {
        E::TrustLock(e) => trust(locale, e),
        E::Read(e) => store(locale, e),
        E::FacetId(e) => facet_id(locale, e),
        E::AlreadyExists { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.declaration.exists",
            [("path", &(path.display()))],
        ),
        E::DeclarationMissing { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.declaration.missing",
            [("path", &(path.display()))],
        ),
        E::DeclarationDamaged { detail } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.declaration.damaged",
            [("detail", &(detail))],
        ),
        E::DeclarationUnreadable { detail } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.declaration.unreadable",
            [("detail", &(detail))],
        ),
        E::DeclarationWrite(e) | E::EntityLinkWrite(e) | E::ContentWrite(e) => {
            publication(locale, e)
        }
        E::EntityLinkRemoval(e) => path_detail(locale, e),
        E::NameRetired { name } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.name.retired",
            [("name", &(name))],
        ),
        E::RetiredFileDamaged { detail } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.retired.damaged",
            [("detail", &(retired(locale, detail)))],
        ),
        E::RetiredEntryConflict { name } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.retired.conflict",
            [("name", &(name))],
        ),
        E::LastEnabledFacet { facet } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.minimum",
            [("facet", &(facet))],
        ),
        #[cfg(not(target_os = "ios"))]
        E::Trash(e) => e.message_for(locale),
        #[cfg(target_os = "ios")]
        E::Trash(e) => e.to_string(),
    }
}

pub fn facet_id(locale: &str, error: &crate::FacetIdError) -> String {
    use crate::FacetIdError as E;
    match error {
        E::TrustLock(e) => trust(locale, e),
        E::Store(e) => store(locale, e),
        E::Write(e) => write(locale, e),
        E::Random(solstone_core_journal_io::IdentifierMintError::Entropy(e)) => {
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.journal.facet.record.id.entropy",
                [("error", &(e))],
            )
        }
        E::CollisionExhausted => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.id.exhausted",
            &[],
        ),
        E::DuplicateIdOnDisk { id } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.journal.facet.record.id.duplicate",
            [("id", &(id))],
        ),
    }
}

pub fn observation_source(locale: &str, source: &crate::ObservationErrorSource) -> String {
    use crate::ObservationErrorSource as E;
    match source {
        E::Path(path) => path.display().to_string(),
        E::CapturedSnapshot => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.source.snapshot",
            &[],
        ),
    }
}

pub fn record_reason(locale: &str, reason: &'static str) -> String {
    match reason {
        "invalid JSON" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.reason.json",
            &[],
        ),
        "expected an object" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.reason.object",
            &[],
        ),
        "expected an object with nonblank string content" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.reason.content",
            &[],
        ),
        "expected an object with nonblank string content or relation" => {
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.journal.facet.record.reason.relation",
                &[],
            )
        }
        "id must be an unsigned integer" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.reason.id",
            &[],
        ),
        "invalid history field" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.reason.history",
            &[],
        ),
        "invalid retired field" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.journal.facet.record.reason.retired",
            &[],
        ),
        other => other.to_owned(),
    }
}
