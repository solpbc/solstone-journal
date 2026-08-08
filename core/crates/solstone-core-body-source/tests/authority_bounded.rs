// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{
    AuthorityError, BundleId, DirectoryObservation, authorize_native_bundle, decode_body_manifest,
};

const MAX_MANIFEST_BYTES: usize = 1_048_576;
const MAX_BUNDLE: &str = "body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
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

fn assert_bounded_redacted(error: &AuthorityError, sentinel: &str) {
    let display = error.to_string();
    assert_eq!(display, format!("{error:?}"));
    assert!(display.len() <= 160);
    assert!(Error::source(error).is_none());
    assert!(!display.contains(sentinel));
}

#[test]
fn invalid_directory_errors_are_bounded_and_redact_megabyte_name_bytes() {
    let sentinel = "body-authority-directory-sentinel";
    let name = format!("body-{}", sentinel.repeat(50_000));
    let Err(error) = authorize_native_bundle(DirectoryObservation {
        name: name.as_bytes(),
        envelope_present: false,
        ledger_present: false,
        manifest: None,
    }) else {
        panic!("invalid directory should refuse");
    };
    assert_eq!(error, AuthorityError::InvalidDirectory);
    assert_bounded_redacted(&error, sentinel);
    assert!(error.to_string().contains("<invalid>"));
}

#[test]
fn invalid_manifest_errors_remain_bounded_redacting_and_bound_to_expected_directory() {
    let sentinel = "body-authority-manifest-sentinel";
    let large = sentinel.repeat(15_000);
    let too_large = sentinel.repeat(MAX_MANIFEST_BYTES / sentinel.len() + 1);
    let malformed = format!(r#"{{"payload":"{large}""#);
    let duplicate = format!(r#"{{"import_id":"{large}","import_id":"{large}"}}"#);
    let unknown = format!(r#"{{"body_{large}":"{large}"}}"#);
    let missing = format!(r#"{{"ordinary":"{large}"}}"#);
    let wrong_type = format!(r#"{{"body_source_schema":null,"ordinary":"{large}"}}"#);
    let invalid = valid_manifest().replace(HASH, &large);
    let incompatible = valid_manifest().replace(MAX_BUNDLE, "body-00000000000000000000000000");

    for input in [
        too_large,
        malformed,
        duplicate,
        unknown,
        missing,
        wrong_type,
        invalid,
        incompatible,
    ] {
        let Err(expected) = decode_body_manifest(input.as_bytes(), &bundle()) else {
            panic!("manifest input should refuse");
        };
        let Err(error) = authorize_native_bundle(DirectoryObservation {
            name: MAX_BUNDLE.as_bytes(),
            envelope_present: true,
            ledger_present: true,
            manifest: Some(input.as_bytes()),
        }) else {
            panic!("authority should refuse");
        };
        let AuthorityError::InvalidManifest(actual) = &error else {
            panic!("authority should wrap manifest failure");
        };
        assert_eq!(actual.code(), expected.code());
        assert_eq!(actual.field(), expected.field());
        assert_eq!(actual.bundle(), expected.bundle());
        assert_eq!(error.to_string(), expected.to_string());
        assert_bounded_redacted(&error, sentinel);
        assert!(error.to_string().contains(MAX_BUNDLE));
    }
}
