// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{ManifestScanError, scan_body_manifest};

const MAX_MANIFEST_BYTES: usize = 1_048_576;

#[test]
fn manifest_scan_enforces_size_and_parse_boundaries() {
    let mut exact_limit = vec![b' '; MAX_MANIFEST_BYTES - 2];
    exact_limit.extend_from_slice(b"{}");
    assert_eq!(exact_limit.len(), MAX_MANIFEST_BYTES);
    assert!(scan_body_manifest(&exact_limit).is_ok());

    exact_limit.push(b' ');
    assert_eq!(
        scan_body_manifest(&exact_limit),
        Err(ManifestScanError::InputTooLarge)
    );
    assert_eq!(
        scan_body_manifest(b"{"),
        Err(ManifestScanError::MalformedManifest)
    );
    assert_eq!(
        scan_body_manifest(b"null"),
        Err(ManifestScanError::MalformedManifest)
    );
    assert_eq!(
        scan_body_manifest(&vec![b'{'; MAX_MANIFEST_BYTES + 1]),
        Err(ManifestScanError::InputTooLarge)
    );
}

#[test]
fn manifest_scan_errors_are_bounded_redacting_and_source_free() {
    for (error, expected) in [
        (
            ManifestScanError::InputTooLarge,
            "body-manifest-scan input_too_large: manifest",
        ),
        (
            ManifestScanError::MalformedManifest,
            "body-manifest-scan malformed_manifest: manifest",
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
