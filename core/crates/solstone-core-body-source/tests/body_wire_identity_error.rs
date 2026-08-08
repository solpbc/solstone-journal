// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{BodyWireIdentityError, BodyWireIdentityField};

#[test]
fn body_wire_identity_errors_are_bounded_redacting_and_source_free() {
    for (field, expected) in [
        (
            BodyWireIdentityField::BundleId,
            "body-wire invalid_format: bundle_id",
        ),
        (
            BodyWireIdentityField::Digest,
            "body-wire invalid_format: digest",
        ),
    ] {
        let error = BodyWireIdentityError::InvalidFormat(field);
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert_eq!(display, expected);
        assert_eq!(debug, expected);
        assert!(display.len() <= 40 && debug.len() <= 40);
        assert!(Error::source(&error).is_none());
    }
}
