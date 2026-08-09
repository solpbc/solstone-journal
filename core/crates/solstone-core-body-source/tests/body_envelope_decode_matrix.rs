// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_body_source::{
    EnvelopeErrorCode, EnvelopeErrorField, canonicalize, decode_body_envelope, parse,
};

mod support;

use support::native_bundle_fixture;

fn valid() -> Value {
    serde_json::from_str(
        native_bundle_fixture()["cases"][0]["expected_envelope_jsonl"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}

fn canonical(value: &Value) -> Vec<u8> {
    let parsed = parse(&serde_json::to_vec(value).unwrap()).unwrap();
    format!("{}\n", canonicalize(&parsed).unwrap()).into_bytes()
}

fn raw_replace_case(case_index: usize, field: &str, replacement: &str) -> Vec<u8> {
    let fixture = native_bundle_fixture();
    let input = fixture["cases"][case_index]["expected_envelope_jsonl"]
        .as_str()
        .unwrap();
    let start = format!(r#""{field}":"#);
    let value_start = input.find(&start).unwrap() + start.len();
    let value_end = input[value_start + 1..].find('"').unwrap() + value_start + 1;
    format!(
        "{}{}{}",
        &input[..value_start],
        replacement,
        &input[value_end + 1..]
    )
    .into_bytes()
}

#[test]
fn each_top_level_field_reports_missing_and_whole_field_wrong_type() {
    let fields = [
        ("schema", EnvelopeErrorField::Schema),
        ("bundle_id", EnvelopeErrorField::BundleId),
        ("source_family", EnvelopeErrorField::SourceFamily),
        ("source_hash", EnvelopeErrorField::SourceHash),
        ("raw_retention", EnvelopeErrorField::RawRetention),
        ("row_count", EnvelopeErrorField::RowCount),
        ("days", EnvelopeErrorField::Days),
        ("shards", EnvelopeErrorField::Shards),
        ("ledger", EnvelopeErrorField::Ledger),
        ("summary_plan", EnvelopeErrorField::SummaryPlan),
    ];
    for (field, expected) in fields {
        let mut missing = valid();
        missing.as_object_mut().unwrap().remove(field);
        let error = decode_body_envelope(&canonical(&missing)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::MissingField, "{field}");
        assert_eq!(error.field(), expected, "{field}");

        let decoys = match field {
            "schema" | "bundle_id" | "source_family" | "source_hash" | "raw_retention" => {
                vec![json!(null), json!(true), json!(1.5), json!({}), json!([])]
            }
            "row_count" => vec![
                json!(null),
                json!(true),
                json!(1.5),
                json!("text"),
                json!({}),
                json!([]),
            ],
            "days" | "shards" => {
                vec![
                    json!(null),
                    json!(true),
                    json!(1.5),
                    json!("text"),
                    json!({}),
                ]
            }
            "ledger" => vec![
                json!(null),
                json!(true),
                json!(1.5),
                json!("text"),
                json!([]),
            ],
            "summary_plan" => vec![json!(true), json!(1.5), json!("text"), json!([])],
            _ => unreachable!("all top-level fields are covered"),
        };
        for decoy in decoys {
            let mut wrong_type = valid();
            wrong_type
                .as_object_mut()
                .unwrap()
                .insert(field.into(), decoy);
            let error = decode_body_envelope(&canonical(&wrong_type)).unwrap_err();
            assert_eq!(error.code(), EnvelopeErrorCode::WrongType, "{field}");
            assert_eq!(error.field(), expected, "{field}");
        }
    }
}

#[test]
fn invalid_top_level_values_and_integer_boundaries_are_classified() {
    for (field, value, expected) in [
        ("schema", json!("wrong"), EnvelopeErrorField::Schema),
        (
            "bundle_id",
            json!("body-invalid"),
            EnvelopeErrorField::BundleId,
        ),
        (
            "source_family",
            json!("unknown"),
            EnvelopeErrorField::SourceFamily,
        ),
        ("source_hash", json!("bad"), EnvelopeErrorField::SourceHash),
        (
            "raw_retention",
            json!("unknown"),
            EnvelopeErrorField::RawRetention,
        ),
    ] {
        let mut input = valid();
        input.as_object_mut().unwrap().insert(field.into(), value);
        let error = decode_body_envelope(&canonical(&input)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::InvalidField, "{field}");
        assert_eq!(error.field(), expected, "{field}");
    }
    for case_index in [0, 1] {
        for replacement in ["\"\\u00e9\"", "\"\\ud800\""] {
            let error =
                decode_body_envelope(&raw_replace_case(case_index, "source_hash", replacement))
                    .unwrap_err();
            assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
            assert_eq!(error.field(), EnvelopeErrorField::SourceHash);
        }
        for replacement in ["\"\\u00e9\"", "\"invalid\""] {
            let error =
                decode_body_envelope(&raw_replace_case(case_index, "raw_retention", replacement))
                    .unwrap_err();
            assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
            assert_eq!(error.field(), EnvelopeErrorField::RawRetention);
        }
    }
    for replacement in ["-1", "18446744073709551616"] {
        let input = native_bundle_fixture()["cases"][0]["expected_envelope_jsonl"]
            .as_str()
            .unwrap()
            .replace("\"row_count\":1", &format!("\"row_count\":{replacement}"));
        let error = decode_body_envelope(input.as_bytes()).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
        assert_eq!(error.field(), EnvelopeErrorField::RowCount);
    }
    let mut fractional = valid();
    fractional["row_count"] = json!(1.5);
    let error = decode_body_envelope(&canonical(&fractional)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::RowCount);

    let mut oura: Value = serde_json::from_str(
        native_bundle_fixture()["cases"][1]["expected_envelope_jsonl"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    oura["raw_retention"] = json!("retain_complete");
    let error = decode_body_envelope(&canonical(&oura)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::IncompatibleField);
    assert_eq!(error.field(), EnvelopeErrorField::RawRetention);
}

#[test]
fn correlated_u64_maximum_bytes_round_trip_exactly() {
    let mut input = valid();
    input["shards"][0]["bytes"] = json!(u64::MAX);
    input["ledger"]["bytes"] = json!(u64::MAX);
    let input = canonical(&input);
    let envelope = decode_body_envelope(&input).expect("maximum byte counts remain valid");
    assert_eq!(envelope.row_count(), 1);
    assert_eq!(envelope.shards()[0].bytes(), u64::MAX);
    assert_eq!(envelope.shards()[0].rows(), 1);
    assert_eq!(envelope.ledger().bytes(), u64::MAX);
    assert_eq!(envelope.ledger().events(), 1);
    assert_eq!(
        solstone_core_body_source::encode_body_envelope(&envelope).unwrap(),
        input
    );
}

#[test]
fn pre_bundle_errors_are_unbound_and_later_errors_bind_the_bundle() {
    for (field, value, expected) in [
        ("schema", json!("wrong"), EnvelopeErrorField::Schema),
        ("bundle_id", json!("invalid"), EnvelopeErrorField::BundleId),
    ] {
        let mut input = valid();
        input.as_object_mut().unwrap().insert(field.into(), value);
        let error = decode_body_envelope(&canonical(&input)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
        assert_eq!(error.field(), expected);
        assert_eq!(error.bundle(), None);
    }

    let mut input = valid();
    input
        .as_object_mut()
        .unwrap()
        .insert("source_family".into(), json!("wrong"));
    let error = decode_body_envelope(&canonical(&input)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::SourceFamily);
    assert!(error.bundle().is_some());
}
