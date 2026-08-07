// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyString, BodyValue, CandidateError, CandidateErrorCode, CandidateErrorField, Coordinate,
    FieldState, LedgerCandidate, LedgerSchema, PresentationRow, ValueState, canonicalize, parse,
    project,
};

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect())
        .expect("ASCII string is a valid body string")
}

fn assert_present(value: &FieldState<BodyString>, expected: &str) {
    assert_eq!(value, &FieldState::Present(body_string(expected)));
}

fn assert_candidate(candidate: &LedgerCandidate) {
    assert_eq!(candidate.schema(), LedgerSchema::AppleHealthV1);
    assert_eq!(candidate.source_family(), &body_string("apple_health"));
    assert_eq!(
        candidate.record_type(),
        &body_string("HKQuantityTypeIdentifierStepCount")
    );
    assert_eq!(candidate.dedupe_key(), &body_string("sha256:demo"));
    assert_eq!(candidate.start_date(), &body_string("2026-01-02T03:04:05Z"));
    assert_eq!(candidate.day(), &body_string("20260102"));
    assert_present(candidate.kind(), "record");
    assert_eq!(candidate.import_id(), &FieldState::Null);
    assert_present(candidate.month(), "2026-01");
    assert_eq!(candidate.end_date(), &FieldState::Null);
    assert_present(candidate.source_record_id(), "source-1");
    assert_present(candidate.source_name(), "Demo Device");
    assert_present(candidate.source_version(), "1.0");
    assert_present(candidate.unit(), "count");
    assert_present(
        candidate.normalized_ref(),
        "imports/demo/normalized/2026-01.jsonl#L1",
    );
    assert_present(candidate.raw_ref(), "imports/demo/raw/source.json#L1");

    let FieldState::Present(metadata) = candidate.metadata() else {
        panic!("metadata should be present");
    };
    assert_eq!(
        metadata.get(&body_string("nested")),
        Some(&BodyValue::Array(vec![
            BodyValue::Bool(true),
            BodyValue::Null
        ]))
    );
    assert_eq!(
        candidate.value(),
        &ValueState::Present(BodyValue::Integer(
            solstone_core_body_source::BodyInteger::new(false, "42").expect("integer")
        ))
    );
}

fn assert_error(error: &CandidateError) {
    assert_eq!(error.coordinate.bundle(), "bundle-1");
    assert_eq!(error.coordinate.shard(), "shard-1");
    assert_eq!(error.coordinate.line(), Some(7));
    assert_eq!(error.code, CandidateErrorCode::MissingField);
    assert_eq!(error.field, CandidateErrorField::SourceFamily);
}

#[test]
fn public_api_projects_and_recovers_valid_and_refused_rows() {
    let valid_text = r#"{"schema":"solstone.health.apple_health.v1","source_family":"apple_health","record_type":"HKQuantityTypeIdentifierStepCount","dedupe_key":"sha256:demo","start_date":"2026-01-02T03:04:05Z","day":"20260102","kind":"record","import_id":null,"month":"2026-01","end_date":null,"source_record_id":"source-1","source_name":"Demo Device","source_version":"1.0","unit":"count","normalized_ref":"imports/demo/normalized/2026-01.jsonl#L1","raw_ref":"imports/demo/raw/source.json#L1","metadata":{"nested":[true,null]},"value":42}"#;
    let valid_value = parse(valid_text.as_bytes()).expect("valid row parses");
    let valid_row = PresentationRow::new(&valid_value, &Coordinate::new("bundle-1", "shard-1", 7))
        .expect("valid row constructs");
    let valid_before = canonicalize(valid_row.value()).expect("valid row canonicalizes");
    let candidate =
        project(&valid_row, Coordinate::new("bundle-1", "shard-1", 7)).expect("valid row projects");
    assert_candidate(&candidate);
    assert_eq!(
        canonicalize(valid_row.value()).expect("valid row canonicalizes"),
        valid_before
    );

    let invalid_text = r#"{"schema":"solstone.health.apple_health.v1","record_type":"rt","dedupe_key":"dk","start_date":"sd","day":"20260101"}"#;
    let invalid_value = parse(invalid_text.as_bytes()).expect("invalid row parses");
    let invalid_row =
        PresentationRow::new(&invalid_value, &Coordinate::new("bundle-1", "shard-1", 7))
            .expect("invalid row constructs");
    let invalid_before = canonicalize(invalid_row.value()).expect("invalid row canonicalizes");
    let error = project(&invalid_row, Coordinate::new("bundle-1", "shard-1", 7))
        .expect_err("missing family must refuse");
    assert_error(&error);
    assert_eq!(
        canonicalize(invalid_row.value()).expect("invalid row canonicalizes"),
        invalid_before
    );
}
