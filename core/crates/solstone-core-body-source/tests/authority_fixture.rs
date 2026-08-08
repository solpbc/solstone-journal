// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{
    BodyObject, BodyValue, DirectoryObservation, authorize_native_bundle, parse,
};

mod support;

use support::{assert_body_value_bitwise_eq, native_bundle_directory_cases};

fn expected_object(value: &Value) -> BodyObject {
    let encoded = serde_json::to_vec(value).expect("expected binding serializes");
    let BodyValue::Object(object) = parse(&encoded).expect("expected binding parses") else {
        panic!("expected binding is an object");
    };
    object
}

#[test]
fn fixture_directory_cases_authorize_to_their_checked_manifest_bindings() {
    let cases = native_bundle_directory_cases();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let authority = authorize_native_bundle(DirectoryObservation {
            name: case.name.as_bytes(),
            envelope_present: true,
            ledger_present: true,
            manifest: Some(&case.manifest_bytes),
        })
        .unwrap_or_else(|error| panic!("{} should authorize: {error}", case.name));
        assert_eq!(authority.id(), &case.expected_import_id, "{}", case.name);
        assert_eq!(
            authority.id(),
            authority.binding().import_id(),
            "{}",
            case.name
        );
        assert_body_value_bitwise_eq(
            &BodyValue::Object(authority.binding().to_body_object()),
            &BodyValue::Object(expected_object(&case.expected_manifest_binding)),
        );
    }
}

#[test]
fn synthetic_valid_native_observation_exposes_identity_only_through_binding() {
    let name = b"body-00000000000000000000000000";
    let manifest = br#"{"body_source_schema":"solstone.body.bundle.v1","body_bundle_ref":"body-bundle.json","body_bundle_sha256":"sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","import_id":"body-00000000000000000000000000","source_type":"apple_health","source_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","entry_count":0,"days_affected":[],"raw_retention":"discard"}"#;
    let authority = authorize_native_bundle(DirectoryObservation {
        name,
        envelope_present: true,
        ledger_present: true,
        manifest: Some(manifest),
    })
    .expect("synthetic observation should authorize");
    let binding = authority.binding();
    assert_eq!(authority.id(), binding.import_id());
    assert_eq!(binding.body_source_schema(), "solstone.body.bundle.v1");
    assert_eq!(binding.body_bundle_ref(), "body-bundle.json");
    assert_eq!(
        binding.body_bundle_sha256().as_str(),
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
    );
    assert_eq!(binding.source_type().as_str(), "apple_health");
    assert_eq!(
        binding.source_hash().as_str(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(binding.entry_count(), 0);
    assert!(binding.days_affected().is_empty());
    assert_eq!(binding.raw_retention().as_str(), "discard");
}
