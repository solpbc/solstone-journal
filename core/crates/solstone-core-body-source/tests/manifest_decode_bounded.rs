// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{
    BundleId, ManifestBindingError, ManifestBindingErrorCode, ManifestBindingErrorField,
    decode_body_manifest,
};

mod support;

use support::MAX_BUNDLE;

const MAX_MANIFEST_BYTES: usize = 1_048_576;
const DIGEST: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn bundle() -> BundleId {
    BundleId::from_bytes(MAX_BUNDLE.as_bytes()).expect("maximum bundle is valid")
}

fn valid_manifest() -> String {
    format!(
        r#"{{"body_source_schema":"solstone.body.bundle.v1","body_bundle_ref":"body-bundle.json","body_bundle_sha256":"{DIGEST}","import_id":"{MAX_BUNDLE}","source_type":"apple_health","source_hash":"{HASH}","entry_count":0,"days_affected":[],"raw_retention":"discard"}}"#
    )
}

fn assert_bounded_redacted(error: &ManifestBindingError, sentinel: &str) {
    let display = error.to_string();
    assert_eq!(display, format!("{error:?}"));
    assert!(display.len() <= 160);
    assert!(Error::source(error).is_none());
    assert!(!display.contains(sentinel));
    assert!(display.contains(MAX_BUNDLE));
}

#[test]
fn decoder_accepts_the_exact_limit_and_rejects_one_byte_over() {
    let base = valid_manifest();
    let input = format!("{}{}", " ".repeat(MAX_MANIFEST_BYTES - base.len()), base);
    assert_eq!(input.len(), MAX_MANIFEST_BYTES);
    assert!(decode_body_manifest(input.as_bytes(), &bundle()).is_ok());

    let over = format!("{input} ");
    let Err(error) = decode_body_manifest(over.as_bytes(), &bundle()) else {
        panic!("over limit refuses");
    };
    assert_eq!(error.code(), ManifestBindingErrorCode::InputTooLarge);
    assert_eq!(error.field(), ManifestBindingErrorField::Manifest);
}

#[test]
fn megabyte_scale_errors_are_bounded_redacting_and_bound_to_the_expected_bundle() {
    let sentinel = "body-decode-private-sentinel";
    let large = sentinel.repeat(15_000);
    let malformed = format!(r#"{{"payload":"{large}""#);
    let duplicate = format!(r#"{{"import_id":"{large}","import_id":"{large}"}}"#);
    let unknown = format!(r#"{{"body_{large}":"{large}"}}"#);

    for (input, code) in [
        (malformed, ManifestBindingErrorCode::MalformedManifest),
        (duplicate, ManifestBindingErrorCode::DuplicateField),
        (unknown, ManifestBindingErrorCode::UnknownField),
    ] {
        assert!(
            input.len() <= MAX_MANIFEST_BYTES,
            "test input must reach decoder facts"
        );
        let Err(error) = decode_body_manifest(input.as_bytes(), &bundle()) else {
            panic!("input refuses");
        };
        assert_eq!(error.code(), code);
        assert_bounded_redacted(&error, sentinel);
    }
}
