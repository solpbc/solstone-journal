// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{
    AppleSummaryPlan, BodyDay, BodyDigest, BodyEnvelope, BodyMonth, BodyRawRetention,
    BodySourceFamily, BodySourceHash, BundleId, EnvelopeLedger, EnvelopeShard,
};

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

fn days_from_fixture(days: &Value) -> Vec<BodyDay> {
    days.as_array()
        .expect("envelope days")
        .iter()
        .map(|day| {
            BodyDay::from_bytes(day.as_str().expect("envelope day").as_bytes())
                .expect("fixture day is valid")
        })
        .collect()
}

fn month_from_fixture_path(path: &str) -> BodyMonth {
    let month = path
        .strip_prefix("normalized/")
        .and_then(|value| value.strip_suffix(".jsonl"))
        .expect("fixture shard path has normalized month form");
    BodyMonth::from_bytes(month.as_bytes()).expect("fixture shard month is valid")
}

fn envelope_from_fixture(case: &Value, expected: &Value) -> BodyEnvelope {
    let bundle = bundle_from_case(case);
    let family = BodySourceFamily::from_bytes(
        expected["source_family"]
            .as_str()
            .expect("source family")
            .as_bytes(),
    )
    .expect("fixture source family is valid");
    let days = days_from_fixture(&expected["days"]);
    let shards = expected["shards"]
        .as_array()
        .expect("envelope shards")
        .iter()
        .enumerate()
        .map(|(index, shard)| {
            EnvelopeShard::new(
                &bundle,
                index as u64,
                month_from_fixture_path(shard["path"].as_str().expect("shard path")),
                shard["bytes"].as_u64().expect("shard bytes"),
                shard["rows"].as_u64().expect("shard rows"),
                BodyDigest::from_bytes(shard["sha256"].as_str().expect("shard digest").as_bytes())
                    .expect("fixture shard digest is valid"),
            )
            .expect("fixture shard should bind")
        })
        .collect();
    let ledger = EnvelopeLedger::new(
        &bundle,
        expected["ledger"]["bytes"].as_u64().expect("ledger bytes"),
        expected["ledger"]["events"]
            .as_u64()
            .expect("ledger events"),
        BodyDigest::from_bytes(
            expected["ledger"]["sha256"]
                .as_str()
                .expect("ledger digest")
                .as_bytes(),
        )
        .expect("fixture ledger digest is valid"),
    )
    .expect("fixture ledger should bind");
    let summary_plan = (!expected["summary_plan"].is_null()).then(|| {
        AppleSummaryPlan::new(
            &bundle,
            days_from_fixture(&expected["summary_plan"]["days"]),
        )
        .expect("fixture summary plan should bind")
    });

    BodyEnvelope::new(
        bundle,
        family,
        BodySourceHash::from_bytes_for_family(
            expected["source_hash"]
                .as_str()
                .expect("source hash")
                .as_bytes(),
            &family,
        )
        .expect("fixture source hash is valid"),
        BodyRawRetention::from_bytes(
            expected["raw_retention"]
                .as_str()
                .expect("raw retention")
                .as_bytes(),
        )
        .expect("fixture raw retention is valid"),
        expected["row_count"].as_u64().expect("row count"),
        days,
        shards,
        ledger,
        summary_plan,
    )
    .expect("fixture envelope should bind")
}

fn assert_envelope_matches_fixture(envelope: &BodyEnvelope, expected: &Value) {
    assert_eq!(
        envelope.schema(),
        expected["schema"].as_str().expect("schema")
    );
    assert_eq!(
        envelope.bundle_id().as_str(),
        expected["bundle_id"].as_str().expect("bundle ID")
    );
    assert_eq!(
        envelope.source_family().as_str(),
        expected["source_family"].as_str().expect("source family")
    );
    assert_eq!(
        envelope.source_hash().as_str(),
        expected["source_hash"].as_str().expect("source hash")
    );
    assert_eq!(
        envelope.raw_retention().as_str(),
        expected["raw_retention"].as_str().expect("raw retention")
    );
    assert_eq!(
        envelope.row_count(),
        expected["row_count"].as_u64().expect("row count")
    );
    assert_eq!(
        envelope
            .days()
            .iter()
            .map(BodyDay::as_str)
            .collect::<Vec<_>>(),
        expected["days"]
            .as_array()
            .expect("days")
            .iter()
            .map(|day| day.as_str().expect("day"))
            .collect::<Vec<_>>()
    );

    let expected_shards = expected["shards"].as_array().expect("shards");
    assert_eq!(envelope.shards().len(), expected_shards.len());
    for (shard, expected_shard) in envelope.shards().iter().zip(expected_shards) {
        assert_eq!(shard.path(), expected_shard["path"].as_str().expect("path"));
        assert_eq!(
            shard.bytes(),
            expected_shard["bytes"].as_u64().expect("bytes")
        );
        assert_eq!(shard.rows(), expected_shard["rows"].as_u64().expect("rows"));
        assert_eq!(
            shard.sha256().as_str(),
            expected_shard["sha256"].as_str().expect("digest")
        );
    }

    let expected_ledger = &expected["ledger"];
    assert_eq!(
        envelope.ledger().path(),
        expected_ledger["path"].as_str().expect("ledger path")
    );
    assert_eq!(
        envelope.ledger().bytes(),
        expected_ledger["bytes"].as_u64().expect("ledger bytes")
    );
    assert_eq!(
        envelope.ledger().events(),
        expected_ledger["events"].as_u64().expect("ledger events")
    );
    assert_eq!(
        envelope.ledger().sha256().as_str(),
        expected_ledger["sha256"].as_str().expect("ledger digest")
    );

    match (&expected["summary_plan"], envelope.summary_plan()) {
        (expected, Some(plan)) => {
            assert_eq!(
                plan.schema(),
                expected["schema"].as_str().expect("plan schema")
            );
            assert_eq!(
                plan.days().iter().map(BodyDay::as_str).collect::<Vec<_>>(),
                expected["days"]
                    .as_array()
                    .expect("plan days")
                    .iter()
                    .map(|day| day.as_str().expect("plan day"))
                    .collect::<Vec<_>>()
            );
        }
        (expected, None) => assert!(expected.is_null(), "only null plans are absent"),
    }
}

#[test]
fn body_envelope_fixture_matches_native_bundle_cases() {
    let fixture = native_bundle_fixture();
    let mut envelopes = 0;

    for case in fixture["cases"].as_array().expect("fixture cases") {
        let expected: Value = serde_json::from_str(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("expected envelope JSONL"),
        )
        .expect("expected envelope JSONL parses");
        let envelope = envelope_from_fixture(case, &expected);
        assert_envelope_matches_fixture(&envelope, &expected);

        match expected["source_family"].as_str().expect("source family") {
            "apple_health" => assert!(envelope.summary_plan().is_some()),
            "oura_api" => assert!(envelope.summary_plan().is_none()),
            family => panic!("unexpected source family {family}"),
        }
        envelopes += 1;
    }

    assert_eq!(envelopes, 4);
}

#[test]
fn body_envelope_fixture_matches_multimonth_case() {
    let fixture = envelope_multimonth_fixture();
    let case = &fixture["cases"][0];
    let expected = &case["expected_envelope"];
    let envelope = envelope_from_fixture(case, expected);

    assert_envelope_matches_fixture(&envelope, expected);
    assert_eq!(envelope.shards().len(), 2);
    assert_eq!(
        envelope.summary_plan().expect("Apple plan").days(),
        envelope.days()
    );
}
