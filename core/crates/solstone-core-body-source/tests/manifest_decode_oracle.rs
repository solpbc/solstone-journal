// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDigest, BodyManifestBinding, BodyRawRetention, BodySourceFamily, BodySourceHash, BundleId,
    ManifestBindingError, ManifestBindingErrorCode, ManifestBindingErrorField, ManifestScanError,
    decode_body_manifest, scan_body_manifest,
};

mod support;

const BUNDLE: &str = "body-00000000000000000000000000";
const DIGEST: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn bundle() -> BundleId {
    BundleId::from_bytes(BUNDLE.as_bytes()).expect("test bundle is valid")
}

fn valid_manifest() -> String {
    format!(
        r#"{{"body_source_schema":"solstone.body.bundle.v1","body_bundle_ref":"body-bundle.json","body_bundle_sha256":"{DIGEST}","import_id":"{BUNDLE}","source_type":"apple_health","source_hash":"{HASH}","entry_count":0,"days_affected":[],"raw_retention":"discard"}}"#
    )
}

fn zero_binding() -> BodyManifestBinding {
    BodyManifestBinding::new(
        BodyDigest::from_bytes(DIGEST.as_bytes()).expect("digest is valid"),
        bundle(),
        BodySourceFamily::AppleHealth,
        BodySourceHash::from_bytes_for_family(HASH.as_bytes(), &BodySourceFamily::AppleHealth)
            .expect("hash is valid"),
        0,
        vec![],
        BodyRawRetention::Discard,
    )
    .expect("values bind")
}

fn assert_same_binding(actual: &BodyManifestBinding, expected: &BodyManifestBinding) {
    assert_eq!(actual.body_source_schema(), expected.body_source_schema());
    assert_eq!(actual.body_bundle_ref(), expected.body_bundle_ref());
    assert_eq!(actual.body_bundle_sha256(), expected.body_bundle_sha256());
    assert_eq!(actual.import_id(), expected.import_id());
    assert_eq!(actual.source_type(), expected.source_type());
    assert_eq!(actual.source_hash(), expected.source_hash());
    assert_eq!(actual.entry_count(), expected.entry_count());
    assert_eq!(actual.days_affected(), expected.days_affected());
    assert_eq!(actual.raw_retention(), expected.raw_retention());
}

fn assert_error(
    result: Result<BodyManifestBinding, ManifestBindingError>,
    code: ManifestBindingErrorCode,
    field: ManifestBindingErrorField,
) {
    let Err(error) = result else {
        panic!("decoder should refuse");
    };
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
    assert_eq!(error.bundle(), &bundle());
}

#[test]
fn too_long_integer_is_a_malformed_public_manifest() {
    let integer = format!("1{}", "0".repeat(4300));
    let input =
        valid_manifest().replace("\"entry_count\":0", &format!("\"entry_count\":{integer}"));
    assert_error(
        decode_body_manifest(input.as_bytes(), &bundle()),
        ManifestBindingErrorCode::MalformedManifest,
        ManifestBindingErrorField::Manifest,
    );
}

#[test]
fn lexical_negative_zero_composes_to_a_zero_entry_binding() {
    let input = valid_manifest().replace("\"entry_count\":0", "\"entry_count\":-0");
    let binding = decode_body_manifest(input.as_bytes(), &bundle()).expect("negative zero decodes");
    assert_eq!(binding.entry_count(), 0);
    assert_eq!(binding.days_affected(), []);
}

#[test]
fn public_decoder_matches_a_public_facts_and_constructor_oracle() {
    let clean = valid_manifest();
    let oversized = vec![b' '; 1_048_577];
    let cases = [
        (
            clean.as_bytes(),
            Some((
                ManifestBindingErrorCode::MissingField,
                ManifestBindingErrorField::Manifest,
            )),
        ),
        (
            br#"{"import_id":1,"import_id":2}"#.as_slice(),
            Some((
                ManifestBindingErrorCode::DuplicateField,
                ManifestBindingErrorField::ImportId,
            )),
        ),
        (
            br#"{"body_x":null}"#,
            Some((
                ManifestBindingErrorCode::UnknownField,
                ManifestBindingErrorField::Manifest,
            )),
        ),
        (
            br#"{"body_x":null"#,
            Some((
                ManifestBindingErrorCode::MalformedManifest,
                ManifestBindingErrorField::Manifest,
            )),
        ),
        (
            oversized.as_slice(),
            Some((
                ManifestBindingErrorCode::InputTooLarge,
                ManifestBindingErrorField::Manifest,
            )),
        ),
        (
            br#"{}"#,
            Some((
                ManifestBindingErrorCode::MissingField,
                ManifestBindingErrorField::BodySourceSchema,
            )),
        ),
    ];

    assert_same_binding(
        &decode_body_manifest(clean.as_bytes(), &bundle()).expect("clean input decodes"),
        &zero_binding(),
    );
    for (input, expected) in cases.into_iter().skip(1) {
        let scanned = scan_body_manifest(input);
        let (code, field) = match scanned {
            Err(ManifestScanError::InputTooLarge) => (
                ManifestBindingErrorCode::InputTooLarge,
                ManifestBindingErrorField::Manifest,
            ),
            Err(ManifestScanError::MalformedManifest) => (
                ManifestBindingErrorCode::MalformedManifest,
                ManifestBindingErrorField::Manifest,
            ),
            Ok(scanned) if !scanned.duplicated_known_keys().is_empty() => (
                ManifestBindingErrorCode::DuplicateField,
                expected.expect("duplicate oracle").1,
            ),
            Ok(scanned) if scanned.has_unknown_body_prefixed_key() => (
                ManifestBindingErrorCode::UnknownField,
                ManifestBindingErrorField::Manifest,
            ),
            Ok(_) => expected.expect("projection oracle"),
        };
        assert_error(decode_body_manifest(input, &bundle()), code, field);
    }
}
