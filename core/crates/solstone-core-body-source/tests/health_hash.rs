// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::error::Error;

use solstone_core_body_source::{
    BodyHashError, BodyObject, BodyString, BodyValue, Coordinate, FieldState, HealthRecordIdentity,
    IdentityField, PresentationRow, ValueState, health_record_dedupe_key, health_value_hash, parse,
    project,
};

mod support;

const PYTHON_WHITESPACE: [u32; 29] = [
    0x0009, 0x000a, 0x000b, 0x000c, 0x000d, 0x001c, 0x001d, 0x001e, 0x001f, 0x0020, 0x0085, 0x00a0,
    0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a,
    0x2028, 0x2029, 0x202f, 0x205f, 0x3000,
];

fn body_string(value: &str) -> BodyString {
    BodyString::from_code_points(value.bytes().map(u32::from).collect())
        .expect("ASCII text is a body string")
}

fn body_string_points(code_points: Vec<u32>) -> BodyString {
    BodyString::from_code_points(code_points).expect("valid code points are a body string")
}

fn ascii(value: &BodyString) -> String {
    String::from_utf8(
        value
            .code_points()
            .iter()
            .map(|code_point| u8::try_from(*code_point).expect("fixture text is ASCII"))
            .collect(),
    )
    .expect("ASCII bytes are UTF-8")
}

fn field<'a>(object: &'a BodyObject, name: &str) -> &'a BodyValue {
    object
        .get(&body_string(name))
        .unwrap_or_else(|| panic!("missing fixture field {name}"))
}

fn object(value: &BodyValue) -> &BodyObject {
    let BodyValue::Object(object) = value else {
        panic!("fixture value must be an object");
    };
    object
}

fn array(value: &BodyValue) -> &[BodyValue] {
    let BodyValue::Array(array) = value else {
        panic!("fixture value must be an array");
    };
    array
}

fn string(value: &BodyValue) -> &BodyString {
    let BodyValue::String(value) = value else {
        panic!("fixture value must be a string");
    };
    value
}

fn required_string(object: &BodyObject, name: &str) -> BodyString {
    string(field(object, name)).clone()
}

fn optional_string(object: &BodyObject, name: &str) -> FieldState<BodyString> {
    match field(object, name) {
        BodyValue::Null => FieldState::Null,
        BodyValue::String(value) => FieldState::Present(value.clone()),
        _ => panic!("fixture field {name} must be a string or null"),
    }
}

fn optional_object(object: &BodyObject, name: &str) -> FieldState<BodyObject> {
    match field(object, name) {
        BodyValue::Null => FieldState::Null,
        BodyValue::Object(value) => FieldState::Present(value.clone()),
        _ => panic!("fixture field {name} must be an object or null"),
    }
}

fn identity_from_fixture(value: &BodyValue) -> HealthRecordIdentity {
    let object = object(value);
    HealthRecordIdentity {
        source_family: required_string(object, "source_family"),
        record_type: required_string(object, "record_type"),
        start_time: required_string(object, "start_time"),
        end_time: optional_string(object, "end_time"),
        source_record_id: optional_string(object, "source_record_id"),
        source_name: optional_string(object, "source_name"),
        unit: optional_string(object, "unit"),
        metadata: optional_object(object, "metadata"),
        value: ValueState::Present(field(object, "value").clone()),
    }
}

fn value_input(value: &BodyValue) -> (FieldState<BodyString>, FieldState<BodyObject>, ValueState) {
    let object = object(value);
    (
        optional_string(object, "unit"),
        optional_object(object, "metadata"),
        ValueState::Present(field(object, "value").clone()),
    )
}

fn fixture_root(path: std::path::PathBuf) -> BodyValue {
    parse(&std::fs::read(path).expect("fixture should read")).expect("fixture should parse via F1")
}

fn fixture_case_name(case: &BodyObject) -> String {
    ascii(string(field(case, "name")))
}

fn expected(case: &BodyObject, name: &str) -> String {
    ascii(string(field(case, name)))
}

fn base_identity() -> HealthRecordIdentity {
    HealthRecordIdentity {
        source_family: body_string("apple_health"),
        record_type: body_string("record_type"),
        start_time: body_string("2026-01-02"),
        end_time: FieldState::Absent,
        source_record_id: FieldState::Absent,
        source_name: FieldState::Absent,
        unit: FieldState::Absent,
        metadata: FieldState::Absent,
        value: ValueState::Absent,
    }
}

fn nested_arrays(depth: usize) -> BodyValue {
    (0..depth).fold(BodyValue::Null, |value, _| BodyValue::Array(vec![value]))
}

#[test]
fn hash_vectors_match_python_oracles() {
    let root = fixture_root(support::hash_vectors_path());
    let root = object(&root);

    for case in array(field(root, "value_cases")) {
        let case = object(case);
        let (unit, metadata, value) = value_input(field(case, "input"));
        assert_eq!(
            health_value_hash(&unit, &metadata, &value).unwrap(),
            expected(case, "expected_value_hash"),
            "{}",
            fixture_case_name(case)
        );
    }

    for case in array(field(root, "dedupe_cases")) {
        let case = object(case);
        let identity = identity_from_fixture(field(case, "identity"));
        assert_eq!(
            health_value_hash(&identity.unit, &identity.metadata, &identity.value).unwrap(),
            expected(case, "expected_value_hash"),
            "{} value hash",
            fixture_case_name(case)
        );
        assert_eq!(
            health_record_dedupe_key(&identity).unwrap(),
            expected(case, "expected_dedupe_key"),
            "{} dedupe key",
            fixture_case_name(case)
        );
    }

    for case in array(field(root, "python_nonfinite_value_cases")) {
        let case = object(case);
        let value = ValueState::Present(
            parse(ascii(string(field(case, "input_value_literal"))).as_bytes())
                .expect("nonfinite literal parses"),
        );
        assert_eq!(
            health_value_hash(&FieldState::Absent, &FieldState::Absent, &value).unwrap(),
            expected(case, "expected_value_hash"),
            "{}",
            fixture_case_name(case)
        );
    }

    for case in array(field(root, "python_overflow_value_cases")) {
        let case = object(case);
        let value = ValueState::Present(
            parse(ascii(string(field(case, "input_numeric_literal"))).as_bytes())
                .expect("overflow literal parses"),
        );
        assert_eq!(
            health_value_hash(&FieldState::Absent, &FieldState::Absent, &value).unwrap(),
            expected(case, "expected_value_hash"),
            "{}",
            fixture_case_name(case)
        );
    }

    for case in array(field(root, "python_large_integer_value_cases")) {
        let case = object(case);
        let pattern = object(field(case, "decimal_pattern"));
        let BodyValue::Integer(trailing_zeros) = field(pattern, "trailing_zeros") else {
            panic!("trailing zeros must be an integer");
        };
        let count = trailing_zeros.digits().parse::<usize>().unwrap();
        let literal = format!(
            "{}{}",
            ascii(string(field(pattern, "leading"))),
            "0".repeat(count)
        );
        let value = ValueState::Present(parse(literal.as_bytes()).expect("large integer parses"));
        assert_eq!(
            health_value_hash(&FieldState::Absent, &FieldState::Absent, &value).unwrap(),
            expected(case, "expected_value_hash"),
            "{}",
            fixture_case_name(case)
        );
    }
}

#[test]
fn python_whitespace_trims_identity_edges_and_preserves_interior_content() {
    let composite = health_record_dedupe_key(&base_identity()).unwrap();
    let mut source_identity = base_identity();
    source_identity.source_record_id = FieldState::Present(body_string("source-id"));
    let source_id = health_record_dedupe_key(&source_identity).unwrap();

    for code_point in PYTHON_WHITESPACE {
        let whitespace = body_string_points(vec![code_point]);
        let edge = |value: &str| {
            body_string_points(
                whitespace
                    .code_points()
                    .iter()
                    .copied()
                    .chain(value.bytes().map(u32::from))
                    .chain(whitespace.code_points().iter().copied())
                    .collect(),
            )
        };

        let mut family = base_identity();
        family.source_family = edge("APPLE_HEALTH");
        assert_eq!(health_record_dedupe_key(&family).unwrap(), composite);

        let mut record_type = base_identity();
        record_type.record_type = edge("record_type");
        assert_eq!(health_record_dedupe_key(&record_type).unwrap(), composite);

        let mut start_time = base_identity();
        start_time.start_time = edge("2026-01-02");
        assert_eq!(health_record_dedupe_key(&start_time).unwrap(), composite);

        let mut source_record_id = clone_identity(&source_identity);
        source_record_id.source_record_id = FieldState::Present(edge("source-id"));
        assert_eq!(
            health_record_dedupe_key(&source_record_id).unwrap(),
            source_id
        );

        let mut source_start_time = clone_identity(&source_identity);
        source_start_time.start_time = edge("2026-01-02");
        assert_eq!(
            health_record_dedupe_key(&source_start_time).unwrap(),
            source_id
        );

        let mut blank_source_record_id = clone_identity(&source_identity);
        blank_source_record_id.source_record_id = FieldState::Present(whitespace.clone());
        assert_eq!(
            health_record_dedupe_key(&blank_source_record_id).unwrap(),
            composite
        );

        let mut interior = base_identity();
        interior.record_type =
            body_string_points(vec![u32::from(b'r'), code_point, u32::from(b't')]);
        assert_ne!(health_record_dedupe_key(&interior).unwrap(), composite);
    }

    let mut non_ascii = base_identity();
    non_ascii.source_family = body_string_points(vec![0x00e9]);
    assert_eq!(
        health_record_dedupe_key(&non_ascii),
        Err(BodyHashError::InvalidIdentity(IdentityField::SourceFamily))
    );
}

fn clone_identity(identity: &HealthRecordIdentity) -> HealthRecordIdentity {
    HealthRecordIdentity {
        source_family: identity.source_family.clone(),
        record_type: identity.record_type.clone(),
        start_time: identity.start_time.clone(),
        end_time: identity.end_time.clone(),
        source_record_id: identity.source_record_id.clone(),
        source_name: identity.source_name.clone(),
        unit: identity.unit.clone(),
        metadata: identity.metadata.clone(),
        value: identity.value.clone(),
    }
}

#[test]
fn invalid_identity_precedence_is_bounded_and_redacting() {
    for source_id in [
        FieldState::Absent,
        FieldState::Present(body_string("source-id")),
    ] {
        for (field, expected_error) in [
            ("source_family", IdentityField::SourceFamily),
            ("record_type", IdentityField::RecordType),
            ("start_time", IdentityField::StartTime),
        ] {
            let mut identity = base_identity();
            identity.source_record_id = source_id.clone();
            let whitespace = body_string_points(vec![0x001c; 1_048_576]);
            match field {
                "source_family" => {
                    identity.source_family = whitespace;
                    identity.record_type =
                        body_string(&("sentinel-type-".to_owned() + &"x".repeat(1_048_576)));
                }
                "record_type" => {
                    identity.record_type = whitespace;
                    identity.start_time =
                        body_string(&("sentinel-start-".to_owned() + &"x".repeat(1_048_576)));
                }
                "start_time" => identity.start_time = whitespace,
                _ => unreachable!(),
            }
            let error = health_record_dedupe_key(&identity).expect_err("blank field must fail");
            assert_eq!(error, BodyHashError::InvalidIdentity(expected_error));
            let display = error.to_string();
            let debug = format!("{error:?}");
            assert!(display.len() <= 64 && debug.len() <= 64);
            assert_eq!(display, debug);
            assert!(Error::source(&error).is_none());
            assert!(!display.contains("sentinel") && !debug.contains("sentinel"));
        }
    }

    let mut multi_fault = base_identity();
    multi_fault.source_family = body_string("");
    multi_fault.record_type = body_string("");
    multi_fault.start_time = body_string("");
    assert_eq!(
        health_record_dedupe_key(&multi_fault),
        Err(BodyHashError::InvalidIdentity(IdentityField::SourceFamily))
    );
    multi_fault.source_family = body_string("apple_health");
    assert_eq!(
        health_record_dedupe_key(&multi_fault),
        Err(BodyHashError::InvalidIdentity(IdentityField::RecordType))
    );
    multi_fault.record_type = body_string("record_type");
    assert_eq!(
        health_record_dedupe_key(&multi_fault),
        Err(BodyHashError::InvalidIdentity(IdentityField::StartTime))
    );
}

#[test]
fn truthiness_and_source_id_branch_match_python() {
    let base = base_identity();
    let composite = health_record_dedupe_key(&base).unwrap();
    for state in [
        FieldState::Absent,
        FieldState::Null,
        FieldState::Present(body_string("")),
    ] {
        let mut identity = clone_identity(&base);
        identity.end_time = state;
        assert_eq!(health_record_dedupe_key(&identity).unwrap(), composite);
    }
    let mut explicit_end = clone_identity(&base);
    explicit_end.end_time = FieldState::Present(body_string(" "));
    assert_ne!(health_record_dedupe_key(&explicit_end).unwrap(), composite);

    for state in [
        FieldState::Absent,
        FieldState::Null,
        FieldState::Present(body_string("")),
    ] {
        let mut identity = clone_identity(&base);
        identity.source_name = state;
        assert_eq!(health_record_dedupe_key(&identity).unwrap(), composite);
    }
    let mut whitespace_name = clone_identity(&base);
    whitespace_name.source_name = FieldState::Present(body_string(" "));
    assert_ne!(
        health_record_dedupe_key(&whitespace_name).unwrap(),
        composite
    );

    let absent_value = health_value_hash(
        &FieldState::Absent,
        &FieldState::Absent,
        &ValueState::Absent,
    )
    .unwrap();
    let null_value = health_value_hash(
        &FieldState::Null,
        &FieldState::Null,
        &ValueState::Present(BodyValue::Null),
    )
    .unwrap();
    assert_eq!(absent_value, null_value);
    for unit in [
        FieldState::Absent,
        FieldState::Null,
        FieldState::Present(body_string("")),
    ] {
        assert_eq!(
            health_value_hash(&unit, &FieldState::Absent, &ValueState::Absent).unwrap(),
            absent_value
        );
    }
    for metadata in [
        FieldState::Absent,
        FieldState::Null,
        FieldState::Present(BTreeMap::new()),
    ] {
        assert_eq!(
            health_value_hash(&FieldState::Absent, &metadata, &ValueState::Absent).unwrap(),
            absent_value
        );
    }
    assert_ne!(
        health_value_hash(
            &FieldState::Present(body_string(" ")),
            &FieldState::Present(BTreeMap::new()),
            &ValueState::Present(BodyValue::Bool(false)),
        )
        .unwrap(),
        absent_value
    );
    assert_ne!(
        health_value_hash(
            &FieldState::Absent,
            &FieldState::Absent,
            &ValueState::Present(BodyValue::Bool(false)),
        )
        .unwrap(),
        absent_value
    );
    assert_ne!(
        health_value_hash(
            &FieldState::Absent,
            &FieldState::Absent,
            &ValueState::Present(parse(b"0").unwrap()),
        )
        .unwrap(),
        absent_value
    );

    for state in [
        FieldState::Absent,
        FieldState::Null,
        FieldState::Present(body_string("")),
    ] {
        let mut identity = clone_identity(&base);
        identity.source_record_id = state;
        assert_eq!(health_record_dedupe_key(&identity).unwrap(), composite);
    }

    let mut source_id = clone_identity(&base);
    source_id.source_record_id = FieldState::Present(body_string("source-id"));
    let expected = health_record_dedupe_key(&source_id).unwrap();
    source_id.unit = FieldState::Present(body_string("changed"));
    source_id.end_time = FieldState::Present(body_string("changed"));
    source_id.source_name = FieldState::Present(body_string("changed"));
    source_id.metadata = FieldState::Present(BTreeMap::from([(
        body_string("changed"),
        BodyValue::Array(vec![BodyValue::Bool(false)]),
    )]));
    source_id.value = ValueState::Present(BodyValue::Bool(false));
    assert_eq!(health_record_dedupe_key(&source_id).unwrap(), expected);
    assert_ne!(
        health_value_hash(
            &FieldState::Absent,
            &FieldState::Absent,
            &ValueState::Absent
        )
        .unwrap(),
        health_value_hash(&source_id.unit, &source_id.metadata, &source_id.value).unwrap()
    );
}

#[test]
fn codec_identity_metadata_matches_the_pinned_hash() {
    let root = fixture_root(support::codec_fixture_path());
    let root = object(&root);
    let row = array(field(root, "rows"))
        .iter()
        .map(object)
        .find(|row| ascii(string(field(row, "name"))) == "apple_v1_all_shapes")
        .expect("apple codec row");
    let value = field(row, "row").clone();
    let presentation = PresentationRow::new(&value, &Coordinate::new("bundle", "shard", 1))
        .expect("codec row presents");
    let candidate =
        project(&presentation, Coordinate::new("bundle", "shard", 1)).expect("codec row projects");
    let identity = HealthRecordIdentity {
        source_family: candidate.source_family().clone(),
        record_type: candidate.record_type().clone(),
        start_time: candidate.start_date().clone(),
        end_time: candidate.end_date().clone(),
        source_record_id: candidate.source_record_id().clone(),
        source_name: candidate.source_name().clone(),
        unit: candidate.unit().clone(),
        metadata: FieldState::Present(object(field(row, "identity_metadata")).clone()),
        value: candidate.value().clone(),
    };
    assert_eq!(
        health_value_hash(&identity.unit, &identity.metadata, &identity.value).unwrap(),
        expected(row, "expected_identity_value_hash")
    );
    assert_eq!(
        health_record_dedupe_key(&identity).unwrap(),
        ascii(candidate.dedupe_key())
    );

    let enriched = HealthRecordIdentity {
        metadata: candidate.metadata().clone(),
        ..identity
    };
    assert_ne!(
        health_value_hash(&enriched.unit, &enriched.metadata, &enriched.value).unwrap(),
        expected(row, "expected_identity_value_hash")
    );
    assert_ne!(
        health_record_dedupe_key(&enriched).unwrap(),
        ascii(candidate.dedupe_key())
    );
}

#[test]
fn depth_translation_preserves_row_budget_and_source_id_bypasses_payload() {
    let nested = "[".repeat(127) + "null" + &"]".repeat(127);
    let row = format!(
        r#"{{"schema":"solstone.health.apple_health.v1","source_family":"apple_health","record_type":"rt","dedupe_key":"dk","start_date":"sd","day":"20260101","value":{nested}}}"#
    );
    let parsed = parse(row.as_bytes()).expect("128-level row parses");
    let presentation = PresentationRow::new(&parsed, &Coordinate::new("bundle", "shard", 1))
        .expect("row presents");
    let candidate =
        project(&presentation, Coordinate::new("bundle", "shard", 1)).expect("row projects");
    assert!(health_value_hash(candidate.unit(), candidate.metadata(), candidate.value()).is_ok());

    let shallow = ValueState::Present(nested_arrays(127));
    let shallow_before = shallow.clone();
    assert!(health_value_hash(&FieldState::Absent, &FieldState::Absent, &shallow).is_ok());
    assert_eq!(shallow, shallow_before);
    let deep = ValueState::Present(nested_arrays(128));
    let error = health_value_hash(&FieldState::Absent, &FieldState::Absent, &deep)
        .expect_err("combined depth 129 must fail");
    assert_eq!(error, BodyHashError::ValueTooDeep);
    assert_eq!(error.to_string(), "body-hash value_too_deep");
    assert_eq!(format!("{error:?}"), error.to_string());

    let mut invalid = base_identity();
    invalid.start_time = body_string("");
    invalid.value = deep.clone();
    assert_eq!(
        health_record_dedupe_key(&invalid),
        Err(BodyHashError::InvalidIdentity(IdentityField::StartTime))
    );

    let mut source_id_deep = base_identity();
    source_id_deep.source_record_id = FieldState::Present(body_string("source-id"));
    source_id_deep.value = deep;
    source_id_deep.metadata =
        FieldState::Present(BTreeMap::from([(body_string("deep"), nested_arrays(128))]));
    let mut source_id_shallow = base_identity();
    source_id_shallow.source_record_id = FieldState::Present(body_string("source-id"));
    assert_eq!(
        health_record_dedupe_key(&source_id_deep).unwrap(),
        health_record_dedupe_key(&source_id_shallow).unwrap()
    );
}

#[test]
fn hashes_are_repeatable_and_failures_are_typed() {
    let identity = base_identity();
    let first = health_record_dedupe_key(&identity).unwrap();
    let second = health_record_dedupe_key(&identity).unwrap();
    assert_eq!(first, second);

    let mut invalid = base_identity();
    invalid.source_family = body_string("");
    let first = health_record_dedupe_key(&invalid).expect_err("invalid identity fails");
    let second = health_record_dedupe_key(&invalid).expect_err("invalid identity fails again");
    assert_eq!(first, second);
}
