// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{BodySourcePolicyError, BodySourcePolicyField};

#[test]
fn body_source_policy_errors_are_bounded_redacting_and_source_free() {
    for (error, expected) in [
        (
            BodySourcePolicyError::InvalidFormat(BodySourcePolicyField::SourceFamily),
            "body-source-policy invalid_format: source_family",
        ),
        (
            BodySourcePolicyError::InvalidFormat(BodySourcePolicyField::RawRetention),
            "body-source-policy invalid_format: raw_retention",
        ),
        (
            BodySourcePolicyError::Incompatible(BodySourcePolicyField::RawRetention),
            "body-source-policy incompatible: raw_retention",
        ),
    ] {
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert_eq!(display, expected);
        assert_eq!(debug, expected);
        assert!(display.len() <= 64 && debug.len() <= 64);
        assert!(Error::source(&error).is_none());
    }
}
