// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_body_source::{
    EnvelopeErrorCode, EnvelopeErrorField, canonicalize, decode_body_envelope, parse,
};

use crate::support;

use support::{envelope_multimonth_fixture, native_bundle_fixture};

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
fn shard_container_and_paths_report_their_own_fields_and_indices() {
    let mut whole = valid();
    whole["shards"] = json!(null);
    let error = decode_body_envelope(&canonical(&whole)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::Shards);
    assert_eq!(error.index(), None);

    let mut element = valid();
    element["shards"] = json!([null]);
    let error = decode_body_envelope(&canonical(&element)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::Shards);
    assert_eq!(error.index(), Some(0));

    let mut path = valid();
    path["shards"][0]["path"] = json!("normalized/2026-13.jsonl");
    let error = decode_body_envelope(&canonical(&path)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::ShardPath);
    assert_eq!(error.index(), Some(0));
}

#[test]
fn shard_leaf_and_constructor_errors_pass_through() {
    let mut wrong_type = valid();
    wrong_type["shards"][0]["bytes"] = json!(false);
    let error = decode_body_envelope(&canonical(&wrong_type)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::ShardBytes);
    assert_eq!(error.index(), Some(0));

    let mut constructor = valid();
    constructor["shards"][0]["bytes"] = json!(0);
    let error = decode_body_envelope(&canonical(&constructor)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::ShardBytes);
    assert_eq!(error.index(), Some(0));
}

#[test]
fn shard_paths_and_each_remaining_leaf_cover_missing_wrong_and_invalid_cases() {
    for path in ["wrong/2026-01.jsonl", "normalized/2026-01.txt"] {
        let mut input = valid();
        input["shards"][0]["path"] = json!(path);
        let error = decode_body_envelope(&canonical(&input)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
        assert_eq!(error.field(), EnvelopeErrorField::ShardPath);
    }
    for (key, field) in [
        ("rows", EnvelopeErrorField::ShardRows),
        ("sha256", EnvelopeErrorField::ShardSha256),
    ] {
        let mut missing = valid();
        missing["shards"][0].as_object_mut().unwrap().remove(key);
        let error = decode_body_envelope(&canonical(&missing)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::MissingField);
        assert_eq!(error.field(), field);

        let mut wrong = valid();
        wrong["shards"][0][key] = json!(false);
        let error = decode_body_envelope(&canonical(&wrong)).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
        assert_eq!(error.field(), field);
    }
    let mut zero_rows = valid();
    zero_rows["shards"][0]["rows"] = json!(0);
    let error = decode_body_envelope(&canonical(&zero_rows)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::ShardRows);

    let mut bad_digest = valid();
    bad_digest["shards"][0]["sha256"] = json!("sha256:bad");
    let error = decode_body_envelope(&canonical(&bad_digest)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::ShardSha256);
}

#[test]
fn shard_numeric_boundaries_and_later_indices_are_preserved() {
    let mut negative = valid();
    negative["shards"][0]["bytes"] = json!(-1);
    let error = decode_body_envelope(&canonical(&negative)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::ShardBytes);

    let raw = native_bundle_fixture()["cases"][0]["expected_envelope_jsonl"]
        .as_str()
        .unwrap()
        .replace("\"bytes\":894", "\"bytes\":18446744073709551616");
    let error = decode_body_envelope(raw.as_bytes()).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::ShardBytes);

    let mut second: Value = serde_json::from_str(
        envelope_multimonth_fixture()["cases"][0]["expected_envelope_jsonl"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    second["shards"][1]["rows"] = json!(false);
    let error = decode_body_envelope(&canonical(&second)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::WrongType);
    assert_eq!(error.field(), EnvelopeErrorField::ShardRows);
    assert_eq!(error.index(), Some(1));
}

#[test]
fn intrinsic_and_aggregate_shard_errors_pass_through_without_decoder_reimplementation() {
    let mut rows_over_bytes = valid();
    rows_over_bytes["shards"][0]["bytes"] = json!(1);
    rows_over_bytes["shards"][0]["rows"] = json!(2);
    let error = decode_body_envelope(&canonical(&rows_over_bytes)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::IncompatibleField);
    assert_eq!(error.field(), EnvelopeErrorField::ShardRows);

    let mut reverse: Value = serde_json::from_str(
        envelope_multimonth_fixture()["cases"][0]["expected_envelope_jsonl"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    reverse["shards"][0]["path"] = json!("normalized/2026-02.jsonl");
    reverse["shards"][1]["path"] = json!("normalized/2026-01.jsonl");
    let error = decode_body_envelope(&canonical(&reverse)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::InvalidField);
    assert_eq!(error.field(), EnvelopeErrorField::Shards);
    assert_eq!(error.index(), Some(1));

    let mut total = valid();
    total["row_count"] = json!(2);
    total["ledger"]["events"] = json!(2);
    let error = decode_body_envelope(&canonical(&total)).unwrap_err();
    assert_eq!(error.code(), EnvelopeErrorCode::CountMismatch);
    assert_eq!(error.field(), EnvelopeErrorField::ShardRows);
}
