// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, LedgerEventErrorCode, LedgerEventErrorField, canonicalize,
    decode_body_envelope, decode_body_ledger_event, encode_body_ledger_event, parse,
};

use crate::support;

use support::{build_ledger_event, ledger_events_fixture, native_bundle_fixture};

fn context() -> (BodyEnvelope, String) {
    context_for_native_case(0)
}

fn context_for_native_case(index: usize) -> (BodyEnvelope, String) {
    let case = &native_bundle_fixture()["cases"][index];
    (
        decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("envelope frame")
                .as_bytes(),
        )
        .expect("fixture envelope decodes"),
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger frame")
            .to_owned(),
    )
}

fn canonical(value: Value) -> Vec<u8> {
    let raw = serde_json::to_vec(&value).expect("JSON serializes");
    let body = parse(&raw).expect("JSON parses");
    let mut frame = canonicalize(&body)
        .expect("JSON canonicalizes")
        .into_bytes();
    frame.push(b'\n');
    frame
}

fn assert_error(
    frame: &[u8],
    envelope: &BodyEnvelope,
    expected_sequence: u64,
    code: LedgerEventErrorCode,
    field: LedgerEventErrorField,
) {
    let error = decode_body_ledger_event(frame, envelope, expected_sequence).expect_err("refuses");
    assert_eq!(error.bundle(), Some(envelope.bundle_id()));
    assert_eq!(error.line(), expected_sequence);
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
}

#[test]
fn expected_sequence_precedes_every_frame_scan_failure() {
    let (envelope, _) = context();
    let oversized = vec![b'{'; 65_538];
    for frame in [b"{".as_slice(), oversized.as_slice()] {
        assert_error(
            frame,
            &envelope,
            0,
            LedgerEventErrorCode::InvalidSequence,
            LedgerEventErrorField::Sequence,
        );
    }
    assert_error(
        b"{",
        &envelope,
        1,
        LedgerEventErrorCode::MalformedJson,
        LedgerEventErrorField::Ledger,
    );
    assert_error(
        &oversized,
        &envelope,
        1,
        LedgerEventErrorCode::InputTooLarge,
        LedgerEventErrorField::Ledger,
    );
}

#[test]
fn decoded_references_and_nullable_fields_follow_the_checked_contract() {
    let (envelope, frame) = context();
    let mut value: Value = serde_json::from_str(&frame).expect("fixture ledger object");
    value["sequence"] = json!(2);
    assert_error(
        &canonical(value.clone()),
        &envelope,
        1,
        LedgerEventErrorCode::InvalidSequence,
        LedgerEventErrorField::Sequence,
    );

    value = serde_json::from_str(&frame).expect("fixture ledger object");
    value["shard"] = json!("normalized/2099-01.jsonl");
    assert_error(
        &canonical(value.clone()),
        &envelope,
        1,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
    );
    value["shard"] = serde_json::from_str::<Value>(&frame).unwrap()["shard"].clone();
    value["line"] = json!(2);
    assert_error(
        &canonical(value),
        &envelope,
        1,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Line,
    );

    let mut value: Value = serde_json::from_str(&frame).expect("fixture ledger object");
    value["source_family"] = json!("oura_api");
    assert_error(
        &canonical(value),
        &envelope,
        1,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::SourceFamily,
    );

    for key in ["end_time", "raw_ref", "source_record_id"] {
        let mut null: Value = serde_json::from_str(&frame).expect("fixture ledger object");
        null[key] = Value::Null;
        assert!(
            decode_body_ledger_event(&canonical(null), &envelope, 1).is_ok(),
            "{key}"
        );

        let mut missing: Value = serde_json::from_str(&frame).expect("fixture ledger object");
        missing.as_object_mut().expect("object").remove(key);
        assert_error(
            &canonical(missing),
            &envelope,
            1,
            LedgerEventErrorCode::MissingField,
            match key {
                "end_time" => LedgerEventErrorField::EndTime,
                "raw_ref" => LedgerEventErrorField::RawRef,
                "source_record_id" => LedgerEventErrorField::SourceRecordId,
                _ => unreachable!(),
            },
        );
    }
}

#[test]
fn projection_fields_are_closed_required_and_type_checked_in_fixed_order() {
    let (envelope, frame) = context();
    let base: Value = serde_json::from_str(&frame).expect("fixture ledger object");
    let fields = [
        ("schema", LedgerEventErrorField::Schema),
        ("bundle_id", LedgerEventErrorField::BundleId),
        ("sequence", LedgerEventErrorField::Sequence),
        ("row_schema", LedgerEventErrorField::RowSchema),
        ("shard", LedgerEventErrorField::Shard),
        ("line", LedgerEventErrorField::Line),
        ("normalized_ref", LedgerEventErrorField::NormalizedRef),
        ("row_sha256", LedgerEventErrorField::RowSha256),
        ("dedupe_key", LedgerEventErrorField::DedupeKey),
        ("source_family", LedgerEventErrorField::SourceFamily),
        ("source_record_id", LedgerEventErrorField::SourceRecordId),
        ("record_type", LedgerEventErrorField::RecordType),
        ("start_time", LedgerEventErrorField::StartTime),
        ("end_time", LedgerEventErrorField::EndTime),
        ("day", LedgerEventErrorField::Day),
        ("value_hash", LedgerEventErrorField::ValueHash),
        ("raw_ref", LedgerEventErrorField::RawRef),
    ];
    for (key, field) in fields {
        let mut missing = base.clone();
        missing.as_object_mut().expect("object").remove(key);
        assert_error(
            &canonical(missing),
            &envelope,
            1,
            LedgerEventErrorCode::MissingField,
            field,
        );

        let mut wrong_type = base.clone();
        wrong_type[key] = json!({"wrong": true});
        assert_error(
            &canonical(wrong_type),
            &envelope,
            1,
            LedgerEventErrorCode::WrongType,
            field,
        );
    }

    let mut unknown = base.clone();
    unknown["unknown"] = Value::Null;
    assert_error(
        &canonical(unknown),
        &envelope,
        1,
        LedgerEventErrorCode::UnknownField,
        LedgerEventErrorField::Ledger,
    );
    for (key, marker) in [
        ("ascii_unknown", Some("ascii_unknown")),
        ("", None),
        ("é", Some("é")),
    ] {
        let mut unknown = base.clone();
        unknown[key] = Value::Null;
        let frame = canonical(unknown);
        let error = decode_body_ledger_event(&frame, &envelope, 1).expect_err("unknown refuses");
        assert_eq!(error.code(), LedgerEventErrorCode::UnknownField);
        assert_eq!(error.field(), LedgerEventErrorField::Ledger);
        if let Some(marker) = marker {
            assert!(!error.to_string().contains(marker));
            assert!(!format!("{error:?}").contains(marker));
        }
    }
    let surrogate_unknown = format!(
        "{},\"\\ud800\":null}}\n",
        frame.trim_end().trim_end_matches('}')
    );
    let error = decode_body_ledger_event(surrogate_unknown.as_bytes(), &envelope, 1)
        .expect_err("unknown refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::UnknownField);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert!(!error.to_string().contains("ud800"));
    let noncanonical_unknown = format!(r#"{{"unknown":null,{}"#, frame.trim_start_matches('{'));
    assert_error(
        noncanonical_unknown.as_bytes(),
        &envelope,
        1,
        LedgerEventErrorCode::NoncanonicalJson,
        LedgerEventErrorField::Ledger,
    );
    let escaped_unknown = format!(
        "{},\"\\u0075nknown\":null}}\n",
        frame.trim_end().trim_end_matches('}')
    );
    assert_error(
        escaped_unknown.as_bytes(),
        &envelope,
        1,
        LedgerEventErrorCode::NoncanonicalJson,
        LedgerEventErrorField::Ledger,
    );
    let duplicate_unknown = format!(
        "{},\"unknown\":null,\"unknown\":null}}\n",
        frame.trim_end().trim_end_matches('}')
    );
    assert_error(
        duplicate_unknown.as_bytes(),
        &envelope,
        1,
        LedgerEventErrorCode::NoncanonicalJson,
        LedgerEventErrorField::Ledger,
    );

    let mut reverse_lexicographic = base.clone();
    reverse_lexicographic["schema"] = json!("wrong");
    reverse_lexicographic
        .as_object_mut()
        .expect("object")
        .remove("bundle_id");
    assert_error(
        &canonical(reverse_lexicographic),
        &envelope,
        1,
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::Schema,
    );
    let mut unknown_and_invalid = base;
    unknown_and_invalid["schema"] = json!("wrong");
    unknown_and_invalid["zz_unknown"] = Value::Null;
    assert_error(
        &canonical(unknown_and_invalid),
        &envelope,
        1,
        LedgerEventErrorCode::UnknownField,
        LedgerEventErrorField::Ledger,
    );
}

#[test]
fn value_and_reference_validation_rejects_exactly_the_event_contract_twins() {
    let (envelope, frame) = context();
    let base: Value = serde_json::from_str(&frame).expect("fixture ledger object");

    for (key, replacement, field) in [
        ("schema", json!("wrong"), LedgerEventErrorField::Schema),
        ("bundle_id", json!("wrong"), LedgerEventErrorField::BundleId),
        (
            "row_schema",
            json!("wrong"),
            LedgerEventErrorField::RowSchema,
        ),
        (
            "row_sha256",
            json!("sha256:abc"),
            LedgerEventErrorField::RowSha256,
        ),
        (
            "dedupe_key",
            json!("sha256:abc"),
            LedgerEventErrorField::DedupeKey,
        ),
        (
            "value_hash",
            json!("sha256:abc"),
            LedgerEventErrorField::ValueHash,
        ),
        ("day", json!("20260230"), LedgerEventErrorField::Day),
    ] {
        let mut value = base.clone();
        value[key] = replacement;
        assert_error(
            &canonical(value),
            &envelope,
            1,
            LedgerEventErrorCode::InvalidField,
            field,
        );
    }

    let mut source_family = base.clone();
    source_family["source_family"] = json!("not_a_family");
    assert_error(
        &canonical(source_family),
        &envelope,
        1,
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::SourceFamily,
    );

    let mut wrong_bundle = base.clone();
    wrong_bundle["bundle_id"] = json!("body-00000000000000000000000000");
    assert_error(
        &canonical(wrong_bundle),
        &envelope,
        1,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::BundleId,
    );
    for row_schema in ["solstone.health.normalized.v1", "solstone.health.oura.v1"] {
        let mut incompatible = base.clone();
        incompatible["row_schema"] = json!(row_schema);
        assert_error(
            &canonical(incompatible),
            &envelope,
            1,
            LedgerEventErrorCode::IncompatibleField,
            LedgerEventErrorField::RowSchema,
        );
    }
    for field in ["sequence", "line"] {
        for (literal, code) in [
            ("1.0", LedgerEventErrorCode::WrongType),
            ("-1", LedgerEventErrorCode::InvalidField),
            ("18446744073709551616", LedgerEventErrorCode::InvalidField),
        ] {
            let replacement = format!("\"{field}\":{literal}");
            let needle = format!("\"{field}\":1");
            let input = frame.replacen(&needle, &replacement, 1);
            assert_error(
                input.as_bytes(),
                &envelope,
                1,
                code,
                if field == "sequence" {
                    LedgerEventErrorField::Sequence
                } else {
                    LedgerEventErrorField::Line
                },
            );
        }
    }

    for key in ["record_type", "start_time"] {
        let mut value = base.clone();
        value[key] = json!(" \t\u{3000}");
        assert_error(
            &canonical(value),
            &envelope,
            1,
            LedgerEventErrorCode::InvalidField,
            if key == "record_type" {
                LedgerEventErrorField::RecordType
            } else {
                LedgerEventErrorField::StartTime
            },
        );
    }

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
        let mut value = base.clone();
        value["raw_ref"] = json!(raw_ref);
        assert_error(
            &canonical(value),
            &envelope,
            1,
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::RawRef,
        );
    }
}

#[test]
fn exact_reference_fields_reject_lossy_or_normalizing_twins_for_both_families() {
    for index in 0..2 {
        let (envelope, frame) = context_for_native_case(index);
        let base: Value = serde_json::from_str(&frame).expect("fixture ledger object");
        let bundle = base["bundle_id"].as_str().expect("bundle id");
        let shard = base["shard"].as_str().expect("shard");
        let normalized_ref = base["normalized_ref"].as_str().expect("normalized ref");
        for (field, exact, variants, code, error_field) in [
            (
                "schema",
                "solstone.body.ledger_event.v1".to_owned(),
                vec![
                    "SOLSTONE.BODY.LEDGER_EVENT.V1".to_owned(),
                    " solstone.body.ledger_event.v1".to_owned(),
                    "solstone.body.ledger_event.v1é".to_owned(),
                    "solstone.body.ledger_event.v1\\ud800".to_owned(),
                ],
                LedgerEventErrorCode::InvalidField,
                LedgerEventErrorField::Schema,
            ),
            (
                "bundle_id",
                bundle.to_owned(),
                vec![
                    bundle.to_uppercase(),
                    format!(" {bundle}"),
                    format!("{bundle}é"),
                    format!("{bundle}\\ud800"),
                ],
                LedgerEventErrorCode::InvalidField,
                LedgerEventErrorField::BundleId,
            ),
            (
                "shard",
                shard.to_owned(),
                vec![
                    shard.to_uppercase(),
                    format!(" {shard}"),
                    format!("{shard}é"),
                    format!("{shard}\\ud800"),
                ],
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::Shard,
            ),
            (
                "normalized_ref",
                normalized_ref.to_owned(),
                vec![
                    normalized_ref.to_uppercase(),
                    format!(" {normalized_ref}"),
                    format!("{normalized_ref}é"),
                    format!("{normalized_ref}\\ud800"),
                ],
                LedgerEventErrorCode::ReferenceMismatch,
                LedgerEventErrorField::NormalizedRef,
            ),
        ] {
            let needle = format!("\"{field}\":\"{exact}\"");
            for replacement in variants {
                let input = if replacement.ends_with("\\ud800") {
                    let replacement = format!("\"{field}\":\"{replacement}\"");
                    frame.replacen(&needle, &replacement, 1).into_bytes()
                } else {
                    let mut changed = base.clone();
                    changed[field] = json!(replacement);
                    canonical(changed)
                };
                assert_error(&input, &envelope, 1, code, error_field);
            }
        }
    }
}

#[test]
fn multishard_sequence_three_resolves_to_the_second_shard_first_line() {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope frame")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let frame = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger frames")
        .lines()
        .nth(2)
        .expect("third event");
    let input = format!("{frame}\n");
    let event = decode_body_ledger_event(input.as_bytes(), &envelope, 3).expect("event decodes");
    assert_eq!(event.shard(), envelope.shards()[1].path());
    assert_eq!(event.line(), 1);
}

#[test]
fn cross_shard_day_and_multi_fault_precedence_follow_shared_validation() {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope frame")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let second: Value = serde_json::from_str(
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger frames")
            .lines()
            .nth(1)
            .expect("second event"),
    )
    .expect("ledger object");
    let mut wrong_shard = second.clone();
    wrong_shard["sequence"] = json!(3);
    assert_error(
        &canonical(wrong_shard),
        &envelope,
        3,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
    );
    let third: Value = serde_json::from_str(
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger frames")
            .lines()
            .nth(2)
            .expect("third event"),
    )
    .expect("ledger object");
    let mut first_next_shard = third;
    first_next_shard["sequence"] = json!(2);
    assert_error(
        &canonical(first_next_shard),
        &envelope,
        2,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
    );

    for day in ["20260104", "20260201"] {
        let mut value = second.clone();
        value["day"] = json!(day);
        assert_error(
            &canonical(value),
            &envelope,
            2,
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::Day,
        );
    }

    let mut shard_before_line = second.clone();
    shard_before_line["sequence"] = json!(3);
    shard_before_line["line"] = json!(1);
    assert_error(
        &canonical(shard_before_line),
        &envelope,
        3,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
    );

    let mut normalized_before_dedupe = second.clone();
    normalized_before_dedupe["normalized_ref"] = json!("wrong");
    normalized_before_dedupe["dedupe_key"] = json!("sha256:abc");
    assert_error(
        &canonical(normalized_before_dedupe),
        &envelope,
        2,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::NormalizedRef,
    );

    let mut bundle_before_sequence = second.clone();
    bundle_before_sequence["bundle_id"] = json!("body-00000000000000000000000000");
    bundle_before_sequence["sequence"] = json!(3);
    assert_error(
        &canonical(bundle_before_sequence),
        &envelope,
        2,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::BundleId,
    );

    let mut schema_before_shard = second.clone();
    schema_before_shard["row_schema"] = json!("solstone.health.oura.v1");
    schema_before_shard["shard"] = json!("normalized/2026-02.jsonl");
    assert_error(
        &canonical(schema_before_shard),
        &envelope,
        2,
        LedgerEventErrorCode::IncompatibleField,
        LedgerEventErrorField::RowSchema,
    );

    let mut dedupe_before_family = second.clone();
    dedupe_before_family["dedupe_key"] = json!("sha256:abc");
    dedupe_before_family["source_family"] = json!("not_a_family");
    assert_error(
        &canonical(dedupe_before_family),
        &envelope,
        2,
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::DedupeKey,
    );

    let mut family_before_day = second.clone();
    family_before_day["source_family"] = json!("oura_api");
    family_before_day["day"] = json!("20260201");
    assert_error(
        &canonical(family_before_day),
        &envelope,
        2,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::SourceFamily,
    );

    let mut normalized_before_day = second.clone();
    normalized_before_day["normalized_ref"] = json!("wrong");
    normalized_before_day["day"] = json!("20260201");
    assert_error(
        &canonical(normalized_before_day),
        &envelope,
        2,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::NormalizedRef,
    );

    let mut day_before_value_hash = second.clone();
    day_before_value_hash["day"] = json!("20260230");
    day_before_value_hash["value_hash"] = json!("sha256:abc");
    assert_error(
        &canonical(day_before_value_hash),
        &envelope,
        2,
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::Day,
    );

    let mut source_family_before_raw = second;
    source_family_before_raw["source_family"] = json!("oura_api");
    source_family_before_raw["raw_ref"] = json!("imports/wrong/raw/file");
    assert_error(
        &canonical(source_family_before_raw),
        &envelope,
        2,
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::SourceFamily,
    );
}

#[test]
fn exact_maximum_frame_decodes_and_one_extra_byte_refuses() {
    let case = &native_bundle_fixture()["cases"][1];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope frame")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let expected: Value = serde_json::from_str(
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger frame"),
    )
    .expect("ledger object");
    let row = case["expected_normalized_jsonl"]
        .as_str()
        .expect("normalized row");
    let prefix = format!("imports/{}/raw/oura/", envelope.bundle_id().as_str());
    let baseline_row = row.replace(
        expected["raw_ref"].as_str().expect("raw ref"),
        &format!("{prefix}a"),
    );
    let value_hash = BodyDigest::from_bytes(
        expected["value_hash"]
            .as_str()
            .expect("value hash")
            .as_bytes(),
    )
    .expect("value hash is valid");
    let baseline = build_ledger_event(
        &envelope,
        baseline_row.trim_end_matches('\n'),
        0,
        1,
        1,
        None,
        value_hash.clone(),
    );
    let required = 65_537
        - encode_body_ledger_event(&baseline)
            .expect("baseline encodes")
            .len();
    let maximum_row = row.replace(
        expected["raw_ref"].as_str().expect("raw ref"),
        &format!("{prefix}{}", "a".repeat(required + 1)),
    );
    let maximum = build_ledger_event(
        &envelope,
        maximum_row.trim_end_matches('\n'),
        0,
        1,
        1,
        None,
        value_hash,
    );
    let frame = encode_body_ledger_event(&maximum).expect("maximum event encodes");
    assert_eq!(frame.len(), 65_537);
    assert_eq!(
        decode_body_ledger_event(&frame, &envelope, 1).unwrap(),
        maximum
    );

    let mut over = frame.clone();
    over.push(b'x');
    assert_error(
        &over,
        &envelope,
        1,
        LedgerEventErrorCode::InputTooLarge,
        LedgerEventErrorField::Ledger,
    );
}
