// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;

use solstone_core_cli_copy::MessageDetail;
use solstone_core_installation_identity::{GuardError, IdentityError};

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
    io::Error::new(io::ErrorKind::PermissionDenied, error.message_detail())
}
