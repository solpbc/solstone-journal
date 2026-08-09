// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    AppleSummaryPlan, BodyDay, BodyDigest, BodyEnvelope, BodyRawRetention, BodySourceFamily,
    BodySourceHash, BundleId, EnvelopeLedger, EnvelopeShard, encode_body_envelope,
};

mod support;

use support::{envelope_multimonth_fixture, native_bundle_fixture};

#[test]
fn native_fixture_envelopes_recompute_artifacts_and_encode_exact_jsonl() {
    for case in native_bundle_fixture()["cases"]
        .as_array()
        .expect("fixture cases")
    {
        let envelope = native_envelope(case);
        let encoded = encode_body_envelope(&envelope).expect("fixture envelope encodes");
        assert_eq!(
            encoded,
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("expected JSONL")
                .as_bytes()
        );
        assert_eq!(sha256(&encoded), case["expected_envelope_sha256"]);
        assert_fixture_shape(case["expected_envelope_jsonl"].as_str().unwrap(), &envelope);
    }
}

#[test]
fn multimonth_fixture_envelope_recomputes_artifacts_and_encodes_exact_jsonl() {
    let fixture = envelope_multimonth_fixture();
    let case = &fixture["cases"][0];
    let envelope = multimonth_envelope(case);
    let encoded = encode_body_envelope(&envelope).expect("fixture envelope encodes");
    assert_eq!(
        encoded,
        case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()
    );
    assert_eq!(sha256(&encoded), case["expected_envelope_sha256"]);
    assert_fixture_shape(case["expected_envelope_jsonl"].as_str().unwrap(), &envelope);
}

fn native_envelope(case: &Value) -> BodyEnvelope {
    let manifest = &case["manifest"];
    let bundle = bundle(case);
    let family = family(manifest);
    let days = days(&manifest["days_affected"]);
    let rows = manifest["entry_count"].as_u64().unwrap();
    let normalized = case["expected_normalized_jsonl"].as_str().unwrap();
    let shards = if rows == 0 {
        vec![]
    } else {
        let digest = checked_digest(normalized, &case["expected_normalized_sha256"]);
        vec![
            EnvelopeShard::new(
                &bundle,
                0,
                days[0].month(),
                normalized.len() as u64,
                normalized.lines().count() as u64,
                digest,
            )
            .unwrap(),
        ]
    };
    let ledger_text = case["expected_ledger_jsonl"].as_str().unwrap();
    let ledger = EnvelopeLedger::new(
        &bundle,
        ledger_text.len() as u64,
        ledger_text.lines().count() as u64,
        checked_digest(ledger_text, &case["expected_ledger_sha256"]),
    )
    .unwrap();
    build_envelope(bundle, family, manifest, days, shards, ledger)
}

fn multimonth_envelope(case: &Value) -> BodyEnvelope {
    let binding = &case["expected_manifest_binding"];
    let bundle = bundle(case);
    let family = family(binding);
    let days = days(&binding["days_affected"]);
    let expected = &case["expected_envelope"];
    let shards = case["digest_basis"]["shards"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, basis)| {
            let text = basis["exact_bytes"].as_str().unwrap();
            let expected_shard = &expected["shards"][index];
            let digest = checked_digest(text, &expected_shard["sha256"]);
            let month = basis["path"]
                .as_str()
                .unwrap()
                .strip_prefix("normalized/")
                .and_then(|path| path.strip_suffix(".jsonl"))
                .unwrap();
            EnvelopeShard::new(
                &bundle,
                index as u64,
                solstone_core_body_source::BodyMonth::from_bytes(month.as_bytes()).unwrap(),
                text.len() as u64,
                text.lines().count() as u64,
                digest,
            )
            .unwrap()
        })
        .collect();
    let ledger_text = case["digest_basis"]["ledger"]["exact_bytes"]
        .as_str()
        .unwrap();
    let ledger = EnvelopeLedger::new(
        &bundle,
        ledger_text.len() as u64,
        ledger_text.lines().count() as u64,
        checked_digest(ledger_text, &expected["ledger"]["sha256"]),
    )
    .unwrap();
    build_envelope(bundle, family, binding, days, shards, ledger)
}

fn build_envelope(
    bundle: BundleId,
    family: BodySourceFamily,
    manifest: &Value,
    days: Vec<BodyDay>,
    shards: Vec<EnvelopeShard>,
    ledger: EnvelopeLedger,
) -> BodyEnvelope {
    let plan = (family == BodySourceFamily::AppleHealth)
        .then(|| AppleSummaryPlan::new(&bundle, days.clone()).unwrap());
    BodyEnvelope::new(
        bundle,
        family,
        BodySourceHash::from_bytes_for_family(
            manifest["source_hash"].as_str().unwrap().as_bytes(),
            &family,
        )
        .unwrap(),
        BodyRawRetention::from_bytes(manifest["raw_retention"].as_str().unwrap().as_bytes())
            .unwrap(),
        manifest["entry_count"].as_u64().unwrap(),
        days,
        shards,
        ledger,
        plan,
    )
    .unwrap()
}

fn assert_fixture_shape(jsonl: &str, envelope: &BodyEnvelope) {
    let encoded: Value = serde_json::from_str(jsonl.trim_end_matches('\n')).unwrap();
    assert_eq!(
        keys(&encoded),
        set([
            "bundle_id",
            "days",
            "ledger",
            "raw_retention",
            "row_count",
            "schema",
            "shards",
            "source_family",
            "source_hash",
            "summary_plan"
        ])
    );
    assert_eq!(
        keys(&encoded["ledger"]),
        set(["bytes", "events", "path", "sha256"])
    );
    let encoded_shards = encoded["shards"].as_array().unwrap();
    assert_eq!(encoded_shards.len(), envelope.shards().len());
    for (shard, envelope_shard) in encoded_shards.iter().zip(envelope.shards()) {
        assert_eq!(keys(shard), set(["bytes", "path", "rows", "sha256"]));
        assert_eq!(shard["bytes"], envelope_shard.bytes());
        assert_eq!(shard["path"], envelope_shard.path());
        assert_eq!(shard["rows"], envelope_shard.rows());
        assert_eq!(shard["sha256"], envelope_shard.sha256().as_str());
    }
    assert_eq!(encoded["bundle_id"], envelope.bundle_id().as_str());
    assert_eq!(encoded["schema"], envelope.schema());
    assert_eq!(encoded["row_count"], envelope.row_count());
    assert_eq!(
        encoded["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect::<Vec<_>>(),
        envelope
            .days()
            .iter()
            .map(BodyDay::as_str)
            .map(Some)
            .collect::<Vec<_>>()
    );
    assert_eq!(encoded["source_family"], envelope.source_family().as_str());
    assert_eq!(encoded["source_hash"], envelope.source_hash().as_str());
    assert_eq!(encoded["raw_retention"], envelope.raw_retention().as_str());
    assert_eq!(encoded["ledger"]["bytes"], envelope.ledger().bytes());
    assert_eq!(encoded["ledger"]["events"], envelope.ledger().events());
    assert_eq!(encoded["ledger"]["path"], envelope.ledger().path());
    assert_eq!(
        encoded["ledger"]["sha256"],
        envelope.ledger().sha256().as_str()
    );
    match envelope.summary_plan() {
        Some(plan) => {
            assert_eq!(keys(&encoded["summary_plan"]), set(["days", "schema"]));
            assert_eq!(encoded["summary_plan"]["schema"], plan.schema());
            assert_eq!(
                encoded["summary_plan"]["days"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(Value::as_str)
                    .collect::<Vec<_>>(),
                plan.days()
                    .iter()
                    .map(BodyDay::as_str)
                    .map(Some)
                    .collect::<Vec<_>>()
            );
        }
        None => assert!(encoded["summary_plan"].is_null()),
    }
}

fn bundle(case: &Value) -> BundleId {
    BundleId::from_bytes(case["directory"].as_str().unwrap().as_bytes()).unwrap()
}

fn family(manifest: &Value) -> BodySourceFamily {
    BodySourceFamily::from_bytes(manifest["source_type"].as_str().unwrap().as_bytes()).unwrap()
}

fn days(value: &Value) -> Vec<BodyDay> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|day| BodyDay::from_bytes(day.as_str().unwrap().as_bytes()).unwrap())
        .collect()
}

fn checked_digest(text: &str, expected: &Value) -> BodyDigest {
    let actual = sha256(text.as_bytes());
    assert_eq!(actual, expected.as_str().unwrap());
    BodyDigest::from_bytes(actual.as_bytes()).unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn set<const N: usize>(values: [&'static str; N]) -> BTreeSet<&'static str> {
    values.into_iter().collect()
}
