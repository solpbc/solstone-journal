// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use solstone_core_body_source::{
    BodyString, BodyValue, CandidateErrorCode, CandidateErrorField, Coordinate, FieldState,
    LedgerSchema, PresentationRow, ValueState, parse, project,
};

mod support;

use support::assert_body_value_bitwise_eq;

const OPTIONAL_FIELDS: [(&str, CandidateErrorField); 10] = [
    ("kind", CandidateErrorField::Kind),
    ("import_id", CandidateErrorField::ImportId),
    ("month", CandidateErrorField::Month),
    ("end_date", CandidateErrorField::EndDate),
    ("source_record_id", CandidateErrorField::SourceRecordId),
    ("source_name", CandidateErrorField::SourceName),
    ("source_version", CandidateErrorField::SourceVersion),
    ("unit", CandidateErrorField::Unit),
    ("normalized_ref", CandidateErrorField::NormalizedRef),
    ("raw_ref", CandidateErrorField::RawRef),
];

const PYTHON_WHITESPACE: [u32; 29] = [
    0x0009, 0x000a, 0x000b, 0x000c, 0x000d, 0x001c, 0x001d, 0x001e, 0x001f, 0x0020, 0x0085, 0x00a0,
    0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a,
    0x2028, 0x2029, 0x202f, 0x205f, 0x3000,
];

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

fn presentation_from_json(row: Map<String, Value>) -> PresentationRow {
    let text = serde_json::to_string(&Value::Object(row)).expect("row serializes");
    let value = parse(text.as_bytes()).expect("row parses");
    PresentationRow::new(&value, &Coordinate::new("bundle", "shard", 1)).expect("row constructs")
}

fn project_json(
    row: Map<String, Value>,
) -> Result<solstone_core_body_source::LedgerCandidate, solstone_core_body_source::CandidateError> {
    let presentation = presentation_from_json(row);
    project(&presentation, Coordinate::new("bundle", "shard", 1))
}

fn assert_error(
    result: Result<
        solstone_core_body_source::LedgerCandidate,
        solstone_core_body_source::CandidateError,
    >,
    code: CandidateErrorCode,
    field: CandidateErrorField,
) {
    let error = result.expect_err("projection should fail");
    assert_eq!(error.code, code);
    assert_eq!(error.field, field);
}

fn string_value(value: &str) -> Vec<u32> {
    value.bytes().map(u32::from).collect()
}

fn optional_state<'a>(
    candidate: &'a solstone_core_body_source::LedgerCandidate,
    name: &str,
) -> &'a FieldState<BodyString> {
    match name {
        "kind" => candidate.kind(),
        "import_id" => candidate.import_id(),
        "month" => candidate.month(),
        "end_date" => candidate.end_date(),
        "source_record_id" => candidate.source_record_id(),
        "source_name" => candidate.source_name(),
        "source_version" => candidate.source_version(),
        "unit" => candidate.unit(),
        "normalized_ref" => candidate.normalized_ref(),
        "raw_ref" => candidate.raw_ref(),
        _ => panic!("unknown optional field {name}"),
    }
}

fn body_object() -> solstone_core_body_source::BodyObject {
    let BodyValue::Object(object) = parse(
        br#"{"schema":"solstone.health.apple_health.v1","source_family":"apple_health","record_type":"rt","dedupe_key":"dk","start_date":"sd","day":"20260101"}"#,
    )
    .expect("minimal body object parses")
    else {
        unreachable!("minimal row is an object");
    };
    object
}

fn body_key(name: &str) -> BodyString {
    BodyString::from_code_points(string_value(name)).expect("ASCII key is valid")
}

#[test]
fn schema_and_family_matrix_is_closed_and_exact() {
    for (schema, family, expected) in [
        (
            "solstone.health.apple_health.v1",
            "apple_health",
            LedgerSchema::AppleHealthV1,
        ),
        ("solstone.health.oura.v1", "oura_api", LedgerSchema::OuraV1),
        (
            "solstone.health.normalized.v1",
            "apple_health",
            LedgerSchema::NormalizedV1,
        ),
    ] {
        let mut row = valid_row();
        row.insert("schema".to_owned(), json!(schema));
        row.insert("source_family".to_owned(), json!(family));
        assert_eq!(project_json(row).expect("valid pairing").schema(), expected);
    }

    for (schema, family) in [
        ("solstone.health.apple_health.v1", "oura_api"),
        ("solstone.health.apple_health.v1", "unknown_family"),
        ("solstone.health.oura.v1", "apple_health"),
        ("solstone.health.oura.v1", "unknown_family"),
        ("solstone.health.normalized.v1", "oura_api"),
        ("solstone.health.normalized.v1", "unknown_family"),
    ] {
        let mut row = valid_row();
        row.insert("schema".to_owned(), json!(schema));
        row.insert("source_family".to_owned(), json!(family));
        assert_error(
            project_json(row),
            CandidateErrorCode::IncompatibleField,
            CandidateErrorField::SourceFamily,
        );
    }

    for schema in ["solstone.health.made_up.v1", ""] {
        let mut row = valid_row();
        row.insert("schema".to_owned(), json!(schema));
        assert_error(
            project_json(row),
            CandidateErrorCode::UnsupportedSchema,
            CandidateErrorField::Schema,
        );
    }
    for value in [Value::Null, json!(42)] {
        let mut row = valid_row();
        row.insert("schema".to_owned(), value);
        assert_error(
            project_json(row),
            CandidateErrorCode::UnsupportedSchema,
            CandidateErrorField::Schema,
        );
    }
    let mut absent_schema = valid_row();
    absent_schema.remove("schema");
    assert_error(
        project_json(absent_schema),
        CandidateErrorCode::UnsupportedSchema,
        CandidateErrorField::Schema,
    );

    for schema in [
        "solstone.health.apple_health.v1",
        "solstone.health.oura.v1",
        "solstone.health.normalized.v1",
    ] {
        let prefix = schema.strip_suffix('1').expect("schema ends in one");
        let suffix = format!("x{schema}");
        let case_variant = format!("S{}", &schema[1..]);
        for near_miss in [prefix.to_owned(), suffix, case_variant] {
            let mut row = valid_row();
            row.insert("schema".to_owned(), json!(near_miss));
            assert_error(
                project_json(row),
                CandidateErrorCode::UnsupportedSchema,
                CandidateErrorField::Schema,
            );
        }
    }
}

#[test]
fn required_identity_fields_enforce_exact_python_blank_rules() {
    for (field, error_field) in [
        ("source_family", CandidateErrorField::SourceFamily),
        ("record_type", CandidateErrorField::RecordType),
        ("dedupe_key", CandidateErrorField::DedupeKey),
        ("start_date", CandidateErrorField::StartDate),
    ] {
        let mut absent = valid_row();
        absent.remove(field);
        assert_error(
            project_json(absent),
            CandidateErrorCode::MissingField,
            error_field,
        );

        let mut null = valid_row();
        null.insert(field.to_owned(), Value::Null);
        assert_error(
            project_json(null),
            CandidateErrorCode::WrongType,
            error_field,
        );

        for value in [json!(42), json!(true), json!([]), json!({})] {
            let mut row = valid_row();
            row.insert(field.to_owned(), value);
            assert_error(
                project_json(row),
                CandidateErrorCode::WrongType,
                error_field,
            );
        }

        for code_point in PYTHON_WHITESPACE {
            let whitespace = char::from_u32(code_point)
                .expect("listed code point is a scalar")
                .to_string();
            let mut row = valid_row();
            row.insert(field.to_owned(), json!(whitespace));
            assert_error(
                project_json(row),
                CandidateErrorCode::BlankField,
                error_field,
            );
        }

        let mut empty = valid_row();
        empty.insert(field.to_owned(), json!(""));
        assert_error(
            project_json(empty),
            CandidateErrorCode::BlankField,
            error_field,
        );

        for value in ["\tvalue\t", "va\tlue"] {
            let mut row = valid_row();
            let value = if field == "source_family" {
                value.replace("value", "apple_health")
            } else {
                value.to_owned()
            };
            row.insert(field.to_owned(), json!(value));
            if field == "source_family" {
                assert_error(
                    project_json(row),
                    CandidateErrorCode::IncompatibleField,
                    CandidateErrorField::SourceFamily,
                );
            } else {
                assert!(project_json(row).is_ok(), "{field} should accept {value:?}");
            }
        }

        let mut surrogate = body_object();
        surrogate.insert(
            body_key(field),
            BodyValue::String(BodyString::from_code_points(vec![0xd800]).expect("surrogate")),
        );
        let value = BodyValue::Object(surrogate);
        let presentation = PresentationRow::new(&value, &Coordinate::new("bundle", "shard", 1))
            .expect("row constructs");
        if field == "source_family" {
            assert_error(
                project(&presentation, Coordinate::new("bundle", "shard", 1)),
                CandidateErrorCode::IncompatibleField,
                CandidateErrorField::SourceFamily,
            );
        } else {
            assert!(
                project(&presentation, Coordinate::new("bundle", "shard", 1)).is_ok(),
                "{field} should accept a lone surrogate"
            );
        }
    }
}

#[test]
fn day_is_required_but_empty_is_valid() {
    let mut absent = valid_row();
    absent.remove("day");
    assert_error(
        project_json(absent),
        CandidateErrorCode::MissingField,
        CandidateErrorField::Day,
    );
    for value in [Value::Null, json!(42)] {
        let mut row = valid_row();
        row.insert("day".to_owned(), value);
        assert_error(
            project_json(row),
            CandidateErrorCode::WrongType,
            CandidateErrorField::Day,
        );
    }
    let mut empty = valid_row();
    empty.insert("day".to_owned(), json!(""));
    let candidate = project_json(empty).expect("empty day should project");
    assert_eq!(candidate.day().code_points(), Vec::<u32>::new());
}

#[test]
fn optional_string_fields_preserve_tri_state_and_reject_other_types() {
    for (field, error_field) in OPTIONAL_FIELDS {
        let candidate = project_json(valid_row()).expect("absent optional field is valid");
        assert_eq!(optional_state(&candidate, field), &FieldState::Absent);

        let mut null = valid_row();
        null.insert(field.to_owned(), Value::Null);
        let candidate = project_json(null).expect("null optional field is valid");
        assert_eq!(optional_state(&candidate, field), &FieldState::Null);

        for value in ["", " "] {
            let mut row = valid_row();
            row.insert(field.to_owned(), json!(value));
            let candidate = project_json(row).expect("optional string is valid");
            let FieldState::Present(actual) = optional_state(&candidate, field) else {
                panic!("optional string field should be present");
            };
            assert_eq!(actual.code_points(), string_value(value));
        }

        for value in [json!(true), json!(42), json!([]), json!({})] {
            let mut row = valid_row();
            row.insert(field.to_owned(), value);
            assert_error(
                project_json(row),
                CandidateErrorCode::WrongType,
                error_field,
            );
        }
    }
}

#[test]
fn metadata_preserves_tri_state_and_requires_objects() {
    let candidate = project_json(valid_row()).expect("absent metadata is valid");
    assert_eq!(candidate.metadata(), &FieldState::Absent);

    let mut null = valid_row();
    null.insert("metadata".to_owned(), Value::Null);
    let candidate = project_json(null).expect("null metadata is valid");
    assert_eq!(candidate.metadata(), &FieldState::Null);

    for metadata in [json!({}), json!({"nested": [1, {"truth": true}, null]})] {
        let mut row = valid_row();
        row.insert("metadata".to_owned(), metadata.clone());
        let candidate = project_json(row).expect("object metadata is valid");
        let FieldState::Present(actual) = candidate.metadata() else {
            panic!("metadata should be present");
        };
        let expected = parse(
            serde_json::to_string(&metadata)
                .expect("metadata serializes")
                .as_bytes(),
        )
        .expect("metadata parses");
        assert_body_value_bitwise_eq(&BodyValue::Object(actual.clone()), &expected);
    }

    for value in [json!(true), json!(42), json!("str"), json!([])] {
        let mut row = valid_row();
        row.insert("metadata".to_owned(), value);
        assert_error(
            project_json(row),
            CandidateErrorCode::WrongType,
            CandidateErrorField::Metadata,
        );
    }
}

#[test]
fn metadata_preserves_bitwise_float_variants() {
    let row_text = r#"{"schema":"solstone.health.apple_health.v1","source_family":"apple_health","record_type":"rt","dedupe_key":"dk","start_date":"sd","day":"20260101","metadata":{"neg_zero":-0.0,"pos_zero":0.0,"pos_inf":Infinity,"neg_inf":-Infinity,"nan":NaN}}"#;
    let value = parse(row_text.as_bytes()).expect("row parses");
    let presentation = PresentationRow::new(&value, &Coordinate::new("bundle", "shard", 1))
        .expect("row constructs");
    let candidate = project(&presentation, Coordinate::new("bundle", "shard", 1))
        .expect("float-bearing metadata is valid");
    let FieldState::Present(actual) = candidate.metadata() else {
        panic!("literal metadata should be present");
    };
    let BodyValue::Object(expected) = parse(
        br#"{"neg_zero":-0.0,"pos_zero":0.0,"pos_inf":Infinity,"neg_inf":-Infinity,"nan":NaN}"#,
    )
    .expect("metadata reparses") else {
        unreachable!("metadata text is an object");
    };
    assert_body_value_bitwise_eq(
        &BodyValue::Object(actual.clone()),
        &BodyValue::Object(expected),
    );

    let quiet = 0x7ff8_0000_0000_0001;
    let signaling = 0x7ff0_0000_0000_0001;
    let metadata = BTreeMap::from([
        (
            body_key("quiet_positive"),
            BodyValue::Number(f64::from_bits(quiet)),
        ),
        (
            body_key("quiet_negative"),
            BodyValue::Number(f64::from_bits(quiet | (1_u64 << 63))),
        ),
        (
            body_key("signaling_positive"),
            BodyValue::Number(f64::from_bits(signaling)),
        ),
        (
            body_key("signaling_negative"),
            BodyValue::Number(f64::from_bits(signaling | (1_u64 << 63))),
        ),
    ]);
    let expected = BodyValue::Object(metadata.clone());
    let mut row = body_object();
    row.insert(body_key("metadata"), BodyValue::Object(metadata));
    let value = BodyValue::Object(row);
    let presentation = PresentationRow::new(&value, &Coordinate::new("bundle", "shard", 1))
        .expect("row constructs");
    let candidate = project(&presentation, Coordinate::new("bundle", "shard", 1))
        .expect("bit-constructed NaN metadata is valid");
    let FieldState::Present(actual) = candidate.metadata() else {
        panic!("bit-constructed metadata should be present");
    };
    assert_body_value_bitwise_eq(&BodyValue::Object(actual.clone()), &expected);
}

#[test]
fn value_preserves_every_f1_value_variant() {
    for literal in [
        "null",
        "true",
        r#""str""#,
        "12345678901234567890123456789012345678901234567890",
        "3.5",
        "NaN",
        "Infinity",
        "-Infinity",
        "[1,2,3]",
        r#"{"nested":[true,null,{"value":-0.0}]}"#,
    ] {
        let value = parse(literal.as_bytes()).expect("value literal parses");
        let mut object = body_object();
        object.insert(body_key("value"), value.clone());
        let row = BodyValue::Object(object);
        let presentation = PresentationRow::new(&row, &Coordinate::new("bundle", "shard", 1))
            .expect("row constructs");
        let candidate = project(&presentation, Coordinate::new("bundle", "shard", 1))
            .expect("every value type is valid");
        let ValueState::Present(actual) = candidate.value() else {
            panic!("present value should remain present");
        };
        assert_body_value_bitwise_eq(actual, &value);
    }

    let candidate = project_json(valid_row()).expect("absent value is valid");
    assert_eq!(candidate.value(), &ValueState::Absent);
}
