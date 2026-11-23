// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{GuardError, IdentityError, PROGRAM_FOLDER_JOURNAL_REFUSAL, RecordError};
use solstone_core_cli_copy::MessageDetail;

impl IdentityError {
    pub fn message_detail(self) -> MessageDetail {
        self.message_detail_ref()
    }

    pub fn render(&self, locale: &str) -> String {
        self.message_detail_ref().render(locale)
    }

    pub fn message_detail_ref(&self) -> MessageDetail {
        match self {
            IdentityError::Io { operation, source } => match *operation {
                "canonicalize root" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.root",
                    [("error", &source)],
                ),
                "resolve LocalAppData known folder" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.local.folder",
                    [("error", &source)],
                ),
                "open owner directory" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.owner.open",
                    [("error", &source)],
                ),
                "open storage directory" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.storage.open",
                    [("error", &source)],
                ),
                "open storage file" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.storage.file",
                    [("error", &source)],
                ),
                "query storage attributes" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.storage.attrs",
                    [("error", &source)],
                ),
                "query storage identity" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.storage.identity",
                    [("error", &source)],
                ),
                "lock identity storage" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.storage.lock",
                    [("error", &source)],
                ),
                "clone adoption marker" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.marker.clone",
                    [("error", &source)],
                ),
                "read adoption marker" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.marker.read",
                    [("error", &source)],
                ),
                "stat identity record" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.record.stat",
                    [("error", &source)],
                ),
                "read identity record" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.record.read",
                    [("error", &source)],
                ),
                "stat adoption marker" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.marker.stat",
                    [("error", &source)],
                ),
                "read namespace directory" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.io.namespace.read",
                    [("error", &source)],
                ),
                "open home directory" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.io.open.home.directory",
                    [("error", &source)],
                ),
                "stat storage directory" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.io.stat.storage.directory",
                    [("error", &source)],
                ),
                "stat storage file" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.io.stat.storage.file",
                    [("error", &source)],
                ),
                "clone namespace registry" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.io.clone.namespace.registry",
                    [("error", &source)],
                ),
                "open namespace registry" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.io.open.namespace.registry",
                    [("error", &source)],
                ),
                "read namespace registry" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.io.read.namespace.registry",
                    [("error", &source)],
                ),
                "clone namespace directory" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.io.clone.namespace.directory",
                    [("error", &source)],
                ),
                "open namespace directory" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.io.open.namespace.directory",
                    [("error", &source)],
                ),
                "create setup home directory" => MessageDetail::message(
                    "journal.cli.setup.identity.io.create.home",
                    [("error", &source)],
                ),
                _ => format!("{operation}: {source}").into(),
            },
            IdentityError::InvalidInput(reason)
            | IdentityError::UnsafeState(reason)
            | IdentityError::AdmissionRefused(reason)
            | IdentityError::NotAdopted(reason) => match *reason {
                PROGRAM_FOLDER_JOURNAL_REFUSAL => {
                    MessageDetail::window("journal.setup.identity.program.folder", [])
                }
                "namespace record is missing" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.record.missing",
                    [],
                ),
                "installation identity record is prepared" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.prepared", [])
                }
                "installation identity record is tombstoned" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.tombstoned", [])
                }
                "record does not bind the requested root" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.root.mismatch",
                    [],
                ),
                "owner coordinator is poisoned" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.owner.poisoned",
                    [],
                ),
                "storage path is a reparse point" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.storage.reparse",
                    [],
                ),
                "storage path is not a directory" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.storage.directory",
                    [],
                ),
                "storage path is not a regular file" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.storage.file", [])
                }
                "adoption marker has invalid content" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.marker.content",
                    [],
                ),
                "identity record exceeds bounds" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.record.size", [])
                }
                "adoption marker has invalid size" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.marker.size", [])
                }
                "orphan adoption marker" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.marker.orphan",
                    [],
                ),
                "committed record has no adoption marker" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.marker.missing",
                    [],
                ),
                "unknown namespace child" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.namespace.child",
                    [],
                ),
                "LocalAppData known folder is not valid UTF-16" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.local.encoding",
                    [],
                ),
                "Windows path contains an interior NUL" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.path.nul", [])
                }
                "Windows path is not valid UTF-16" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.path.encoding",
                    [],
                ),
                "Windows path escapes its absolute root" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.path.escape", [])
                }
                "path exceeds 4096 bytes" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.path.size", [])
                }
                "Windows verbatim and device paths are not accepted" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.path.device", [])
                }
                "Windows UNC path has no server" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.path.server", [])
                }
                "Windows UNC path has no share" => {
                    MessageDetail::message("journal.cli.depict.admission.identity.path.share", [])
                }
                "Windows path must be fully qualified" => MessageDetail::message(
                    "journal.cli.depict.admission.identity.path.relative",
                    [],
                ),
                "home must be absolute" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.home.must.be.absolute",
                    [],
                ),
                "storage directory is not owned by the current user" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.storage.directory.is.not.owned.by.the.current.user",
                    [],
                ),
                "storage directory does not have mode 0700" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.storage.directory.does.not.have.private.mode",
                    [],
                ),
                "storage file is not owned by the current user" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.storage.file.is.not.owned.by.the.current.user",
                    [],
                ),
                "storage file does not have mode 0600" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.storage.file.does.not.have.private.mode",
                    [],
                ),
                "namespace entry is not lowercase hex" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.namespace.entry.is.not.lowercase.hex",
                    [],
                ),
                "namespace registry exceeds 1024 entries" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.namespace.registry.entries.limit",
                    [],
                ),
                "namespace registry exceeds 16 MiB" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.namespace.registry.size.limit",
                    [],
                ),
                "installation identity is unsupported for Android" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.installation.identity.is.unsupported.for.android",
                    [],
                ),
                "path must be absolute and NUL-free" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.path.must.be.absolute.and.nul.free",
                    [],
                ),
                "path escapes its absolute root" => MessageDetail::message(
                    "journal.cli.sol.skills.identity.path.escapes.its.absolute.root",
                    [],
                ),
                "existing wrapper or service artifacts are bound to a different installation" => {
                    MessageDetail::message("journal.cli.setup.identity.foreign.artifacts", [])
                }
                "artifact binding is malformed or ambiguous" => {
                    MessageDetail::message("journal.cli.setup.identity.uncertain.artifacts", [])
                }
                "HOME is not set" => {
                    MessageDetail::message("journal.cli.setup.identity.home.unset", [])
                }
                "PATH resolves outside the V2 installation" => {
                    MessageDetail::message("journal.cli.setup.identity.path.outside", [])
                }
                "Windows path has an odd byte count" => {
                    MessageDetail::message("journal.cli.setup.identity.windows.path.odd", [])
                }
                "adopted record has no adoption marker" => {
                    MessageDetail::message("journal.cli.setup.identity.adopted.no.marker", [])
                }
                "artifact guard does not match identity" => {
                    MessageDetail::message("journal.cli.setup.identity.guard.mismatch", [])
                }
                "artifact guard does not match the admitted root" => {
                    MessageDetail::message("journal.cli.setup.identity.guard.root.mismatch", [])
                }
                "artifact guard is not a complete matching identity" => {
                    MessageDetail::message("journal.cli.setup.identity.guard.incomplete", [])
                }
                "current executable is unavailable" => {
                    MessageDetail::message("journal.cli.setup.identity.executable.unavailable", [])
                }
                "effective journal path cannot be represented safely in managed wrappers" => {
                    MessageDetail::message("journal.cli.setup.identity.wrapper.journal.unsafe", [])
                }
                "existing artifacts have no valid bootstrap evidence" => {
                    MessageDetail::message("journal.cli.setup.identity.no.bootstrap.evidence", [])
                }
                "existing wrapper or service artifacts are guarded, but no matching installation record exists in identity storage" => {
                    MessageDetail::message("journal.cli.setup.identity.guarded.no.record", [])
                }
                "finish journal setup before changing its folder" => {
                    MessageDetail::message("journal.cli.setup.identity.finish.setup.first", [])
                }
                "finish setup or recovery before changing the journal folder" => {
                    MessageDetail::message("journal.cli.setup.identity.finish.recovery.first", [])
                }
                "generation is not canonical decimal" => {
                    MessageDetail::message("journal.cli.setup.identity.generation.decimal", [])
                }
                "generation is out of range" => {
                    MessageDetail::message("journal.cli.setup.identity.generation.range", [])
                }
                "generation must be nonzero" => {
                    MessageDetail::message("journal.cli.setup.identity.generation.nonzero", [])
                }
                "generation overflow" => {
                    MessageDetail::message("journal.cli.setup.identity.generation.overflow", [])
                }
                "installation identity does not exist" => {
                    MessageDetail::message("journal.cli.setup.identity.not.found", [])
                }
                "installation provider base has no application directory" => {
                    MessageDetail::message("journal.cli.setup.identity.provider.app.dir", [])
                }
                "installation provider base has no parent" => {
                    MessageDetail::message("journal.cli.setup.identity.provider.parent", [])
                }
                "installation root is unavailable" => {
                    MessageDetail::message("journal.cli.setup.identity.root.unavailable", [])
                }
                "legacy artifacts need a valid provider-less schema-v1 manifest" => {
                    MessageDetail::message("journal.cli.setup.identity.legacy.needs.manifest", [])
                }
                "legacy manifest evidence is malformed or unreadable" => {
                    MessageDetail::message("journal.cli.setup.identity.legacy.manifest.invalid", [])
                }
                "malformed record stage" => {
                    MessageDetail::message("journal.cli.setup.identity.record.stage.malformed", [])
                }
                "namespace record does not match its root namespace" => {
                    MessageDetail::message("journal.cli.setup.identity.namespace.mismatch", [])
                }
                "owner admission root changed during lock" => {
                    MessageDetail::message("journal.cli.setup.identity.owner.root.changed", [])
                }
                "owner platform does not match this Windows build" => MessageDetail::message(
                    "journal.cli.setup.identity.windows.platform.mismatch",
                    [],
                ),
                "prepared identity cannot be uninstalled" => {
                    MessageDetail::message("journal.cli.setup.identity.prepared.uninstall", [])
                }
                "stale record stage does not match its name" => {
                    MessageDetail::message("journal.cli.setup.identity.record.stage.stale", [])
                }
                "the installation changed while changing its journal folder; try again" => {
                    MessageDetail::message("journal.cli.setup.identity.changed.retry", [])
                }
                "tombstoned record has no adoption marker" => {
                    MessageDetail::message("journal.cli.setup.identity.tombstoned.no.marker", [])
                }
                _ => (*reason).into(),
            },
            IdentityError::Record(error) => match error {
                RecordError::NotAscii => {
                    MessageDetail::message("journal.cli.depict.admission.record.ascii", [])
                }
                RecordError::TooLarge => {
                    MessageDetail::message("journal.cli.depict.admission.record.size", [])
                }
                RecordError::WrongLineCount => {
                    MessageDetail::message("journal.cli.depict.admission.record.lines", [])
                }
                RecordError::InvalidField(field) => MessageDetail::message(
                    "journal.cli.depict.admission.record.field",
                    [("field", field)],
                ),
                RecordError::ChecksumMismatch => {
                    MessageDetail::message("journal.cli.depict.admission.record.checksum", [])
                }
            },
            IdentityError::Guard(error) => match error {
                GuardError::Missing => {
                    MessageDetail::message("journal.cli.setup.identity.guard.missing", [])
                }
                GuardError::Duplicate(field) => MessageDetail::message(
                    "journal.cli.setup.identity.guard.duplicate",
                    [("field", field)],
                ),
                GuardError::Invalid(field) => MessageDetail::message(
                    "journal.cli.setup.identity.guard.invalid",
                    [("field", field)],
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_cli_copy::ENGLISH;

    #[test]
    fn program_folder_and_guard_errors_match_english_display() {
        let program_folder = IdentityError::AdmissionRefused(PROGRAM_FOLDER_JOURNAL_REFUSAL);
        assert_eq!(program_folder.render(ENGLISH), program_folder.to_string());
        for error in [
            IdentityError::Guard(GuardError::Missing),
            IdentityError::Guard(GuardError::Duplicate("namespace")),
            IdentityError::Guard(GuardError::Invalid("generation")),
            IdentityError::Io {
                operation: "create setup home directory",
                source: std::io::Error::other("denied"),
            },
            IdentityError::AdmissionRefused("PATH resolves outside the V2 installation"),
            IdentityError::NotAdopted("installation identity does not exist"),
            IdentityError::UnsafeState("tombstoned record has no adoption marker"),
        ] {
            assert_eq!(error.render(ENGLISH), error.to_string());
        }
    }
}
