// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_body_source::{
    BodyDigest, BodyLedgerEvent, CandidateError, Coordinate, LedgerCandidate, LedgerEventErrorCode,
    LedgerEventErrorField, PresentationRow, decode_body_envelope, parse, project,
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

fn native_context(index: usize) -> (solstone_core_body_source::BodyEnvelope, Map<String, Value>) {
    let fixture = native_bundle_fixture();
    let case = &fixture["cases"][index];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("fixture envelope decodes");
    let row: Value = serde_json::from_str(
        case["expected_normalized_jsonl"]
            .as_str()
            .expect("normalized row"),
    )
    .expect("normalized JSON parses");
    let Value::Object(row) = row else {
        unreachable!("fixture row is object")
    };
    (envelope, row)
}

fn multishard_context() -> (solstone_core_body_source::BodyEnvelope, Map<String, Value>) {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("fixture envelope decodes");
    let first_row = case["shards"][0]["expected_jsonl"]
        .as_str()
        .expect("January rows")
        .lines()
        .next()
        .expect("January first row");
    let Value::Object(row) = serde_json::from_str(first_row).expect("normalized JSON parses")
    else {
        unreachable!("fixture row is object")
    };
    (envelope, row)
}

fn project_row(row: Map<String, Value>) -> Result<LedgerCandidate, CandidateError> {
    let encoded = serde_json::to_string(&Value::Object(row)).unwrap();
    let value = parse(encoded.as_bytes()).unwrap();
    let coordinate = Coordinate::new("bundle", "shard", 1);
    let presentation = PresentationRow::new(&value, &coordinate).unwrap();
    project(&presentation, coordinate)
}

fn bind(
    envelope: &solstone_core_body_source::BodyEnvelope,
    sequence: u64,
    shard_index: u64,
    line: u64,
    candidate: &LedgerCandidate,
) -> Result<BodyLedgerEvent, solstone_core_body_source::LedgerEventError> {
    BodyLedgerEvent::new(
        envelope,
        sequence,
        shard_index,
        line,
        digest(ROW_SHA256),
        digest(VALUE_HASH),
        candidate,
    )
}

fn assert_error(
    result: Result<BodyLedgerEvent, solstone_core_body_source::LedgerEventError>,
    code: LedgerEventErrorCode,
    field: LedgerEventErrorField,
    envelope: &solstone_core_body_source::BodyEnvelope,
    sequence: u64,
) {
    let error = result.expect_err("binding should fail");
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
    assert_eq!(error.bundle(), Some(envelope.bundle_id()));
    assert_eq!(error.line(), sequence);
}

#[test]
fn validation_precedence_and_location_follow_the_ten_stages() {
    let (envelope, valid) = context();
    let candidate = project_row(valid.clone()).unwrap();
    assert_error(
        bind(&envelope, 0, 99, 0, &candidate),
        LedgerEventErrorCode::InvalidSequence,
        LedgerEventErrorField::Sequence,
        &envelope,
        0,
    );
    assert_error(
        bind(&envelope, 1, u64::MAX, 0, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
        &envelope,
        1,
    );
    assert_error(
        bind(&envelope, 1, 0, 0, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Line,
        &envelope,
        1,
    );
    assert_error(
        bind(&envelope, 1, 0, u64::MAX, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Line,
        &envelope,
        1,
    );

    let mut normalized = valid.clone();
    normalized.insert("schema".into(), json!("solstone.health.normalized.v1"));
    let candidate = project_row(normalized).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::IncompatibleField,
        LedgerEventErrorField::RowSchema,
        &envelope,
        1,
    );

    let (oura_envelope, mut apple_for_oura) = native_context(1);
    apple_for_oura.insert("schema".into(), json!("solstone.health.apple_health.v1"));
    apple_for_oura.insert("source_family".into(), json!("apple_health"));
    let candidate = project_row(apple_for_oura).unwrap();
    assert_error(
        bind(&oura_envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::IncompatibleField,
        LedgerEventErrorField::RowSchema,
        &oura_envelope,
        1,
    );

    let mut oura_for_apple = valid.clone();
    oura_for_apple.insert("schema".into(), json!("solstone.health.oura.v1"));
    oura_for_apple.insert("source_family".into(), json!("oura_api"));
    let candidate = project_row(oura_for_apple).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::IncompatibleField,
        LedgerEventErrorField::RowSchema,
        &envelope,
        1,
    );

    let mut import_id = valid.clone();
    import_id.insert("import_id".into(), Value::Null);
    let candidate = project_row(import_id).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::BundleId,
        &envelope,
        1,
    );

    let mut missing_import_id = valid.clone();
    missing_import_id.remove("import_id");
    let candidate = project_row(missing_import_id).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::BundleId,
        &envelope,
        1,
    );

    let mut wrong_import_id = valid.clone();
    wrong_import_id.insert(
        "import_id".into(),
        json!(envelope.bundle_id().as_str().to_uppercase()),
    );
    let candidate = project_row(wrong_import_id).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::BundleId,
        &envelope,
        1,
    );

    let mut month = valid.clone();
    month.insert("month".into(), Value::Null);
    let candidate = project_row(month).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
        &envelope,
        1,
    );

    let mut missing_month = valid.clone();
    missing_month.remove("month");
    let candidate = project_row(missing_month).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
        &envelope,
        1,
    );

    let mut wrong_month = valid.clone();
    wrong_month.insert("month".into(), json!("2026-01 "));
    let candidate = project_row(wrong_month).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
        &envelope,
        1,
    );

    let mut invalid_day = valid.clone();
    invalid_day.insert("day".into(), json!("invalid"));
    let candidate = project_row(invalid_day).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::Day,
        &envelope,
        1,
    );

    let mut absent_day = valid.clone();
    absent_day.insert("day".into(), json!("20260103"));
    let candidate = project_row(absent_day).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Day,
        &envelope,
        1,
    );

    let mut normalized_ref = valid.clone();
    normalized_ref.insert("normalized_ref".into(), json!("imports/wrong"));
    let candidate = project_row(normalized_ref).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::NormalizedRef,
        &envelope,
        1,
    );

    let mut near_normalized_ref = valid.clone();
    near_normalized_ref.insert(
        "normalized_ref".into(),
        json!(format!(
            "imports/{}/normalized/2026-01.jsonl#L2",
            envelope.bundle_id().as_str()
        )),
    );
    let candidate = project_row(near_normalized_ref).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::NormalizedRef,
        &envelope,
        1,
    );

    let mut dedupe = valid.clone();
    dedupe.insert("dedupe_key".into(), json!("sha256:ABC"));
    let candidate = project_row(dedupe).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::DedupeKey,
        &envelope,
        1,
    );

    let mut uppercase_dedupe = valid.clone();
    let uppercase = ROW_SHA256
        .strip_prefix("sha256:")
        .expect("test digest has prefix")
        .to_uppercase();
    uppercase_dedupe.insert("dedupe_key".into(), json!(format!("sha256:{uppercase}")));
    let candidate = project_row(uppercase_dedupe).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::DedupeKey,
        &envelope,
        1,
    );

    let mut raw_ref = valid;
    raw_ref.insert("raw_ref".into(), json!("imports/wrong/raw/file"));
    let candidate = project_row(raw_ref).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::RawRef,
        &envelope,
        1,
    );
}

#[test]
fn accepts_boundary_values_and_collapses_optional_states() {
    let (envelope, mut row) = context();
    row.insert("source_record_id".into(), Value::Null);
    row.insert("end_date".into(), Value::Null);
    row.remove("raw_ref");
    let candidate = project_row(row).unwrap();
    let event = bind(&envelope, 1, 0, 1, &candidate).expect("boundary event binds");
    assert!(event.source_record_id().is_none());
    assert!(event.end_time().is_none());
    assert!(event.raw_ref().is_none());

    assert_error(
        bind(&envelope, 2, 0, 1, &candidate),
        LedgerEventErrorCode::InvalidSequence,
        LedgerEventErrorField::Sequence,
        &envelope,
        2,
    );
    assert_error(
        bind(&envelope, 1, 1, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
        &envelope,
        1,
    );
    assert_error(
        bind(&envelope, 1, 0, 2, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Line,
        &envelope,
        1,
    );
}

#[test]
fn rejects_a_real_day_in_the_selected_shards_wrong_month() {
    let (envelope, mut row) = multishard_context();
    row.insert("day".into(), json!("20260201"));
    let candidate = project_row(row).unwrap();
    assert_error(
        bind(&envelope, 1, 0, 1, &candidate),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Day,
        &envelope,
        1,
    );
}

#[test]
fn multishard_sequence_boundaries_include_the_declared_row_count() {
    let (envelope, row) = multishard_context();
    let candidate = project_row(row).unwrap();
    assert!(bind(&envelope, 3, 0, 1, &candidate).is_ok());
    assert_error(
        bind(&envelope, 4, 0, 1, &candidate),
        LedgerEventErrorCode::InvalidSequence,
        LedgerEventErrorField::Sequence,
        &envelope,
        4,
    );
}
