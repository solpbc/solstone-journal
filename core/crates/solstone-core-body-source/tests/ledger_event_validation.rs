// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error as _;

use serde_json::{Map, Value, json};
use solstone_core_body_source::{
    BodyDigest, BodyLedgerEvent, BodyString, BodyValue, CandidateError, Coordinate,
    LedgerCandidate, LedgerEventErrorCode, LedgerEventErrorField, PresentationRow,
    decode_body_envelope, parse, project,
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

fn project_body_string(
    row: Map<String, Value>,
    field: &str,
    code_points: Vec<u32>,
) -> LedgerCandidate {
    let encoded = serde_json::to_string(&Value::Object(row)).unwrap();
    let BodyValue::Object(mut object) = parse(encoded.as_bytes()).unwrap() else {
        unreachable!("fixture row is object")
    };
    let key = BodyString::from_code_points(field.bytes().map(u32::from).collect()).unwrap();
    object.insert(
        key,
        BodyValue::String(BodyString::from_code_points(code_points).unwrap()),
    );
    let value = BodyValue::Object(object);
    let coordinate = Coordinate::new("bundle", "shard", 1);
    let presentation = PresentationRow::new(&value, &coordinate).unwrap();
    project(&presentation, coordinate).unwrap()
}

fn ascii_points(value: &str) -> Vec<u32> {
    value.bytes().map(u32::from).collect()
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
    for family_index in 0..2 {
        let (envelope, valid) = native_context(family_index);
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

        if family_index == 0 {
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
        }

        let mut cross_family = valid.clone();
        let (schema, source_family) = if family_index == 0 {
            ("solstone.health.oura.v1", "oura_api")
        } else {
            ("solstone.health.apple_health.v1", "apple_health")
        };
        cross_family.insert("schema".into(), json!(schema));
        cross_family.insert("source_family".into(), json!(source_family));
        let candidate = project_row(cross_family).unwrap();
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

#[test]
fn exact_reference_fields_reject_every_lossy_or_normalizing_twin_for_both_families() {
    for family_index in 0..2 {
        let (envelope, row) = native_context(family_index);
        for (field, error_field) in [
            ("import_id", LedgerEventErrorField::BundleId),
            ("month", LedgerEventErrorField::Shard),
            ("normalized_ref", LedgerEventErrorField::NormalizedRef),
        ] {
            let expected = row[field].as_str().unwrap();
            let base = ascii_points(expected);
            let mut variants = Vec::new();
            let mut prefixed = vec![u32::from(b'x')];
            prefixed.extend(&base);
            variants.push(prefixed);
            let mut suffixed = base.clone();
            suffixed.push(u32::from(b'x'));
            variants.push(suffixed);
            let mut spaced = vec![u32::from(b' ')];
            spaced.extend(&base);
            variants.push(spaced);
            let mut non_ascii = base.clone();
            non_ascii.insert(1, 0x0100);
            variants.push(non_ascii);
            let mut surrogate = base.clone();
            surrogate.insert(1, 0xd800);
            variants.push(surrogate);
            if let Some(position) = base
                .iter()
                .position(|point| (*point as u8).is_ascii_lowercase())
            {
                let mut case_changed = base.clone();
                case_changed[position] -= 32;
                variants.push(case_changed);
            }

            for code_points in variants {
                let candidate = project_body_string(row.clone(), field, code_points);
                assert_error(
                    bind(&envelope, 1, 0, 1, &candidate),
                    LedgerEventErrorCode::ReferenceMismatch,
                    error_field,
                    &envelope,
                    1,
                );
            }
        }

        for replacement in [None, Some(Value::Null)] {
            let mut row = row.clone();
            match replacement {
                None => {
                    row.remove("normalized_ref");
                }
                Some(value) => {
                    row.insert("normalized_ref".into(), value);
                }
            }
            let candidate = project_row(row).unwrap();
            assert_error(
                bind(&envelope, 1, 0, 1, &candidate),
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::NormalizedRef,
                &envelope,
                1,
            );
        }

        let mut malformed = row.clone();
        malformed.insert("dedupe_key".into(), json!("sha256:abc"));
        let candidate = project_row(malformed).unwrap();
        assert_error(
            bind(&envelope, 1, 0, 1, &candidate),
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::DedupeKey,
            &envelope,
            1,
        );
    }
}

#[test]
fn multishard_shard_and_line_boundaries_are_checked_at_first_and_last_positions() {
    let fixture = ledger_events_fixture();
    let case = &fixture["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()).unwrap();
    let january_rows: Vec<&str> = case["shards"][0]["expected_jsonl"]
        .as_str()
        .unwrap()
        .lines()
        .collect();
    let february_row = case["shards"][1]["expected_jsonl"]
        .as_str()
        .unwrap()
        .lines()
        .next()
        .unwrap();
    let Value::Object(january_second) = serde_json::from_str(january_rows[1]).unwrap() else {
        unreachable!()
    };
    let Value::Object(february) = serde_json::from_str(february_row).unwrap() else {
        unreachable!()
    };
    let january_second = project_row(january_second).unwrap();
    let february = project_row(february).unwrap();

    assert!(bind(&envelope, 2, 0, 2, &january_second).is_ok());
    assert!(bind(&envelope, 3, 1, 1, &february).is_ok());
    assert_error(
        bind(&envelope, 2, 0, 3, &january_second),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Line,
        &envelope,
        2,
    );
    assert_error(
        bind(&envelope, 3, 2, 1, &february),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
        &envelope,
        3,
    );
    assert_error(
        bind(&envelope, u64::MAX, 1, 1, &february),
        LedgerEventErrorCode::InvalidSequence,
        LedgerEventErrorField::Sequence,
        &envelope,
        u64::MAX,
    );
}

#[test]
fn adjacent_constructor_faults_follow_the_declared_precedence() {
    for family_index in 0..2 {
        let (envelope, valid) = native_context(family_index);
        let candidate = project_row(valid.clone()).unwrap();
        assert_error(
            bind(&envelope, 0, u64::MAX, 0, &candidate),
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

        let mut row = valid.clone();
        if family_index == 0 {
            row.insert("schema".into(), json!("solstone.health.normalized.v1"));
        } else {
            row.insert("schema".into(), json!("solstone.health.apple_health.v1"));
            row.insert("source_family".into(), json!("apple_health"));
        }
        row.insert("import_id".into(), Value::Null);
        let schema_and_import = project_row(row).unwrap();
        assert_error(
            bind(&envelope, 1, 0, 0, &schema_and_import),
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Line,
            &envelope,
            1,
        );
        assert_error(
            bind(&envelope, 1, 0, 1, &schema_and_import),
            LedgerEventErrorCode::IncompatibleField,
            LedgerEventErrorField::RowSchema,
            &envelope,
            1,
        );

        let mut row = valid.clone();
        row.insert("import_id".into(), Value::Null);
        row.insert("month".into(), Value::Null);
        let candidate = project_row(row).unwrap();
        assert_error(
            bind(&envelope, 1, 0, 1, &candidate),
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::BundleId,
            &envelope,
            1,
        );

        let mut row = valid.clone();
        row.insert("month".into(), Value::Null);
        row.insert("day".into(), json!("invalid"));
        let candidate = project_row(row).unwrap();
        assert_error(
            bind(&envelope, 1, 0, 1, &candidate),
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Shard,
            &envelope,
            1,
        );

        let mut row = valid.clone();
        row.insert("day".into(), json!("invalid"));
        row.insert("normalized_ref".into(), json!("wrong"));
        let candidate = project_row(row).unwrap();
        assert_error(
            bind(&envelope, 1, 0, 1, &candidate),
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::Day,
            &envelope,
            1,
        );

        let mut row = valid.clone();
        row.insert("normalized_ref".into(), json!("wrong"));
        row.insert("dedupe_key".into(), json!("wrong"));
        let candidate = project_row(row).unwrap();
        assert_error(
            bind(&envelope, 1, 0, 1, &candidate),
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::NormalizedRef,
            &envelope,
            1,
        );

        let mut row = valid;
        row.insert("dedupe_key".into(), json!("wrong"));
        row.insert("raw_ref".into(), json!("wrong"));
        let candidate = project_row(row).unwrap();
        assert_error(
            bind(&envelope, 1, 0, 1, &candidate),
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::DedupeKey,
            &envelope,
            1,
        );
    }
}

#[test]
fn identity_code_points_optional_states_and_supplied_digests_are_preserved() {
    let (envelope, row) = context();
    let encoded = serde_json::to_string(&Value::Object(row.clone())).unwrap();
    let BodyValue::Object(mut object) = parse(encoded.as_bytes()).unwrap() else {
        unreachable!()
    };
    for (field, code_points) in [
        ("record_type", vec![u32::from(b'R'), 0xd800, 0x1c]),
        ("start_date", vec![u32::from(b'S'), 0xdc00, 0x1f]),
        ("end_date", vec![0xd800]),
        ("source_record_id", vec![0xdc00]),
    ] {
        object.insert(
            BodyString::from_code_points(field.bytes().map(u32::from).collect()).unwrap(),
            BodyValue::String(BodyString::from_code_points(code_points).unwrap()),
        );
    }
    let value = BodyValue::Object(object);
    let coordinate = Coordinate::new("bundle", "shard", 1);
    let presentation = PresentationRow::new(&value, &coordinate).unwrap();
    let candidate = project(&presentation, coordinate).unwrap();
    let event = bind(&envelope, 1, 0, 1, &candidate).unwrap();
    assert_eq!(
        event.record_type().code_points(),
        [u32::from(b'R'), 0xd800, 0x1c]
    );
    assert_eq!(
        event.start_time().code_points(),
        [u32::from(b'S'), 0xdc00, 0x1f]
    );
    assert_eq!(event.end_time().unwrap().code_points(), [0xd800]);
    assert_eq!(event.source_record_id().unwrap().code_points(), [0xdc00]);

    for field in ["end_date", "source_record_id", "raw_ref"] {
        for null in [false, true] {
            let mut state_row = row.clone();
            if null {
                state_row.insert(field.into(), Value::Null);
            } else {
                state_row.remove(field);
            }
            let candidate = project_row(state_row).unwrap();
            let event = bind(&envelope, 1, 0, 1, &candidate).unwrap();
            match field {
                "end_date" => assert!(event.end_time().is_none()),
                "source_record_id" => assert!(event.source_record_id().is_none()),
                "raw_ref" => assert!(event.raw_ref().is_none()),
                _ => unreachable!(),
            }
        }
    }

    for field in ["end_date", "source_record_id"] {
        let candidate = project_body_string(row.clone(), field, vec![0x1c, 0x1f]);
        let event = bind(&envelope, 1, 0, 1, &candidate).unwrap();
        let actual = if field == "end_date" {
            event.end_time().unwrap()
        } else {
            event.source_record_id().unwrap()
        };
        assert_eq!(actual.code_points(), [0x1c, 0x1f]);
    }

    let (oura_envelope, oura_row) = native_context(1);
    let original = project_row(oura_row.clone()).unwrap();
    let original_event = bind(&oura_envelope, 1, 0, 1, &original).unwrap();
    let mut changed = oura_row;
    changed.insert("value".into(), json!({"not": "the stored value"}));
    changed.insert("unit".into(), json!("different-unit"));
    changed.insert("metadata".into(), json!({"changed": true}));
    changed.insert("source_name".into(), json!("Different Synthetic Source"));
    let changed = project_row(changed).unwrap();
    let changed_event = bind(&oura_envelope, 1, 0, 1, &changed).unwrap();
    assert_eq!(changed_event, original_event);
    assert_eq!(changed_event.row_sha256().as_str(), ROW_SHA256);
    assert_eq!(changed_event.value_hash().as_str(), VALUE_HASH);
}

#[test]
fn constructor_errors_redact_megabyte_identity_and_raw_ref_sentinels() {
    let (envelope, valid) = context();
    let marker = "private-body-ledger-sentinel";
    for (field, value, expected_field) in [
        (
            "import_id",
            marker.repeat(40_000),
            LedgerEventErrorField::BundleId,
        ),
        (
            "raw_ref",
            format!(
                "imports/{}/raw/{}/..",
                envelope.bundle_id().as_str(),
                marker.repeat(40_000)
            ),
            LedgerEventErrorField::RawRef,
        ),
    ] {
        let mut row = valid.clone();
        row.insert(field.into(), json!(value));
        let candidate = project_row(row).unwrap();
        let error = bind(&envelope, 1, 0, 1, &candidate).unwrap_err();
        assert_eq!(error.field(), expected_field);
        assert_eq!(error.bundle(), Some(envelope.bundle_id()));
        assert_eq!(error.line(), 1);
        let display = error.to_string();
        assert_eq!(format!("{error:?}"), display);
        assert!(error.source().is_none());
        assert!(display.len() <= 256);
        assert!(!display.contains(marker));
    }
}
