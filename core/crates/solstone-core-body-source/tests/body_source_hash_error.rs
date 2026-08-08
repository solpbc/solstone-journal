// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::BodySourceHashError;

#[test]
fn body_source_hash_errors_are_bounded_redacting_and_source_free() {
    let error = BodySourceHashError::InvalidFormat;
    let display = error.to_string();
    let debug = format!("{error:?}");
    assert_eq!(display, "body-source-hash invalid_format: source_hash");
    assert_eq!(debug, "body-source-hash invalid_format: source_hash");
    assert!(display.len() <= 48 && debug.len() <= 48);
    assert!(Error::source(&error).is_none());
}
