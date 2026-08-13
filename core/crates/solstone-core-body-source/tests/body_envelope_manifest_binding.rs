// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyEnvelope, BodyManifestBinding, BodyRawRetention, BodySourceFamily,
    BodySourceHash, BundleId, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField, canonicalize,
    decode_body_envelope, decode_body_envelope_with_manifest, encode_body_envelope, parse,
};

use crate::support;

use support::{
    NativeBundleManifestBindingCase, envelope_multimonth_fixture,
    envelope_multimonth_manifest_binding, native_bundle_fixture,
    native_bundle_manifest_binding_cases,
};

const OTHER_DIGEST: &str =
    "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

fn binding_for_case(case: &NativeBundleManifestBindingCase) -> BodyManifestBinding {
    BodyManifestBinding::new(
        case.body_bundle_sha256.clone(),
        case.import_id.clone(),
        case.source_type,
        case.source_hash.clone(),
        case.entry_count,
        case.days_affected.clone(),
        case.raw_retention,
    )
    .expect("fixture binding is valid")
}

fn binding(
    digest: BodyDigest,
    import_id: BundleId,
    source_type: BodySourceFamily,
    source_hash: BodySourceHash,
    entry_count: u64,
    days_affected: Vec<BodyDay>,
    raw_retention: BodyRawRetention,
) -> BodyManifestBinding {
    BodyManifestBinding::new(
        digest,
        import_id,
        source_type,
        source_hash,
        entry_count,
        days_affected,
        raw_retention,
    )
    .expect("test binding is valid")
}

fn input_for_case(fixture: &Value, name: &str) -> Vec<u8> {
    fixture["cases"]
        .as_array()
        .expect("fixture cases")
        .iter()
        .find(|case| case["name"] == name)
        .expect("fixture case")["expected_envelope_jsonl"]
        .as_str()
        .expect("fixture envelope JSONL")
        .as_bytes()
        .to_vec()
}

fn sha256(bytes: &[u8]) -> BodyDigest {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let spelling = format!("sha256:{:x}", digest.finalize());
    BodyDigest::from_bytes(spelling.as_bytes()).expect("SHA-256 output is a valid digest")
}

fn assert_fixture_digest(case: &Value, input: &[u8]) {
    let digest = sha256(input);
    assert_eq!(digest.as_str(), case["expected_envelope_sha256"]);
    assert_eq!(
        digest.as_str(),
        case["expected_manifest_binding"]["body_bundle_sha256"]
    );
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

fn assert_manifest_mismatch(error: EnvelopeError, bundle: &BundleId) {
    assert_eq!(error.code(), EnvelopeErrorCode::ManifestMismatch);
    assert_eq!(error.field(), EnvelopeErrorField::ManifestBinding);
    assert_eq!(error.index(), None);
    assert_eq!(error.bundle(), Some(bundle));
    let expected = format!(
        "body-envelope[{}]/body-bundle.json manifest_mismatch: manifest_binding",
        bundle.as_str()
    );
    assert_eq!(error.to_string(), expected);
    assert_eq!(format!("{error:?}"), expected);
    assert!(expected.len() <= 256);
    assert!(!expected.contains("sha256:"));
    assert!(!expected.contains("source_hash"));
    assert!(Error::source(&error).is_none());
}

fn canonical(value: &Value) -> Vec<u8> {
    let parsed =
        parse(&serde_json::to_vec(value).expect("value serializes")).expect("value parses");
    format!("{}\n", canonicalize(&parsed).expect("value canonicalizes")).into_bytes()
}

#[test]
fn fixture_envelopes_bind_and_round_trip() {
    let fixture = native_bundle_fixture();
    let cases = native_bundle_manifest_binding_cases();
    assert_eq!(cases.len(), 4);
    for (fixture_case, binding_case) in fixture["cases"]
        .as_array()
        .expect("fixture cases")
        .iter()
        .zip(cases)
    {
        assert_eq!(fixture_case["name"].as_str().unwrap(), binding_case.name);
        let input = fixture_case["expected_envelope_jsonl"]
            .as_str()
            .expect("fixture envelope JSONL")
            .as_bytes();
        assert_fixture_digest(fixture_case, input);
        let envelope = decode_body_envelope_with_manifest(input, &binding_for_case(&binding_case))
            .expect("fixture envelope binds");
        let expected: Value = serde_json::from_str(
            fixture_case["expected_envelope_jsonl"]
                .as_str()
                .expect("fixture envelope JSONL"),
        )
        .expect("fixture envelope parses");
        assert_envelope_matches(&envelope, &expected);
        assert_eq!(encode_body_envelope(&envelope).unwrap(), input);
    }

    let fixture = envelope_multimonth_fixture();
    let case = &fixture["cases"][0];
    let input = case["expected_envelope_jsonl"]
        .as_str()
        .expect("fixture envelope JSONL")
        .as_bytes();
    assert_fixture_digest(case, input);
    let envelope =
        decode_body_envelope_with_manifest(input, &envelope_multimonth_manifest_binding())
            .expect("multimonth envelope binds");
    let expected: Value = serde_json::from_str(case["expected_envelope_jsonl"].as_str().unwrap())
        .expect("fixture envelope parses");
    assert_envelope_matches(&envelope, &expected);
    assert_eq!(encode_body_envelope(&envelope).unwrap(), input);
}

#[test]
fn valid_binding_mismatches_collapse_to_manifest_binding_error() {
    let fixture = native_bundle_fixture();
    let cases = native_bundle_manifest_binding_cases();
    let apple = cases
        .iter()
        .find(|case| case.name == "apple_retain_complete_one_row")
        .expect("Apple fixture case");
    let oura = cases
        .iter()
        .find(|case| case.name == "oura_retain_parsed_one_row")
        .expect("Oura fixture case");
    let apple_input = input_for_case(&fixture, &apple.name);
    let oura_input = input_for_case(&fixture, &oura.name);

    let other_bundle =
        BundleId::from_bytes(b"body-01J9ZK2F5M7Q8R3S4T6V0W1X31").expect("other bundle ID is valid");
    let apple_family_hash = BodySourceHash::from_bytes_for_family(
        oura.source_hash.as_str().as_bytes(),
        &BodySourceFamily::AppleHealth,
    )
    .expect("plain hash is valid for Apple");
    let other_hash = BodySourceHash::from_bytes_for_family(
        b"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        &BodySourceFamily::AppleHealth,
    )
    .expect("alternate hash is valid");
    let changed_days = vec![BodyDay::from_bytes(b"20260103").expect("day is valid")];

    let cases = vec![
        (
            "bundle",
            &apple_input,
            binding(
                sha256(&apple_input),
                other_bundle,
                apple.source_type,
                apple.source_hash.clone(),
                apple.entry_count,
                apple.days_affected.clone(),
                apple.raw_retention,
            ),
            &apple.import_id,
        ),
        (
            "family",
            &oura_input,
            binding(
                sha256(&oura_input),
                oura.import_id.clone(),
                BodySourceFamily::AppleHealth,
                apple_family_hash,
                oura.entry_count,
                oura.days_affected.clone(),
                BodyRawRetention::RetainParsed,
            ),
            &oura.import_id,
        ),
        (
            "source hash",
            &apple_input,
            binding(
                sha256(&apple_input),
                apple.import_id.clone(),
                apple.source_type,
                other_hash,
                apple.entry_count,
                apple.days_affected.clone(),
                apple.raw_retention,
            ),
            &apple.import_id,
        ),
        (
            "raw retention",
            &apple_input,
            binding(
                sha256(&apple_input),
                apple.import_id.clone(),
                apple.source_type,
                apple.source_hash.clone(),
                apple.entry_count,
                apple.days_affected.clone(),
                BodyRawRetention::RetainParsed,
            ),
            &apple.import_id,
        ),
        (
            "entry count",
            &apple_input,
            binding(
                sha256(&apple_input),
                apple.import_id.clone(),
                apple.source_type,
                apple.source_hash.clone(),
                apple.entry_count + 1,
                apple.days_affected.clone(),
                apple.raw_retention,
            ),
            &apple.import_id,
        ),
        (
            "days",
            &oura_input,
            binding(
                sha256(&oura_input),
                oura.import_id.clone(),
                oura.source_type,
                oura.source_hash.clone(),
                oura.entry_count,
                changed_days,
                oura.raw_retention,
            ),
            &oura.import_id,
        ),
    ];

    for (name, input, binding, expected_bundle) in cases {
        let Err(error) = decode_body_envelope_with_manifest(input, &binding) else {
            panic!("{name} unexpectedly bound");
        };
        assert_manifest_mismatch(error, expected_bundle);
    }
}

#[test]
fn digest_and_retention_claims_bind_exact_input_bytes() {
    let fixture = native_bundle_fixture();
    let cases = native_bundle_manifest_binding_cases();
    let apple = cases
        .iter()
        .find(|case| case.name == "apple_retain_complete_one_row")
        .expect("Apple fixture case");
    let input = input_for_case(&fixture, &apple.name);
    let matching_digest = sha256(&input);

    let different_digest =
        BodyDigest::from_bytes(OTHER_DIGEST.as_bytes()).expect("digest is valid");
    let mismatch = binding(
        different_digest,
        apple.import_id.clone(),
        apple.source_type,
        apple.source_hash.clone(),
        apple.entry_count,
        apple.days_affected.clone(),
        apple.raw_retention,
    );
    let error = decode_body_envelope_with_manifest(&input, &mismatch).unwrap_err();
    assert_manifest_mismatch(error, &apple.import_id);

    let exact = binding(
        matching_digest.clone(),
        apple.import_id.clone(),
        apple.source_type,
        apple.source_hash.clone(),
        apple.entry_count,
        apple.days_affected.clone(),
        apple.raw_retention,
    );
    assert!(decode_body_envelope_with_manifest(&input, &exact).is_ok());

    let without_lf = input.strip_suffix(b"\n").expect("fixture input ends in LF");
    let no_lf_digest = sha256(without_lf);
    assert_ne!(no_lf_digest, matching_digest);
    let no_lf = binding(
        no_lf_digest,
        apple.import_id.clone(),
        apple.source_type,
        apple.source_hash.clone(),
        apple.entry_count,
        apple.days_affected.clone(),
        apple.raw_retention,
    );
    let error = decode_body_envelope_with_manifest(&input, &no_lf).unwrap_err();
    assert_manifest_mismatch(error, &apple.import_id);

    let retention = binding(
        matching_digest,
        apple.import_id.clone(),
        apple.source_type,
        apple.source_hash.clone(),
        apple.entry_count,
        apple.days_affected.clone(),
        BodyRawRetention::RetainParsed,
    );
    let error = decode_body_envelope_with_manifest(&input, &retention).unwrap_err();
    assert_manifest_mismatch(error, &apple.import_id);

    let mut mutated: Value = serde_json::from_slice(&input).expect("fixture envelope parses");
    mutated["raw_retention"] = Value::from("retain_parsed");
    let mutated_input = canonical(&mutated);
    let stale_semantics = binding(
        sha256(&mutated_input),
        apple.import_id.clone(),
        apple.source_type,
        apple.source_hash.clone(),
        apple.entry_count,
        apple.days_affected.clone(),
        apple.raw_retention,
    );
    let error = decode_body_envelope_with_manifest(&mutated_input, &stale_semantics).unwrap_err();
    assert_manifest_mismatch(error, &apple.import_id);
}

#[test]
fn decoder_failures_pass_through_and_prefixes_are_safe() {
    let fixture = native_bundle_fixture();
    let cases = native_bundle_manifest_binding_cases();
    let apple = cases
        .iter()
        .find(|case| case.name == "apple_retain_complete_one_row")
        .expect("Apple fixture case");
    let binding = binding_for_case(apple);
    let valid = input_for_case(&fixture, &apple.name);
    let mut aggregate: Value = serde_json::from_slice(&valid).expect("fixture envelope parses");
    aggregate["row_count"] = Value::from(0);
    let unknown = format!(
        r#"{{"a_unknown":null,{}"#,
        std::str::from_utf8(&valid[1..]).unwrap()
    );
    let failures = vec![
        vec![b' '; 1_048_577],
        b"{\n".to_vec(),
        valid[..valid.len() - 1].to_vec(),
        b"null\n".to_vec(),
        unknown.into_bytes(),
        canonical(&aggregate),
    ];
    for input in failures {
        let direct = decode_body_envelope(&input).expect_err("input must fail decoding");
        let bound = decode_body_envelope_with_manifest(&input, &binding)
            .expect_err("input must fail through bound decoder");
        assert_eq!(bound, direct);
        assert_ne!(bound.code(), EnvelopeErrorCode::ManifestMismatch);
    }

    for length in 1..valid.len() {
        let result = catch_unwind(AssertUnwindSafe(|| {
            decode_body_envelope_with_manifest(&valid[..length], &binding)
        }))
        .unwrap_or_else(|_| panic!("bound decoder panicked for prefix length {length}"));
        let error = result.expect_err("proper prefix must not bind");
        assert_ne!(error.code(), EnvelopeErrorCode::ManifestMismatch);
        let display = error.to_string();
        assert!(display.len() <= 256);
        assert_eq!(display, format!("{error:?}"));
        assert!(!display.contains(['{', '}', '"']));
        assert!(Error::source(&error).is_none());
    }
}
