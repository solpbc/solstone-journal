// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{
    BodyManifestBinding, BodyObject, BodyValue, decode_body_manifest, parse, scan_body_manifest,
};

mod support;

use support::{
    assert_body_value_bitwise_eq, native_bundle_fixture, native_bundle_manifest_binding_cases,
};

fn expected_object(value: &Value) -> BodyObject {
    let encoded = serde_json::to_vec(value).expect("expected binding serializes");
    let BodyValue::Object(object) = parse(&encoded).expect("expected binding parses") else {
        panic!("expected binding is an object");
    };
    object
}

fn binding_for_fixture(case: &support::NativeBundleManifestBindingCase) -> BodyManifestBinding {
    BodyManifestBinding::new(
        case.body_bundle_sha256.clone(),
        case.import_id.clone(),
        case.source_type,
        case.source_hash.clone(),
        case.entry_count,
        case.days_affected.clone(),
        case.raw_retention,
    )
    .unwrap_or_else(|error| panic!("{} should bind: {error}", case.name))
}

#[test]
fn fixture_manifests_decode_to_their_checked_binding_oracles() {
    let fixture = native_bundle_fixture();
    let cases = native_bundle_manifest_binding_cases();
    assert_eq!(cases.len(), 4);

    for (index, case) in cases.iter().enumerate() {
        let bytes = serde_json::to_vec(&fixture["cases"][index]["manifest"])
            .expect("fixture manifest serializes");
        let binding = decode_body_manifest(&bytes, &case.import_id)
            .unwrap_or_else(|error| panic!("{} should decode: {error}", case.name));

        assert_eq!(binding.body_source_schema(), "solstone.body.bundle.v1");
        assert_eq!(binding.body_bundle_ref(), "body-bundle.json");
        assert_eq!(binding.body_bundle_sha256(), &case.body_bundle_sha256);
        assert_eq!(binding.import_id(), &case.import_id);
        assert_eq!(binding.source_type(), case.source_type);
        assert_eq!(binding.source_hash(), &case.source_hash);
        assert_eq!(binding.entry_count(), case.entry_count);
        assert_eq!(binding.days_affected(), case.days_affected);
        assert_eq!(binding.raw_retention(), case.raw_retention);
        assert_body_value_bitwise_eq(
            &BodyValue::Object(binding.to_body_object()),
            &BodyValue::Object(expected_object(&case.expected_manifest_binding)),
        );
    }
}

#[test]
fn decoded_fixture_binding_applies_like_direct_fixture_binding_and_keeps_extensions() {
    let fixture = native_bundle_fixture();
    let case = &native_bundle_manifest_binding_cases()[0];
    let bytes =
        serde_json::to_vec(&fixture["cases"][0]["manifest"]).expect("fixture manifest serializes");
    let scanned = scan_body_manifest(&bytes).expect("fixture manifest scans");
    let source = BodyValue::Object(scanned.object().clone());
    let decoded = decode_body_manifest(&bytes, &case.import_id).expect("fixture manifest decodes");
    let direct = binding_for_fixture(case);

    let decoded_result = decoded.apply_to(&source).expect("decoded binding applies");
    let direct_result = direct.apply_to(&source).expect("direct binding applies");
    assert_body_value_bitwise_eq(
        &BodyValue::Object(decoded_result.clone()),
        &BodyValue::Object(direct_result),
    );
    assert_eq!(
        decoded_result.get(
            &solstone_core_body_source::BodyString::from_code_points(
                "fixture_extension".bytes().map(u32::from).collect(),
            )
            .expect("extension key is valid"),
        ),
        scanned.object().get(
            &solstone_core_body_source::BodyString::from_code_points(
                "fixture_extension".bytes().map(u32::from).collect(),
            )
            .expect("extension key is valid"),
        )
    );
}
