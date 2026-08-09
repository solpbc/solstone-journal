// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyDigest, BodyRowEventErrorKind, LedgerEventErrorCode, LedgerEventErrorField,
    decode_body_envelope, decode_body_ledger_event, validate_body_row_event,
};

mod support;

use support::{build_ledger_event, native_bundle_fixture};

fn digest(bytes: &[u8]) -> BodyDigest {
    let text = format!("sha256:{:x}", Sha256::digest(bytes));
    BodyDigest::from_bytes(text.as_bytes()).unwrap()
}

fn base() -> (
    solstone_core_body_source::BodyEnvelope,
    String,
    solstone_core_body_source::BodyLedgerEvent,
) {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()).unwrap();
    let row = case["expected_normalized_jsonl"]
        .as_str()
        .unwrap()
        .trim_end()
        .to_owned();
    let event = decode_body_ledger_event(
        case["expected_ledger_jsonl"].as_str().unwrap().as_bytes(),
        &envelope,
        1,
    )
    .unwrap();
    (envelope, row, event)
}

fn mutate(row: &str, field: &str, replacement: Value) -> String {
    let mut value: Value = serde_json::from_str(row).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert(field.to_owned(), replacement);
    serde_json::to_string(&value).unwrap()
}

fn original_event_with_mutated_digest(
    envelope: &solstone_core_body_source::BodyEnvelope,
    row: &str,
    event: &solstone_core_body_source::BodyLedgerEvent,
    frame: &[u8],
) -> solstone_core_body_source::BodyLedgerEvent {
    build_ledger_event(
        envelope,
        row,
        0,
        1,
        1,
        Some(digest(frame)),
        event.value_hash().clone(),
    )
}

fn assert_event_error(row: &str, code: LedgerEventErrorCode, field: LedgerEventErrorField) {
    let (envelope, original, event) = base();
    let frame = format!("{row}\n");
    let event = original_event_with_mutated_digest(&envelope, &original, &event, frame.as_bytes());
    let error =
        validate_body_row_event(&envelope, frame.as_bytes(), &event).expect_err("row refuses");
    match error.kind() {
        BodyRowEventErrorKind::Event(inner) => {
            assert_eq!(inner.code(), code);
            assert_eq!(inner.field(), field);
        }
        other => panic!("expected event error, got {other:?}"),
    }
}

#[test]
fn cross_checked_row_references_fail_event_construction() {
    let (_, row, _) = base();
    // These values are independently checked by BodyLedgerEvent::new.
    assert_event_error(
        &mutate(&row, "import_id", json!("body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z")),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::BundleId,
    );
    let mut schema_and_family: Value = serde_json::from_str(&row).unwrap();
    schema_and_family["schema"] = json!("solstone.health.oura.v1");
    schema_and_family["source_family"] = json!("oura_api");
    assert_event_error(
        &serde_json::to_string(&schema_and_family).unwrap(),
        LedgerEventErrorCode::IncompatibleField,
        LedgerEventErrorField::RowSchema,
    );
    assert_event_error(
        &mutate(&row, "month", json!("2026-02")),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Shard,
    );
    assert_event_error(
        &mutate(
            &row,
            "normalized_ref",
            json!("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y/normalized/2026-01.jsonl#L9"),
        ),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::NormalizedRef,
    );
    assert_event_error(
        &mutate(&row, "day", json!("20260201")),
        LedgerEventErrorCode::ReferenceMismatch,
        LedgerEventErrorField::Day,
    );
    assert_event_error(
        &mutate(&row, "raw_ref", json!("not-an-import")),
        LedgerEventErrorCode::InvalidField,
        LedgerEventErrorField::RawRef,
    );
}

#[test]
fn stored_verbatim_row_fields_require_final_event_equality() {
    let (_, row, _) = base();
    // Dedupe is only parsed; record IDs, type, and times are stored verbatim.
    for (field, value) in [
        (
            "dedupe_key",
            json!("sha256:0000000000000000000000000000000000000000000000000000000000000000"),
        ),
        ("source_record_id", json!("changed-record")),
        ("record_type", json!("changed-type")),
        ("start_date", json!("2026-01-02 06:31:00 -0700")),
        ("end_date", json!("2026-01-02 07:16:00 -0700")),
    ] {
        let (envelope, original, event) = base();
        let mutated = mutate(&row, field, value);
        let frame = format!("{mutated}\n");
        let event =
            original_event_with_mutated_digest(&envelope, &original, &event, frame.as_bytes());
        let error = validate_body_row_event(&envelope, frame.as_bytes(), &event)
            .expect_err("stored field differs");
        assert_eq!(
            error.kind(),
            &BodyRowEventErrorKind::EventMismatch,
            "{field}"
        );
    }
}

#[test]
fn digest_precedes_projection_and_untracked_fields_do_not_change_event() {
    let (envelope, row, event) = base();
    let changed = mutate(&row, "record_type", json!("changed-type"));
    let changed_frame = format!("{changed}\n");
    let error = validate_body_row_event(&envelope, changed_frame.as_bytes(), &event)
        .expect_err("stale digest refuses first");
    assert_eq!(error.kind(), &BodyRowEventErrorKind::RowDigestMismatch);

    let untracked = mutate(&row, "future_unrecognized", json!({"x": 1}));
    let untracked_frame = format!("{untracked}\n");
    let error = validate_body_row_event(&envelope, untracked_frame.as_bytes(), &event)
        .expect_err("bytes changed");
    assert_eq!(error.kind(), &BodyRowEventErrorKind::RowDigestMismatch);
    let updated =
        original_event_with_mutated_digest(&envelope, &row, &event, untracked_frame.as_bytes());
    assert_eq!(
        validate_body_row_event(&envelope, untracked_frame.as_bytes(), &updated),
        Ok(updated)
    );

    // These fields project into LedgerCandidate but are not retained in
    // BodyLedgerEvent, so an updated row digest is sufficient for replay.
    for (field, value) in [
        ("value", json!("46")),
        ("metadata", json!({"changed": true})),
        ("source_name", json!("Changed Watch")),
        ("source_version", json!("2.0")),
        ("unit", json!("seconds")),
        ("kind", json!("changed-kind")),
    ] {
        let mutated = mutate(&row, field, value);
        let frame = format!("{mutated}\n");
        let updated = original_event_with_mutated_digest(&envelope, &row, &event, frame.as_bytes());
        assert_eq!(
            validate_body_row_event(&envelope, frame.as_bytes(), &updated),
            Ok(updated),
            "{field} is row-only"
        );
    }
}
