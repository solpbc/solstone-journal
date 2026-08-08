// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{BodyDigest, BundleId, EnvelopeLedger};

mod support;

use support::{envelope_multimonth_fixture, native_bundle_fixture};

fn bundle_from_case(case: &Value) -> BundleId {
    BundleId::from_bytes(
        case["directory"]
            .as_str()
            .expect("case directory")
            .as_bytes(),
    )
    .expect("fixture directory is a valid bundle ID")
}

fn assert_ledger_matches_fixture(ledger: &EnvelopeLedger, expected: &Value) {
    assert_eq!(
        ledger.path(),
        expected["path"].as_str().expect("ledger path")
    );
    assert_eq!(
        ledger.bytes(),
        expected["bytes"].as_u64().expect("ledger bytes")
    );
    assert_eq!(
        ledger.events(),
        expected["events"].as_u64().expect("ledger events")
    );
    assert_eq!(
        ledger.sha256().as_str(),
        expected["sha256"].as_str().expect("ledger digest")
    );
}

#[test]
fn envelope_ledger_fixture_matches_native_bundle_descriptors() {
    let fixture = native_bundle_fixture();
    let mut descriptors = 0;

    for case in fixture["cases"].as_array().expect("fixture cases") {
        let name = case["name"].as_str().expect("case name");
        let envelope: Value = serde_json::from_str(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("expected envelope JSONL"),
        )
        .expect("expected envelope JSONL parses");
        let expected = &envelope["ledger"];
        let ledger = EnvelopeLedger::new(
            &bundle_from_case(case),
            expected["bytes"].as_u64().expect("ledger bytes"),
            expected["events"].as_u64().expect("ledger events"),
            BodyDigest::from_bytes(
                expected["sha256"]
                    .as_str()
                    .expect("ledger digest")
                    .as_bytes(),
            )
            .expect("fixture ledger digest is valid"),
        )
        .unwrap_or_else(|error| panic!("{name} ledger should bind: {error}"));
        assert_ledger_matches_fixture(&ledger, expected);
        descriptors += 1;
    }

    assert_eq!(descriptors, 4);
}

#[test]
fn envelope_ledger_fixture_matches_multimonth_descriptor() {
    let fixture = envelope_multimonth_fixture();
    let case = &fixture["cases"][0];
    let expected = &case["expected_envelope"]["ledger"];
    let ledger = EnvelopeLedger::new(
        &bundle_from_case(case),
        expected["bytes"].as_u64().expect("ledger bytes"),
        expected["events"].as_u64().expect("ledger events"),
        BodyDigest::from_bytes(
            expected["sha256"]
                .as_str()
                .expect("ledger digest")
                .as_bytes(),
        )
        .expect("fixture ledger digest is valid"),
    )
    .expect("multimonth ledger should bind");

    assert_eq!(ledger.bytes(), 75);
    assert_eq!(ledger.events(), 3);
    assert_ledger_matches_fixture(&ledger, expected);
}
