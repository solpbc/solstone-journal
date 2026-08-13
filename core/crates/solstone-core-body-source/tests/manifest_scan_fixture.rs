// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyString, BodyValue, ManifestKnownKey, parse, scan_body_manifest,
};

use crate::support;

use support::{assert_body_value_bitwise_eq, native_bundle_fixture};

#[test]
fn native_bundle_manifests_scan_to_their_parsed_objects() {
    let fixture = native_bundle_fixture();
    for case in fixture["cases"].as_array().expect("fixture cases") {
        let manifest = serde_json::to_string(&case["manifest"]).expect("manifest serializes");
        let scanned = scan_body_manifest(manifest.as_bytes()).expect("manifest scans");
        let expected = parse(manifest.as_bytes()).expect("manifest parses");
        let BodyValue::Object(expected) = expected else {
            panic!("fixture manifest must be an object");
        };

        assert_body_value_bitwise_eq(
            &BodyValue::Object(scanned.object().clone()),
            &BodyValue::Object(expected),
        );
        assert!(scanned.has_body_prefixed_key());
        assert!(!scanned.has_unknown_body_prefixed_key());
        assert!(scanned.duplicated_known_keys().is_empty());
    }
}

#[test]
fn known_duplicate_facts_are_decoded_and_canonically_ordered() {
    for (input, expected_value) in [
        (
            br#"{"import_id":"first","import_id":"second"}"#.as_slice(),
            "second",
        ),
        (
            br#"{"import_id":"first","import_id":"second","import_id":"third"}"#,
            "third",
        ),
        (
            br#"{"\u0069mport_id":"first","\u0069mport_id":"second"}"#,
            "second",
        ),
        (
            br#"{"\u0069mport_id":"first","\u0069mport_id":"second","\u0069mport_id":"third"}"#,
            "third",
        ),
        (
            br#"{"import_id":"first","\u0069mport_id":"second"}"#,
            "second",
        ),
        (
            br#"{"import_id":"first","\u0069mport_id":"second","import_id":"third"}"#,
            "third",
        ),
    ] {
        let scanned = scan_body_manifest(input).expect("duplicate object scans");
        assert_eq!(
            scanned.duplicated_known_keys(),
            &[ManifestKnownKey::ImportId]
        );
        assert_eq!(
            scanned.object().get(&body_string("import_id")),
            Some(&BodyValue::String(body_string(expected_value)))
        );
    }

    let unrelated = scan_body_manifest(br#"{"other":1,"other":2}"#).expect("object scans");
    assert!(unrelated.duplicated_known_keys().is_empty());

    let ordered =
        scan_body_manifest(br#"{"raw_retention":1,"raw_retention":2,"import_id":1,"import_id":2}"#)
            .expect("object scans");
    assert_eq!(
        ordered.duplicated_known_keys(),
        &[ManifestKnownKey::ImportId, ManifestKnownKey::RawRetention]
    );
}

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect()).expect("ASCII string")
}
