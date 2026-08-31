// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Managed-log logical-field admission policy.

#![allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]

use std::fmt;

/// Why a managed-log logical field is not admissible.
///
/// Variant order is evaluation order. First match wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogicalFieldAdmissionReason {
    /// The field is empty.
    Empty,
    /// The field contains a control character.
    Control,
    /// The field is longer than 512 UTF-8 bytes.
    TooLong,
}

impl fmt::Display for LogicalFieldAdmissionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "the field is empty",
            Self::Control => "the field contains a control character",
            Self::TooLong => "the field is longer than 512 UTF-8 bytes",
        })
    }
}

/// Codes 1–3 of the logical-field policy, in precedence order.
pub(crate) fn check_logical_field(candidate: &str) -> Result<(), LogicalFieldAdmissionReason> {
    if candidate.is_empty() {
        return Err(LogicalFieldAdmissionReason::Empty);
    }
    if candidate.chars().any(|character| character.is_control()) {
        return Err(LogicalFieldAdmissionReason::Control);
    }
    if candidate.len() > 512 {
        return Err(LogicalFieldAdmissionReason::TooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_field_policy_rejects_in_precedence_order_and_bounds_utf8_bytes() {
        assert_eq!(
            check_logical_field(""),
            Err(LogicalFieldAdmissionReason::Empty)
        );
        assert_eq!(
            check_logical_field(&format!("{}\0", "a".repeat(512))),
            Err(LogicalFieldAdmissionReason::Control)
        );
        for codepoint in (0x00..=0x1f).chain([0x7f]).chain(0x80..=0x9f) {
            let candidate = format!("a{}b", char::from_u32(codepoint).unwrap());
            assert_eq!(
                check_logical_field(&candidate),
                Err(LogicalFieldAdmissionReason::Control),
                "U+{codepoint:04X}"
            );
        }

        let boundary = "é".repeat(256);
        assert_eq!(boundary.len(), 512);
        assert_eq!(check_logical_field(&boundary), Ok(()));
        let overlong = format!("{boundary}a");
        assert_eq!(overlong.len(), 513);
        assert_eq!(
            check_logical_field(&overlong),
            Err(LogicalFieldAdmissionReason::TooLong)
        );

        for candidate in [
            "maintenance:backup:run",
            "embedded/slash",
            r"embedded\backslash",
            "CON",
            "trailing.",
            "trailing ",
        ] {
            assert_eq!(check_logical_field(candidate), Ok(()), "{candidate:?}");
        }
    }
}
