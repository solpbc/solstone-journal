// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_body_source::{
    CandidateErrorCode, CandidateErrorField, Coordinate, PresentationRow, canonicalize, parse,
    project,
};

fn valid_row() -> Map<String, Value> {
    let Value::Object(row) = json!({
        "schema": "solstone.health.apple_health.v1",
        "source_family": "apple_health",
        "record_type": "rt",
        "dedupe_key": "dk",
        "start_date": "sd",
        "day": "20260101"
    }) else {
        unreachable!("JSON object literal must be an object");
    };
    row
}

fn presentation_from_json(row: Map<String, Value>) -> (PresentationRow, String) {
    let text = serde_json::to_string(&Value::Object(row)).expect("row serializes");
    let value = parse(text.as_bytes()).expect("row parses");
    let presentation = PresentationRow::new(&value, &Coordinate::new("bundle", "shard", 1))
        .expect("row constructs");
    (presentation, text)
}

fn assert_failure_preserves_row(
    row: Map<String, Value>,
    code: CandidateErrorCode,
    field: CandidateErrorField,
) {
    let (presentation, text) = presentation_from_json(row);
    let error = project(&presentation, Coordinate::new("bundle", "shard", 1))
        .expect_err("projection should fail");
    assert_eq!(error.code, code);
    assert_eq!(error.field, field);
    let reparsed = parse(text.as_bytes()).expect("source text reparses");
    assert_eq!(
        canonicalize(presentation.value()).expect("presentation canonicalizes"),
        canonicalize(&reparsed).expect("reparsed value canonicalizes")
    );
}

#[test]
fn malformed_json_fails_before_presentation_exists() {
    for input in [br#"{"schema":"x""#.as_slice(), br#"{"#.as_slice()] {
        assert!(parse(input).is_err(), "malformed JSON should fail parsing");
    }
}

#[test]
fn nonobject_rows_fail_before_schema_validation() {
    for input in [br#"[]"#.as_slice(), br#""not an object""#.as_slice()] {
        let value = parse(input).expect("input parses");
        let error = PresentationRow::new(&value, &Coordinate::new("bundle", "shard", 1))
            .expect_err("nonobject should fail");
        assert_eq!(error.code, CandidateErrorCode::WrongType);
        assert_eq!(error.field, CandidateErrorField::Row);
    }
}

#[test]
fn first_failure_follows_the_projection_precedence_chain() {
    let mut unknown_schema = valid_row();
    unknown_schema.insert("schema".to_owned(), json!("unknown"));
    unknown_schema.remove("source_family");
    unknown_schema.remove("record_type");
    assert_failure_preserves_row(
        unknown_schema,
        CandidateErrorCode::UnsupportedSchema,
        CandidateErrorField::Schema,
    );

    let mut missing_family = valid_row();
    missing_family.remove("source_family");
    missing_family.insert("record_type".to_owned(), json!(42));
    assert_failure_preserves_row(
        missing_family,
        CandidateErrorCode::MissingField,
        CandidateErrorField::SourceFamily,
    );

    let mut incompatible_family = valid_row();
    incompatible_family.insert("source_family".to_owned(), json!("oura_api"));
    incompatible_family.remove("record_type");
    assert_failure_preserves_row(
        incompatible_family,
        CandidateErrorCode::IncompatibleField,
        CandidateErrorField::SourceFamily,
    );

    let mut missing_day = valid_row();
    missing_day.remove("day");
    missing_day.insert("kind".to_owned(), json!([]));
    assert_failure_preserves_row(
        missing_day,
        CandidateErrorCode::MissingField,
        CandidateErrorField::Day,
    );

    let mut kind_before_import_id = valid_row();
    kind_before_import_id.insert("kind".to_owned(), json!(42));
    kind_before_import_id.insert("import_id".to_owned(), json!(true));
    assert_failure_preserves_row(
        kind_before_import_id,
        CandidateErrorCode::WrongType,
        CandidateErrorField::Kind,
    );

    let mut metadata_last = valid_row();
    metadata_last.insert("metadata".to_owned(), json!("not an object"));
    assert_failure_preserves_row(
        metadata_last,
        CandidateErrorCode::WrongType,
        CandidateErrorField::Metadata,
    );
}
