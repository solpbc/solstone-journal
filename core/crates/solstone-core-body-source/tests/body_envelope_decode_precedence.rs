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

fn assert_error(input: &[u8], code: EnvelopeErrorCode, field: EnvelopeErrorField) {
    let error = decode_body_envelope(input).unwrap_err();
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
}

#[test]
fn scan_then_unknown_then_fixed_top_level_projection_order() {
    let mut input = valid();
    input
        .as_object_mut()
        .unwrap()
        .insert("a_unknown".into(), json!(null));
    input
        .as_object_mut()
        .unwrap()
        .insert("schema".into(), json!(null));
    let error = decode_body_envelope(&canonical(&input)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::UnknownField);
    assert_eq!(error.field(), EnvelopeErrorField::Envelope);
    assert_eq!(error.bundle(), None);

    let mut input = valid();
    input
        .as_object_mut()
        .unwrap()
        .insert("schema".into(), json!("wrong"));
    input
        .as_object_mut()
        .unwrap()
        .insert("bundle_id".into(), json!("invalid"));
    let error = decode_body_envelope(&canonical(&input)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::Schema);
}

#[test]
fn nested_unknown_keys_precede_each_shape_local_field_order() {
    let mut shard = valid();
    shard["shards"][0]["a_unknown"] = json!([]);
    shard["shards"][0]["path"] = json!("wrong");
    let error = decode_body_envelope(&canonical(&shard)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::UnknownField);
    assert_eq!(error.field(), EnvelopeErrorField::Shards);
    assert_eq!(error.index(), Some(0));

    let mut ledger = valid();
    ledger["ledger"]["a_unknown"] = json!({"nested": null});
    ledger["ledger"]["path"] = json!("wrong");
    let error = decode_body_envelope(&canonical(&ledger)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::UnknownField);
    assert_eq!(error.field(), EnvelopeErrorField::Ledger);

    let mut plan = valid();
    plan["summary_plan"]["a_unknown"] = json!([null]);
    plan["summary_plan"]["schema"] = json!("wrong");
    let error = decode_body_envelope(&canonical(&plan)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::UnknownField);
    assert_eq!(error.field(), EnvelopeErrorField::SummaryPlan);
}

#[test]
fn every_top_level_field_observes_the_declared_fixed_validation_order() {
    let clean = valid();
    let mut input = clean.clone();
    input["schema"] = json!("wrong");
    input["bundle_id"] = json!("invalid");
    input["source_family"] = json!("invalid");
    input["source_hash"] = json!("invalid");
    input["raw_retention"] = json!("invalid");
    input["row_count"] = json!(-1);
    input["days"] = json!([null]);
    input["shards"] = json!([null]);
    input["ledger"] = json!(null);
    input["summary_plan"] = json!(true);
    input
        .as_object_mut()
        .unwrap()
        .insert("a_unknown".into(), json!({"nested": [null]}));
    assert_error(
        &canonical(&input),
        EnvelopeErrorCode::UnknownField,
        EnvelopeErrorField::Envelope,
    );
    input.as_object_mut().unwrap().remove("a_unknown");

    for (field, code, expected) in [
        (
            "schema",
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::Schema,
        ),
        (
            "bundle_id",
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::BundleId,
        ),
        (
            "source_family",
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::SourceFamily,
        ),
        (
            "source_hash",
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::SourceHash,
        ),
        (
            "raw_retention",
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::RawRetention,
        ),
        (
            "row_count",
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::RowCount,
        ),
        (
            "days",
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::Days,
        ),
        (
            "shards",
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::Shards,
        ),
        (
            "ledger",
            EnvelopeErrorCode::WrongType,
            EnvelopeErrorField::Ledger,
        ),
        (
            "summary_plan",
            EnvelopeErrorCode::WrongType,
            EnvelopeErrorField::SummaryPlan,
        ),
    ] {
        assert_error(&canonical(&input), code, expected);
        input[field] = clean[field].clone();
    }
}

#[test]
fn unknown_keys_include_empty_and_surrogate_spellings_but_values_are_not_keys() {
    let fixture = native_bundle_fixture();
    let base = fixture["cases"][0]["expected_envelope_jsonl"]
        .as_str()
        .unwrap();
    for input in [
        format!(r#"{{"":null,{}"#, &base[1..]).into_bytes(),
        format!("{},\"\\ud800\":null}}\n", base.strip_suffix("}\n").unwrap()).into_bytes(),
    ] {
        let error = decode_body_envelope(&input).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::UnknownField);
        assert_eq!(error.field(), EnvelopeErrorField::Envelope);
        assert!(error.to_string().len() <= 160);
    }

    let mut value = valid();
    value["schema"] = json!(r#"{"unknown":1}"#);
    assert_error(
        &canonical(&value),
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::Schema,
    );
}

#[test]
fn nested_objects_use_unknown_then_their_declared_local_field_order() {
    let mut shard = valid();
    shard["shards"][0]["path"] = json!("wrong");
    shard["shards"][0]["bytes"] = json!(false);
    shard["shards"][0]["rows"] = json!(false);
    shard["shards"][0]["sha256"] = json!("bad");
    assert_error(
        &canonical(&shard),
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardPath,
    );
    shard["shards"][0]["path"] = valid()["shards"][0]["path"].clone();
    assert_error(
        &canonical(&shard),
        EnvelopeErrorCode::WrongType,
        EnvelopeErrorField::ShardBytes,
    );
    shard["shards"][0]["bytes"] = valid()["shards"][0]["bytes"].clone();
    assert_error(
        &canonical(&shard),
        EnvelopeErrorCode::WrongType,
        EnvelopeErrorField::ShardRows,
    );
    shard["shards"][0]["rows"] = valid()["shards"][0]["rows"].clone();
    assert_error(
        &canonical(&shard),
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardSha256,
    );

    let mut ledger = valid();
    ledger["ledger"]["path"] = json!("wrong");
    ledger["ledger"]["bytes"] = json!(false);
    ledger["ledger"]["events"] = json!(false);
    ledger["ledger"]["sha256"] = json!("bad");
    assert_error(
        &canonical(&ledger),
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::LedgerPath,
    );
    ledger["ledger"]["path"] = valid()["ledger"]["path"].clone();
    assert_error(
        &canonical(&ledger),
        EnvelopeErrorCode::WrongType,
        EnvelopeErrorField::LedgerBytes,
    );
    ledger["ledger"]["bytes"] = valid()["ledger"]["bytes"].clone();
    assert_error(
        &canonical(&ledger),
        EnvelopeErrorCode::WrongType,
        EnvelopeErrorField::LedgerEvents,
    );
    ledger["ledger"]["events"] = valid()["ledger"]["events"].clone();
    assert_error(
        &canonical(&ledger),
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::LedgerSha256,
    );

    let mut plan = valid();
    plan["summary_plan"]["schema"] = json!("wrong");
    plan["summary_plan"]["days"] = json!(null);
    assert_error(
        &canonical(&plan),
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::SummarySchema,
    );
    plan["summary_plan"]["schema"] = valid()["summary_plan"]["schema"].clone();
    assert_error(
        &canonical(&plan),
        EnvelopeErrorCode::WrongType,
        EnvelopeErrorField::SummaryDays,
    );
}
