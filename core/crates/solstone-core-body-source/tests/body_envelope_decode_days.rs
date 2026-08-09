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

#[test]
fn days_whole_field_and_elements_are_indexed_correctly() {
    let mut whole = valid();
    whole["days"] = json!(null);
    let error = decode_body_envelope(&canonical(&whole)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::Days);
    assert_eq!(error.index(), None);

    let mut element = valid();
    element["days"] = json!(["20260102", null]);
    let error = decode_body_envelope(&canonical(&element)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::Days);
    assert_eq!(error.index(), Some(1));

    let mut invalid = valid();
    invalid["days"] = json!(["20260102", "20260230"]);
    let error = decode_body_envelope(&canonical(&invalid)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::Days);
    assert_eq!(error.index(), Some(1));
}

#[test]
fn aggregate_day_order_errors_pass_through_after_local_projection() {
    let mut input = valid();
    input["days"] = json!(["20260103", "20260102"]);
    let error = decode_body_envelope(&canonical(&input)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::Days);
    assert_eq!(error.index(), Some(1));
}

#[test]
fn summary_days_report_nonzero_element_indices() {
    for (days, expected_code) in [
        (json!(["20260102", null]), EnvelopeErrorCode::WrongType),
        (
            json!(["20260102", "20260230"]),
            EnvelopeErrorCode::InvalidField,
        ),
    ] {
        let mut input = valid();
        input["summary_plan"]["days"] = days;
        let error = decode_body_envelope(&canonical(&input)).unwrap_err();
        assert_eq!(error.code(), expected_code);
        assert_eq!(error.field(), EnvelopeErrorField::SummaryDays);
        assert_eq!(error.index(), Some(1));
    }
}

#[test]
fn aggregate_day_window_count_and_plan_agreement_errors_pass_through() {
    let mut window = valid();
    window["source_hash"] = json!(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#window:20260103:20260103"
    );
    let error = decode_body_envelope(&canonical(&window)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::IncompatibleField);
    assert_eq!(error.field(), EnvelopeErrorField::Days);
    assert_eq!(error.index(), Some(0));

    let mut count = valid();
    count["days"] = json!([]);
    count["summary_plan"]["days"] = json!([]);
    let error = decode_body_envelope(&canonical(&count)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::CountMismatch);
    assert_eq!(error.field(), EnvelopeErrorField::Days);
    assert_eq!(error.index(), None);

    let mut mismatch = valid();
    mismatch["summary_plan"]["days"] = json!([]);
    let error = decode_body_envelope(&canonical(&mismatch)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::CountMismatch);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryDays);
    assert_eq!(error.index(), None);
}

#[test]
fn first_aggregate_error_wins_when_days_and_shards_are_both_invalid() {
    let mut input = valid();
    input["days"] = json!(["20260103", "20260102"]);
    input["shards"][0]["path"] = json!("normalized/2026-02.jsonl");
    let error = decode_body_envelope(&canonical(&input)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::Days);
    assert_eq!(error.index(), Some(1));
}
