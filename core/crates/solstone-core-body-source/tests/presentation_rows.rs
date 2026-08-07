// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;

use solstone_core_body_source::{
    BodyInteger, BodyString, BodyValue, CandidateErrorCode, CandidateErrorField, Coordinate,
    FieldState, LedgerSchema, PresentationRow, ValueState, canonicalize, parse, project,
};

mod support;

use support::{assert_body_value_bitwise_eq, codec_rows};

fn assert_body_string(actual: &BodyString, expected: &str) {
    assert_eq!(
        actual.code_points(),
        expected.bytes().map(u32::from).collect::<Vec<_>>()
    );
}

fn assert_present(actual: &FieldState<BodyString>, expected: &str) {
    let FieldState::Present(actual) = actual else {
        panic!("expected present string field");
    };
    assert_body_string(actual, expected);
}

fn metadata_from_json(value: &serde_json::Value) -> BodyValue {
    let json = serde_json::to_string(value).expect("metadata should serialize");
    parse(json.as_bytes()).expect("metadata should parse")
}

#[test]
fn codec_rows_project_without_mutating_presentation_values() {
    let fixture = codec_rows();
    for row in fixture["rows"].as_array().expect("rows") {
        let name = row["name"].as_str().expect("row name");
        let compact = serde_json::to_string(&row["row"]).expect("row should serialize");
        let presentation = PresentationRow::from(parse(compact.as_bytes()).expect("row parses"));
        let expected_canonical = row["expected_canonical_json"]
            .as_str()
            .expect("expected canonical JSON");

        assert_eq!(
            canonicalize(presentation.value()).expect("row canonicalizes"),
            expected_canonical,
            "{name} before projection"
        );
        let candidate = project(&presentation, Coordinate::new("bundle", "shard", 1))
            .expect("fixture row projects");
        assert_eq!(
            canonicalize(presentation.value()).expect("row canonicalizes"),
            expected_canonical,
            "{name} after projection"
        );

        match name {
            "apple_v1_all_shapes" => {
                assert_eq!(candidate.schema(), LedgerSchema::AppleHealthV1);
                assert_body_string(candidate.source_family(), "apple_health");
                assert_body_string(candidate.record_type(), "HKWorkoutActivityTypeRunning");
                assert_body_string(
                    candidate.dedupe_key(),
                    "sha256:1422cf525aecd1153993ec0d4dbbd3b7f192ae5523ec22f5b8ccfc8d203a93a5",
                );
                assert_body_string(candidate.start_date(), "2026-01-02 06:30:00 -0700");
                assert_body_string(candidate.day(), "20260102");
                assert_present(candidate.kind(), "workout");
                assert_present(candidate.import_id(), "synthetic-import");
                assert_present(candidate.month(), "2026-01");
                assert_present(candidate.end_date(), "2026-01-02 07:15:00 -0700");
                assert_eq!(candidate.source_record_id(), &FieldState::Absent);
                assert_present(candidate.source_name(), "Synthetic Watch");
                assert_present(candidate.source_version(), "1.0");
                assert_present(candidate.unit(), "min");
                assert_present(
                    candidate.normalized_ref(),
                    "imports/synthetic-import/normalized/2026-01.jsonl#L6",
                );
                assert_present(
                    candidate.raw_ref(),
                    "imports/synthetic-import/raw/export.xml#workout-6",
                );
                let FieldState::Present(metadata) = candidate.metadata() else {
                    panic!("apple metadata should be present");
                };
                assert_body_value_bitwise_eq(
                    &BodyValue::Object(metadata.clone()),
                    &metadata_from_json(&row["row"]["metadata"]),
                );
                let ValueState::Present(BodyValue::String(value)) = candidate.value() else {
                    panic!("apple value should be a present string");
                };
                assert_body_string(value, "45");
            }
            "oura_v1_all_shapes" => {
                assert_eq!(candidate.schema(), LedgerSchema::OuraV1);
                assert_body_string(candidate.source_family(), "oura_api");
                assert_body_string(candidate.record_type(), "oura.daily_readiness");
                assert_body_string(
                    candidate.dedupe_key(),
                    "sha256:cf5b6fc199a3bcbc4d9361346d957f9098c356fe75f226803d2bd57580d95258",
                );
                assert_body_string(candidate.start_date(), "2026-01-02");
                assert_body_string(candidate.day(), "20260102");
                assert_present(candidate.kind(), "daily_summary");
                assert_present(candidate.import_id(), "synthetic-import");
                assert_present(candidate.month(), "2026-01");
                assert_present(candidate.end_date(), "2026-01-03");
                assert_present(candidate.source_record_id(), "synthetic-readiness-1");
                assert_eq!(candidate.source_name(), &FieldState::Absent);
                assert_eq!(candidate.source_version(), &FieldState::Absent);
                assert_present(candidate.unit(), "score");
                assert_present(
                    candidate.normalized_ref(),
                    "imports/synthetic-import/normalized/2026-01.jsonl#L1",
                );
                assert_present(
                    candidate.raw_ref(),
                    "imports/synthetic-import/raw/oura/daily_readiness-0001.json#item-0",
                );
                let FieldState::Present(metadata) = candidate.metadata() else {
                    panic!("oura metadata should be present");
                };
                assert_body_value_bitwise_eq(
                    &BodyValue::Object(metadata.clone()),
                    &metadata_from_json(&row["row"]["metadata"]),
                );
                let ValueState::Present(BodyValue::Integer(value)) = candidate.value() else {
                    panic!("oura value should be a present integer");
                };
                assert_eq!(value.digits(), "91");
                assert!(!value.is_negative());
            }
            "legacy_normalized_v1" => {
                assert_eq!(candidate.schema(), LedgerSchema::NormalizedV1);
                assert_body_string(candidate.source_family(), "apple_health");
                assert_body_string(candidate.record_type(), "HKQuantityTypeIdentifierHeartRate");
                assert_body_string(candidate.dedupe_key(), "apple-health:synthetic:legacy-1");
                assert_body_string(candidate.start_date(), "2026-01-01T08:00:00-07:00");
                assert_body_string(candidate.day(), "20260101");
                assert_present(candidate.kind(), "record");
                assert_present(candidate.import_id(), "legacy-synthetic-import");
                assert_present(candidate.month(), "2026-01");
                assert_eq!(candidate.end_date(), &FieldState::Null);
                assert_eq!(candidate.source_record_id(), &FieldState::Absent);
                assert_present(candidate.source_name(), "Synthetic Device");
                assert_eq!(candidate.source_version(), &FieldState::Absent);
                assert_present(candidate.unit(), "count/min");
                assert_present(
                    candidate.normalized_ref(),
                    "imports/legacy-synthetic-import/normalized/2026-01.jsonl#L1",
                );
                assert_eq!(candidate.raw_ref(), &FieldState::Absent);
                let FieldState::Present(metadata) = candidate.metadata() else {
                    panic!("legacy metadata should be present");
                };
                assert_body_value_bitwise_eq(
                    &BodyValue::Object(metadata.clone()),
                    &metadata_from_json(&row["row"]["metadata"]),
                );
                let ValueState::Present(BodyValue::String(value)) = candidate.value() else {
                    panic!("legacy value should be a present string");
                };
                assert_body_string(value, "72");
            }
            _ => panic!("unexpected codec row {name}"),
        }
    }
}

#[test]
fn nonobject_rows_refuse_before_schema_and_remain_recoverable() {
    for literal in [
        "null",
        "true",
        "false",
        "12345678901234567890123456789012345678901234567890",
        "3.5",
        "NaN",
        "Infinity",
        "-Infinity",
        r#""a string""#,
        "[1,2,3]",
    ] {
        let parsed = parse(literal.as_bytes()).expect("literal parses");
        let presentation = PresentationRow::from(parsed);
        let error = project(&presentation, Coordinate::new("b", "s", 1))
            .expect_err("nonobject row should refuse");
        assert_eq!(error.code, CandidateErrorCode::WrongType);
        assert_eq!(error.field, CandidateErrorField::Row);

        let reparsed = parse(literal.as_bytes()).expect("literal reparses");
        assert_body_value_bitwise_eq(presentation.value(), &reparsed);
        if !matches!(reparsed, BodyValue::Number(_)) {
            assert_eq!(
                canonicalize(presentation.value()).expect("row canonicalizes"),
                canonicalize(&reparsed).expect("reparsed value canonicalizes")
            );
        }
    }

    let empty_object = PresentationRow::from(BodyValue::Object(BTreeMap::new()));
    let error = project(&empty_object, Coordinate::new("b", "s", 1))
        .expect_err("object without schema should refuse");
    assert_eq!(error.code, CandidateErrorCode::UnsupportedSchema);
    assert_eq!(error.field, CandidateErrorField::Schema);

    let integer = BodyInteger::new(false, "1").expect("integer construction works");
    assert!(matches!(
        PresentationRow::from(BodyValue::Integer(integer)).value(),
        BodyValue::Integer(_)
    ));
}
