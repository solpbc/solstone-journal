// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{
    BodyDay, BodyEnvelope, decode_body_envelope, encode_body_envelope,
};

use crate::support;

use support::{envelope_multimonth_fixture, native_bundle_fixture};

#[test]
fn decodes_and_round_trips_every_native_fixture_envelope() {
    for case in native_bundle_fixture()["cases"].as_array().unwrap() {
        let input = case["expected_envelope_jsonl"].as_str().unwrap().as_bytes();
        let envelope = decode_body_envelope(input).expect("fixture envelope decodes");
        let expected: Value =
            serde_json::from_str(case["expected_envelope_jsonl"].as_str().unwrap()).unwrap();
        assert_envelope_matches(&envelope, &expected);
        assert_eq!(encode_body_envelope(&envelope).unwrap(), input);
    }
}

#[test]
fn decodes_and_round_trips_the_multimonth_fixture_envelope() {
    let case = &envelope_multimonth_fixture()["cases"][0];
    let input = case["expected_envelope_jsonl"].as_str().unwrap().as_bytes();
    let envelope = decode_body_envelope(input).expect("multimonth envelope decodes");
    let expected: Value =
        serde_json::from_str(case["expected_envelope_jsonl"].as_str().unwrap()).unwrap();
    assert_envelope_matches(&envelope, &expected);
    assert_eq!(encode_body_envelope(&envelope).unwrap(), input);
}

fn assert_envelope_matches(envelope: &BodyEnvelope, expected: &Value) {
    assert_eq!(envelope.schema(), expected["schema"].as_str().unwrap());
    assert_eq!(
        envelope.bundle_id().as_str(),
        expected["bundle_id"].as_str().unwrap()
    );
    assert_eq!(
        envelope.source_family().as_str(),
        expected["source_family"].as_str().unwrap()
    );
    assert_eq!(
        envelope.source_hash().as_str(),
        expected["source_hash"].as_str().unwrap()
    );
    assert_eq!(
        envelope.raw_retention().as_str(),
        expected["raw_retention"].as_str().unwrap()
    );
    assert_eq!(
        envelope.row_count(),
        expected["row_count"].as_u64().unwrap()
    );
    assert_eq!(
        envelope
            .days()
            .iter()
            .map(BodyDay::as_str)
            .collect::<Vec<_>>(),
        expected["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(|day| day.as_str().unwrap())
            .collect::<Vec<_>>(),
    );
    let shards = expected["shards"].as_array().unwrap();
    assert_eq!(envelope.shards().len(), shards.len());
    for (shard, expected) in envelope.shards().iter().zip(shards) {
        assert_eq!(shard.path(), expected["path"].as_str().unwrap());
        assert_eq!(shard.bytes(), expected["bytes"].as_u64().unwrap());
        assert_eq!(shard.rows(), expected["rows"].as_u64().unwrap());
        assert_eq!(
            shard.sha256().as_str(),
            expected["sha256"].as_str().unwrap()
        );
    }
    assert_eq!(
        envelope.ledger().path(),
        expected["ledger"]["path"].as_str().unwrap()
    );
    assert_eq!(
        envelope.ledger().bytes(),
        expected["ledger"]["bytes"].as_u64().unwrap()
    );
    assert_eq!(
        envelope.ledger().events(),
        expected["ledger"]["events"].as_u64().unwrap()
    );
    assert_eq!(
        envelope.ledger().sha256().as_str(),
        expected["ledger"]["sha256"].as_str().unwrap()
    );
    match (envelope.summary_plan(), &expected["summary_plan"]) {
        (Some(plan), expected) => {
            assert_eq!(plan.schema(), expected["schema"].as_str().unwrap());
            assert_eq!(
                plan.days().iter().map(BodyDay::as_str).collect::<Vec<_>>(),
                expected["days"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|day| day.as_str().unwrap())
                    .collect::<Vec<_>>(),
            );
        }
        (None, expected) => assert!(expected.is_null()),
    }
}
