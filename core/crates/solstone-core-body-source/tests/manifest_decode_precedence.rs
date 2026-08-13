// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BundleId, ManifestBindingError, ManifestBindingErrorCode, ManifestBindingErrorField,
    ManifestKnownKey, decode_body_manifest,
};

use crate::support;

use support::MIN_BUNDLE;

const DIGEST: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn bundle() -> BundleId {
    BundleId::from_bytes(MIN_BUNDLE.as_bytes()).expect("test bundle is valid")
}

fn valid_manifest(extra: &str) -> String {
    format!(
        r#"{{"body_source_schema":"solstone.body.bundle.v1","body_bundle_ref":"body-bundle.json","body_bundle_sha256":"{DIGEST}","import_id":"{MIN_BUNDLE}","source_type":"apple_health","source_hash":"{HASH}","entry_count":0,"days_affected":[],"raw_retention":"discard"{extra}}}"#
    )
}

fn error(input: &[u8]) -> ManifestBindingError {
    let Err(error) = decode_body_manifest(input, &bundle()) else {
        panic!("decoder should refuse");
    };
    error
}

fn field_for(key: ManifestKnownKey) -> ManifestBindingErrorField {
    match key {
        ManifestKnownKey::BodySourceSchema => ManifestBindingErrorField::BodySourceSchema,
        ManifestKnownKey::BodyBundleRef => ManifestBindingErrorField::BodyBundleRef,
        ManifestKnownKey::BodyBundleSha256 => ManifestBindingErrorField::BodyBundleSha256,
        ManifestKnownKey::ImportId => ManifestBindingErrorField::ImportId,
        ManifestKnownKey::SourceType => ManifestBindingErrorField::SourceType,
        ManifestKnownKey::SourceHash => ManifestBindingErrorField::SourceHash,
        ManifestKnownKey::EntryCount => ManifestBindingErrorField::EntryCount,
        ManifestKnownKey::DaysAffected => ManifestBindingErrorField::DaysAffected,
        ManifestKnownKey::RawRetention => ManifestBindingErrorField::RawRetention,
    }
}

fn escaped(key: &str) -> String {
    key.bytes().map(|byte| format!(r"\u{byte:04x}")).collect()
}

#[test]
fn known_duplicates_map_to_their_canonical_binding_fields() {
    let keys = [
        ManifestKnownKey::BodySourceSchema,
        ManifestKnownKey::BodyBundleRef,
        ManifestKnownKey::BodyBundleSha256,
        ManifestKnownKey::ImportId,
        ManifestKnownKey::SourceType,
        ManifestKnownKey::SourceHash,
        ManifestKnownKey::EntryCount,
        ManifestKnownKey::DaysAffected,
        ManifestKnownKey::RawRetention,
    ];
    for (index, key) in keys.into_iter().enumerate() {
        let spelling = key.as_str();
        let other = if index % 2 == 0 {
            escaped(spelling)
        } else {
            spelling.to_owned()
        };
        let input = format!(r#"{{"{spelling}":null,"{other}":null}}"#);
        let actual = error(input.as_bytes());
        assert_eq!(actual.code(), ManifestBindingErrorCode::DuplicateField);
        assert_eq!(actual.field(), field_for(key));
    }
}

#[test]
fn unknown_prefix_boundaries_and_unrelated_content_follow_top_level_rules() {
    for key in ["body", "Body_", " body_", "xbody_"] {
        let input = valid_manifest(&format!(r#", "{key}": null"#));
        assert!(
            decode_body_manifest(input.as_bytes(), &bundle()).is_ok(),
            "{key}"
        );
    }
    for input in [
        br#"{"body_":null}"#.as_slice(),
        br#"{"body_x":null}"#,
        br#"{"bo\u0064y_x":null}"#,
        br#"{"body_\ud800":null}"#,
    ] {
        let actual = error(input);
        assert_eq!(actual.code(), ManifestBindingErrorCode::UnknownField);
        assert_eq!(actual.field(), ManifestBindingErrorField::Manifest);
    }

    for extra in [
        r#", "ordinary": 1, "ordinary": 2"#,
        r#", "nested": {"body_x": true, "ordinary": 1, "ordinary": 2}"#,
        r#", "ordinary": "body_x""#,
    ] {
        assert!(decode_body_manifest(valid_manifest(extra).as_bytes(), &bundle()).is_ok());
    }
}

#[test]
fn decoder_precedence_is_scan_then_duplicate_then_unknown_then_projection() {
    let mut oversized = vec![b'{'; 1_048_577];
    oversized[0] = b'{';
    let actual = error(&oversized);
    assert_eq!(actual.code(), ManifestBindingErrorCode::InputTooLarge);
    assert_eq!(actual.field(), ManifestBindingErrorField::Manifest);

    let actual = error(br#"{"body_x":null,"import_id":1,"#);
    assert_eq!(actual.code(), ManifestBindingErrorCode::MalformedManifest);

    let actual = error(
        br#"{"raw_retention":1,"raw_retention":2,"body_source_schema":1,"body_source_schema":2}"#,
    );
    assert_eq!(actual.code(), ManifestBindingErrorCode::DuplicateField);
    assert_eq!(actual.field(), ManifestBindingErrorField::BodySourceSchema);

    let actual = error(br#"{"body_x":null,"import_id":1,"import_id":2}"#);
    assert_eq!(actual.code(), ManifestBindingErrorCode::DuplicateField);
    assert_eq!(actual.field(), ManifestBindingErrorField::ImportId);

    let actual = error(br#"{"body_x":null,"body_source_schema":null}"#);
    assert_eq!(actual.code(), ManifestBindingErrorCode::UnknownField);
    assert_eq!(actual.field(), ManifestBindingErrorField::Manifest);

    let phase_two =
        valid_manifest(r#", "source_type":"oura_api", "raw_retention":"retain_complete""#);
    let duplicate_phase_two = format!(
        r#"{},"import_id":"{MIN_BUNDLE}"}}"#,
        &phase_two[..phase_two.len() - 1]
    );
    let actual = error(duplicate_phase_two.as_bytes());
    assert_eq!(actual.code(), ManifestBindingErrorCode::DuplicateField);
    assert_eq!(actual.field(), ManifestBindingErrorField::ImportId);
}
