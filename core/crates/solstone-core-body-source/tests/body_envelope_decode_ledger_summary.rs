// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_body_source::{
    EnvelopeErrorCode, EnvelopeErrorField, canonicalize, decode_body_envelope, parse,
};

mod support;

use support::native_bundle_fixture;

fn apple() -> Value {
    serde_json::from_str(
        native_bundle_fixture()["cases"][0]["expected_envelope_jsonl"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}

fn oura() -> Value {
    serde_json::from_str(
        native_bundle_fixture()["cases"][1]["expected_envelope_jsonl"]
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
fn ledger_local_fields_and_constructor_errors_are_preserved() {
    let mut path = apple();
    path["ledger"]["path"] = json!("ledger.jsonl");
    let error = decode_body_envelope(&canonical(&path)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::LedgerPath);
    assert_eq!(error.index(), None);

    let mut constructor = apple();
    constructor["ledger"]["bytes"] = json!(0);
    constructor["ledger"]["events"] = json!(1);
    let error = decode_body_envelope(&canonical(&constructor)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::IncompatibleField);
    assert_eq!(error.field(), EnvelopeErrorField::LedgerBytes);
}

#[test]
fn summary_plan_is_locally_checked_then_family_rules_pass_through() {
    let mut wrong_type = apple();
    wrong_type["summary_plan"] = json!(false);
    let error = decode_body_envelope(&canonical(&wrong_type)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryPlan);

    let mut schema = apple();
    schema["summary_plan"]["schema"] = json!("wrong");
    let error = decode_body_envelope(&canonical(&schema)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::SummarySchema);

    let mut incompatible = oura();
    incompatible["summary_plan"] = apple()["summary_plan"].clone();
    let error = decode_body_envelope(&canonical(&incompatible)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::IncompatibleField);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryPlan);
}

#[test]
fn ledger_bytes_events_and_digest_each_have_missing_wrong_and_invalid_coverage() {
    for (key, field) in [
        ("bytes", EnvelopeErrorField::LedgerBytes),
        ("events", EnvelopeErrorField::LedgerEvents),
        ("sha256", EnvelopeErrorField::LedgerSha256),
    ] {
        let mut missing = apple();
        missing["ledger"].as_object_mut().unwrap().remove(key);
        let error = decode_body_envelope(&canonical(&missing)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::MissingField);
        assert_eq!(error.field(), field);

        let mut wrong = apple();
        wrong["ledger"][key] = json!(false);
        let error = decode_body_envelope(&canonical(&wrong)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
        assert_eq!(error.field(), field);
    }
    for (key, field) in [
        ("bytes", EnvelopeErrorField::LedgerBytes),
        ("events", EnvelopeErrorField::LedgerEvents),
    ] {
        let mut invalid = apple();
        invalid["ledger"][key] = json!(-1);
        let error = decode_body_envelope(&canonical(&invalid)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
        assert_eq!(error.field(), field);
    }
    let mut digest = apple();
    digest["ledger"]["sha256"] = json!("sha256:bad");
    let error = decode_body_envelope(&canonical(&digest)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::LedgerSha256);
}

#[test]
fn summary_plan_presence_days_and_ordering_are_delegated_correctly() {
    let mut absent = apple();
    absent["summary_plan"] = json!(null);
    let error = decode_body_envelope(&canonical(&absent)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::MissingField);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryPlan);

    let mut missing_days = apple();
    missing_days["summary_plan"]
        .as_object_mut()
        .unwrap()
        .remove("days");
    let error = decode_body_envelope(&canonical(&missing_days)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::MissingField);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryDays);

    let mut wrong_days = apple();
    wrong_days["summary_plan"]["days"] = json!(false);
    let error = decode_body_envelope(&canonical(&wrong_days)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryDays);

    let mut reversed = apple();
    reversed["summary_plan"]["days"] = json!(["20260103", "20260102"]);
    let error = decode_body_envelope(&canonical(&reversed)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryDays);
    assert_eq!(error.index(), None);
}
