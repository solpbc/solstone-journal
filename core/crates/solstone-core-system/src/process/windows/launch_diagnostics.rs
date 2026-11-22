// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;

use solstone_core_cli_copy::MessageDetail;
use solstone_core_installation_identity::{GuardError, IdentityError, RecordError};

use super::super::LaunchError;

pub(super) fn admission(error: io::Error) -> LaunchError {
    let reason = match error.downcast::<MessageDetail>() {
        Ok(detail) => detail,
        Err(external) => external.to_string().into(),
    };
    LaunchError::Admission(reason)
}

// Serde erases a custom Deserialize error to text before adding its position.
// Recover only our exact ProcessBirth diagnostic, on this typed JSON boundary.
pub(super) fn decode_error(error: serde_json::Error) -> io::Error {
    use super::super::common::MALFORMED_LINUX_BOOT_ID;

    if error.is_data() {
        let line = error.line();
        let column = error.column();
        let detail = if line == 0 && error.to_string() == MALFORMED_LINUX_BOOT_ID {
            Some(MessageDetail::message(
                "journal.cli.depict.admission.birth.invalid",
                [],
            ))
        } else if line > 0
            && error.to_string()
                == format!("{MALFORMED_LINUX_BOOT_ID} at line {line} column {column}")
        {
            Some(MessageDetail::message(
                "journal.cli.depict.admission.birth.position",
                [("line", &line), ("column", &column)],
            ))
        } else {
            None
        };
        if let Some(detail) = detail {
            return io::Error::new(io::ErrorKind::InvalidData, detail);
        }
    }
    error.into()
}

pub(super) fn guard_refusal(error: GuardError) -> io::Error {
    let detail = match error {
        GuardError::Missing => {
            MessageDetail::message("journal.cli.depict.admission.guard.missing", [])
        }
        GuardError::Invalid(field) => MessageDetail::message(
            "journal.cli.depict.admission.guard.invalid",
            [("field", &field)],
        ),
        GuardError::Duplicate(field) => MessageDetail::message(
            "journal.cli.depict.admission.guard.duplicate",
            [("field", &field)],
        ),
    };
    io::Error::new(io::ErrorKind::PermissionDenied, detail)
}

pub(super) fn identity_refusal(error: IdentityError) -> io::Error {
    let detail = match error {
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
            _ => format!("{operation}: {source}").into(),
        },
        IdentityError::InvalidInput(reason)
        | IdentityError::UnsafeState(reason)
        | IdentityError::AdmissionRefused(reason)
        | IdentityError::NotAdopted(reason) => match reason {
            "namespace record is missing" => {
                MessageDetail::message("journal.cli.depict.admission.identity.record.missing", [])
            }
            "installation identity record is prepared" => {
                MessageDetail::message("journal.cli.depict.admission.identity.prepared", [])
            }
            "installation identity record is tombstoned" => {
                MessageDetail::message("journal.cli.depict.admission.identity.tombstoned", [])
            }
            "record does not bind the requested root" => {
                MessageDetail::message("journal.cli.depict.admission.identity.root.mismatch", [])
            }
            "owner coordinator is poisoned" => {
                MessageDetail::message("journal.cli.depict.admission.identity.owner.poisoned", [])
            }
            "storage path is a reparse point" => {
                MessageDetail::message("journal.cli.depict.admission.identity.storage.reparse", [])
            }
            "storage path is not a directory" => MessageDetail::message(
                "journal.cli.depict.admission.identity.storage.directory",
                [],
            ),
            "storage path is not a regular file" => {
                MessageDetail::message("journal.cli.depict.admission.identity.storage.file", [])
            }
            "adoption marker has invalid content" => {
                MessageDetail::message("journal.cli.depict.admission.identity.marker.content", [])
            }
            "identity record exceeds bounds" => {
                MessageDetail::message("journal.cli.depict.admission.identity.record.size", [])
            }
            "adoption marker has invalid size" => {
                MessageDetail::message("journal.cli.depict.admission.identity.marker.size", [])
            }
            "orphan adoption marker" => {
                MessageDetail::message("journal.cli.depict.admission.identity.marker.orphan", [])
            }
            "committed record has no adoption marker" => {
                MessageDetail::message("journal.cli.depict.admission.identity.marker.missing", [])
            }
            "unknown namespace child" => {
                MessageDetail::message("journal.cli.depict.admission.identity.namespace.child", [])
            }
            "LocalAppData known folder is not valid UTF-16" => {
                MessageDetail::message("journal.cli.depict.admission.identity.local.encoding", [])
            }
            "Windows path contains an interior NUL" => {
                MessageDetail::message("journal.cli.depict.admission.identity.path.nul", [])
            }
            "Windows path is not valid UTF-16" => {
                MessageDetail::message("journal.cli.depict.admission.identity.path.encoding", [])
            }
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
            "Windows path must be fully qualified" => {
                MessageDetail::message("journal.cli.depict.admission.identity.path.relative", [])
            }
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
    };
    io::Error::new(io::ErrorKind::PermissionDenied, detail)
}
