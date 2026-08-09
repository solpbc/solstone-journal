// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_body_source::{
    BodyDigest, BodyLedgerEvent, BodyString, BodyValue, Coordinate, PresentationRow,
    decode_body_envelope, parse, project,
};

mod support;

use support::native_bundle_fixture;

const ROW_SHA256: &str = "sha256:8c5c69896ead27cbdb3f8e4c29b82f81ba6f62632fd4503b259bb07073573853";
const VALUE_HASH: &str = "sha256:c66e84c7099ced3708ba7b04aeb5c1b4c88f15f1a94296c7321f00a7ab030eda";

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("test digest is valid")
}

fn context() -> (solstone_core_body_source::BodyEnvelope, Map<String, Value>) {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("fixture envelope decodes");
    let Value::Object(row) = serde_json::from_str(
        case["expected_normalized_jsonl"]
            .as_str()
            .expect("normalized row"),
    )
    .expect("normalized JSON parses") else {
        unreachable!("fixture row is object")
    };
    (envelope, row)
}

fn bind(
    envelope: &solstone_core_body_source::BodyEnvelope,
    row: Map<String, Value>,
) -> BodyLedgerEvent {
    let row = serde_json::to_string(&Value::Object(row)).unwrap();
    let value = parse(row.as_bytes()).unwrap();
    let coordinate = Coordinate::new("bundle", "shard", 1);
    let presentation = PresentationRow::new(&value, &coordinate).unwrap();
    let candidate = project(&presentation, coordinate).unwrap();
    BodyLedgerEvent::new(
        envelope,
        1,
        0,
        1,
        digest(ROW_SHA256),
        digest(VALUE_HASH),
        &candidate,
    )
    .expect("valid event binds")
}

#[test]
fn accepts_valid_raw_ref_components_and_equality_is_field_sensitive() {
    let (envelope, row) = context();
    let event = bind(&envelope, row.clone());
    assert_eq!(event.clone(), event);

    let mut nested = row;
    nested.insert(
        "raw_ref".into(),
        json!(format!(
            "imports/{}/raw/deep/日本語/record",
            envelope.bundle_id().as_str()
        )),
    );
    let nested_event = bind(&envelope, nested);
    assert_ne!(event, nested_event);
    assert_eq!(
        nested_event
            .raw_ref()
            .expect("nested raw ref")
            .code_points()
            .last(),
        Some(&u32::from(b'd'))
    );
}

#[test]
fn raw_ref_rejects_each_forbidden_path_shape() {
    let (envelope, row) = context();
    let prefix = format!("imports/{}/raw/", envelope.bundle_id().as_str());
    for raw_ref in [
        "imports/wrong/raw/file".to_owned(),
        prefix.clone(),
        format!("{prefix}/file"),
        format!("{prefix}file/"),
        format!("{prefix}file//next"),
        format!("{prefix}./file"),
        format!("{prefix}file/../next"),
        format!("{prefix}file\0next"),
    ] {
        let mut candidate = row.clone();
        candidate.insert("raw_ref".into(), json!(raw_ref));
        let row = serde_json::to_string(&Value::Object(candidate)).unwrap();
        let value = parse(row.as_bytes()).unwrap();
        let coordinate = Coordinate::new("bundle", "shard", 1);
        let presentation = PresentationRow::new(&value, &coordinate).unwrap();
        let candidate = project(&presentation, coordinate).unwrap();
        let error = BodyLedgerEvent::new(
            &envelope,
            1,
            0,
            1,
            digest(ROW_SHA256),
            digest(VALUE_HASH),
            &candidate,
        )
        .expect_err("invalid raw ref refuses");
        assert_eq!(
            error.code(),
            solstone_core_body_source::LedgerEventErrorCode::InvalidField
        );
        assert_eq!(
            error.field(),
            solstone_core_body_source::LedgerEventErrorField::RawRef
        );
    }
}

#[test]
fn raw_ref_preserves_lone_surrogate_code_points() {
    let (envelope, row) = context();
    let text = serde_json::to_string(&Value::Object(row)).unwrap();
    let BodyValue::Object(mut object) = parse(text.as_bytes()).unwrap() else {
        unreachable!("fixture row is object")
    };
    let key = BodyString::from_code_points("raw_ref".bytes().map(u32::from).collect()).unwrap();
    let mut raw_ref: Vec<u32> = format!("imports/{}/raw/", envelope.bundle_id().as_str())
        .bytes()
        .map(u32::from)
        .collect();
    raw_ref.extend([u32::from(b'x'), 0xd800]);
    object.insert(
        key,
        BodyValue::String(BodyString::from_code_points(raw_ref.clone()).unwrap()),
    );
    let value = BodyValue::Object(object);
    let coordinate = Coordinate::new("bundle", "shard", 1);
    let presentation = PresentationRow::new(&value, &coordinate).unwrap();
    let candidate = project(&presentation, coordinate).unwrap();
    let event = BodyLedgerEvent::new(
        &envelope,
        1,
        0,
        1,
        digest(ROW_SHA256),
        digest(VALUE_HASH),
        &candidate,
    )
    .expect("lone surrogate is legal in a raw-ref component");
    assert_eq!(event.raw_ref().unwrap().code_points(), raw_ref);
}
