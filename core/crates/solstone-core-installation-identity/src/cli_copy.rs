// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{IdentityError, RecordError};
use solstone_core_cli_copy::MessageDetail;

impl IdentityError {
    pub fn message_detail(self) -> MessageDetail {
        match self {
            IdentityError::Io { operation, source } => match operation {
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
                _ => format!("{operation}: {source}").into(),
            },
            IdentityError::InvalidInput(reason)
            | IdentityError::UnsafeState(reason)
            | IdentityError::AdmissionRefused(reason)
            | IdentityError::NotAdopted(reason) => match reason {
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
                _ => reason.into(),
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
                    [("field", &field)],
                ),
                RecordError::ChecksumMismatch => {
                    MessageDetail::message("journal.cli.depict.admission.record.checksum", [])
                }
            },
            IdentityError::Guard(error) => error.to_string().into(),
        }
    }
}
