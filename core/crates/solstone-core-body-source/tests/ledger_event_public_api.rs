// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, BodyLedgerEvent, BodyRawRetention, BodySourceFamily, BodySourceHash,
    BodyString, BodyValue, BundleId, Coordinate, PresentationRow, decode_body_envelope, parse,
    project,
};

mod support;

use support::{ledger_events_fixture, native_bundle_fixture};

const ROW_SHA256: &str = "sha256:8c5c69896ead27cbdb3f8e4c29b82f81ba6f62632fd4503b259bb07073573853";
const VALUE_HASH: &str = "sha256:c66e84c7099ced3708ba7b04aeb5c1b4c88f15f1a94296c7321f00a7ab030eda";

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("test digest is valid")
}

fn context() -> (solstone_core_body_source::BodyEnvelope, Map<String, Value>) {
    native_context(0)
}

fn native_context(index: usize) -> (BodyEnvelope, Map<String, Value>) {
    let case = &native_bundle_fixture()["cases"][index];
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
    bind_with(
        envelope,
        row,
        1,
        0,
        1,
        digest(ROW_SHA256),
        digest(VALUE_HASH),
    )
}

#[allow(clippy::too_many_arguments)]
fn bind_with(
    envelope: &BodyEnvelope,
    row: Map<String, Value>,
    sequence: u64,
    shard_index: u64,
    line: u64,
    row_sha256: BodyDigest,
    value_hash: BodyDigest,
) -> BodyLedgerEvent {
    let row = serde_json::to_string(&Value::Object(row)).unwrap();
    let value = parse(row.as_bytes()).unwrap();
    let coordinate = Coordinate::new("bundle", "shard", 1);
    let presentation = PresentationRow::new(&value, &coordinate).unwrap();
    let candidate = project(&presentation, coordinate).unwrap();
    BodyLedgerEvent::new(
        envelope,
        sequence,
        shard_index,
        line,
        row_sha256,
        value_hash,
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
fn raw_ref_preserves_the_shipping_filename_character_domain() {
    let (envelope, row) = context();
    let prefix = format!("imports/{}/raw/", envelope.bundle_id().as_str());
    let mut components = vec![
        "space name".to_owned(),
        "日本語-é".to_owned(),
        r"back\slash:percent%question?hash#one#two".to_owned(),
    ];
    for code_point in (1_u32..=0x1f).chain(0x7f..=0x9f) {
        components.push(format!(
            "before{}after",
            char::from_u32(code_point).expect("test code point is scalar")
        ));
    }
    for component in components {
        let raw_ref = format!("{prefix}outer/{component}");
        let mut candidate = row.clone();
        candidate.insert("raw_ref".into(), json!(raw_ref));
        let event = bind(&envelope, candidate);
        assert_eq!(
            event.raw_ref().unwrap().code_points(),
            raw_ref.chars().map(u32::from).collect::<Vec<_>>()
        );
    }

    let mut null = row;
    null.insert("raw_ref".into(), Value::Null);
    assert!(bind(&envelope, null).raw_ref().is_none());
}

#[test]
fn raw_ref_rejects_each_forbidden_path_shape() {
    let (envelope, row) = context();
    let prefix = format!("imports/{}/raw/", envelope.bundle_id().as_str());
    for raw_ref in [
        "imports/wrong/raw/file".to_owned(),
        format!("Imports/{}/raw/file", envelope.bundle_id().as_str()),
        format!("imports/{}/Raw/file", envelope.bundle_id().as_str()),
        "imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/raw/file".to_owned(),
        prefix.clone(),
        format!("{prefix}/file"),
        format!("{prefix}file/"),
        format!("{prefix}file//next"),
        format!("{prefix}./file"),
        format!("{prefix}file/./next"),
        format!("{prefix}file/."),
        format!("{prefix}../file"),
        format!("{prefix}file/../next"),
        format!("{prefix}file/.."),
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

#[test]
fn public_equality_distinguishes_each_constructible_semantic_group() {
    let (envelope, row) = context();
    let baseline = bind(&envelope, row.clone());
    assert_eq!(baseline.clone(), baseline);
    assert_ne!(
        baseline,
        bind_with(
            &envelope,
            row.clone(),
            1,
            0,
            1,
            digest("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            digest(VALUE_HASH),
        )
    );
    assert_ne!(
        baseline,
        bind_with(
            &envelope,
            row.clone(),
            1,
            0,
            1,
            digest(ROW_SHA256),
            digest("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        )
    );

    let replacement_bundle = BundleId::from_bytes(b"body-01J9ZK2F5M7Q8R3S4T6V0W1X34").unwrap();
    let replacement_envelope = BodyEnvelope::new(
        replacement_bundle.clone(),
        envelope.source_family(),
        envelope.source_hash().clone(),
        envelope.raw_retention(),
        envelope.row_count(),
        envelope.days().to_vec(),
        envelope.shards().to_vec(),
        envelope.ledger().clone(),
        envelope.summary_plan().cloned(),
    )
    .unwrap();
    let old_bundle = envelope.bundle_id().as_str();
    let new_bundle = replacement_bundle.as_str();
    let mut replacement_row = row.clone();
    for field in ["import_id", "normalized_ref", "raw_ref"] {
        let value = replacement_row[field]
            .as_str()
            .unwrap()
            .replace(old_bundle, new_bundle);
        replacement_row.insert(field.into(), json!(value));
    }
    assert_ne!(baseline, bind(&replacement_envelope, replacement_row));

    let plain_hash = envelope
        .source_hash()
        .as_str()
        .split('#')
        .next()
        .expect("source hash has a base digest");
    let oura_equivalent = BodyEnvelope::new(
        envelope.bundle_id().clone(),
        BodySourceFamily::OuraApi,
        BodySourceHash::from_bytes_for_family(plain_hash.as_bytes(), &BodySourceFamily::OuraApi)
            .unwrap(),
        BodyRawRetention::RetainParsed,
        envelope.row_count(),
        envelope.days().to_vec(),
        envelope.shards().to_vec(),
        envelope.ledger().clone(),
        None,
    )
    .unwrap();
    let mut oura_equivalent_row = row.clone();
    oura_equivalent_row.insert("schema".into(), json!("solstone.health.oura.v1"));
    oura_equivalent_row.insert("source_family".into(), json!("oura_api"));
    let schema_family_twin = bind(&oura_equivalent, oura_equivalent_row);
    assert_eq!(baseline.bundle_id(), schema_family_twin.bundle_id());
    assert_eq!(baseline.sequence(), schema_family_twin.sequence());
    assert_eq!(baseline.shard(), schema_family_twin.shard());
    assert_eq!(baseline.line(), schema_family_twin.line());
    assert_eq!(
        baseline.normalized_ref(),
        schema_family_twin.normalized_ref()
    );
    assert_eq!(baseline.row_sha256(), schema_family_twin.row_sha256());
    assert_eq!(baseline.dedupe_key(), schema_family_twin.dedupe_key());
    assert_eq!(
        baseline.source_record_id(),
        schema_family_twin.source_record_id()
    );
    assert_eq!(baseline.record_type(), schema_family_twin.record_type());
    assert_eq!(baseline.start_time(), schema_family_twin.start_time());
    assert_eq!(baseline.end_time(), schema_family_twin.end_time());
    assert_eq!(baseline.day(), schema_family_twin.day());
    assert_eq!(baseline.value_hash(), schema_family_twin.value_hash());
    assert_eq!(baseline.raw_ref(), schema_family_twin.raw_ref());
    assert_ne!(baseline.row_schema(), schema_family_twin.row_schema());
    assert_ne!(baseline.source_family(), schema_family_twin.source_family());
    assert_ne!(baseline, schema_family_twin);

    let fixture = ledger_events_fixture();
    let case = &fixture["cases"][0];
    let multishard =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()).unwrap();
    let january_first = case["shards"][0]["expected_jsonl"]
        .as_str()
        .unwrap()
        .lines()
        .next()
        .unwrap();
    let Value::Object(january_first) = serde_json::from_str(january_first).unwrap() else {
        unreachable!()
    };
    let first = bind_with(
        &multishard,
        january_first.clone(),
        1,
        0,
        1,
        digest(ROW_SHA256),
        digest(VALUE_HASH),
    );
    let second_sequence = bind_with(
        &multishard,
        january_first.clone(),
        2,
        0,
        1,
        digest(ROW_SHA256),
        digest(VALUE_HASH),
    );
    assert_eq!(first.bundle_id(), second_sequence.bundle_id());
    assert_eq!(first.row_schema(), second_sequence.row_schema());
    assert_eq!(first.shard(), second_sequence.shard());
    assert_eq!(first.line(), second_sequence.line());
    assert_eq!(first.normalized_ref(), second_sequence.normalized_ref());
    assert_eq!(first.row_sha256(), second_sequence.row_sha256());
    assert_eq!(first.dedupe_key(), second_sequence.dedupe_key());
    assert_eq!(first.source_family(), second_sequence.source_family());
    assert_eq!(first.source_record_id(), second_sequence.source_record_id());
    assert_eq!(first.record_type(), second_sequence.record_type());
    assert_eq!(first.start_time(), second_sequence.start_time());
    assert_eq!(first.end_time(), second_sequence.end_time());
    assert_eq!(first.day(), second_sequence.day());
    assert_eq!(first.value_hash(), second_sequence.value_hash());
    assert_eq!(first.raw_ref(), second_sequence.raw_ref());
    assert_ne!(first.sequence(), second_sequence.sequence());

    let mut other_shard_row = january_first;
    other_shard_row.insert("month".into(), json!("2026-02"));
    other_shard_row.insert("day".into(), json!("20260201"));
    other_shard_row.insert(
        "normalized_ref".into(),
        json!(format!(
            "imports/{}/normalized/2026-02.jsonl#L1",
            multishard.bundle_id().as_str()
        )),
    );
    let other_shard = bind_with(
        &multishard,
        other_shard_row,
        1,
        1,
        1,
        digest(ROW_SHA256),
        digest(VALUE_HASH),
    );
    assert_eq!(first.bundle_id(), other_shard.bundle_id());
    assert_eq!(first.sequence(), other_shard.sequence());
    assert_eq!(first.row_schema(), other_shard.row_schema());
    assert_eq!(first.line(), other_shard.line());
    assert_eq!(first.row_sha256(), other_shard.row_sha256());
    assert_eq!(first.dedupe_key(), other_shard.dedupe_key());
    assert_eq!(first.source_family(), other_shard.source_family());
    assert_eq!(first.source_record_id(), other_shard.source_record_id());
    assert_eq!(first.record_type(), other_shard.record_type());
    assert_eq!(first.start_time(), other_shard.start_time());
    assert_eq!(first.end_time(), other_shard.end_time());
    assert_eq!(first.value_hash(), other_shard.value_hash());
    assert_eq!(first.raw_ref(), other_shard.raw_ref());
    assert_ne!(first.shard(), other_shard.shard());
    assert_ne!(first.normalized_ref(), other_shard.normalized_ref());
    assert_ne!(first.day(), other_shard.day());
    assert_ne!(first, second_sequence);
    assert_ne!(first, other_shard);
}
