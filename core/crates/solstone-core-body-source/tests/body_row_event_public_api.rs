// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use serde_json::json;
use solstone_core_body_source::{
    BodyRowEventErrorKind, decode_body_envelope, decode_body_ledger_event, validate_body_row_event,
};

mod support;

use support::{build_ledger_event, native_bundle_fixture, sha256_body_digest};

fn fixture(
    index: usize,
) -> (
    solstone_core_body_source::BodyEnvelope,
    String,
    solstone_core_body_source::BodyLedgerEvent,
) {
    let case = &native_bundle_fixture()["cases"][index];
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

fn event_for(
    envelope: &solstone_core_body_source::BodyEnvelope,
    row: &str,
    event: &solstone_core_body_source::BodyLedgerEvent,
    frame: &[u8],
) -> solstone_core_body_source::BodyLedgerEvent {
    build_ledger_event(
        envelope,
        row,
        0,
        event.sequence(),
        event.line(),
        Some(sha256_body_digest(frame)),
        event.value_hash().clone(),
    )
}

#[test]
fn public_surface_validates_native_rows_and_returns_owned_pure_events() {
    for index in [0, 1] {
        let (envelope, row, event) = fixture(index);
        let envelope_before = envelope.clone();
        let event_before = event.clone();
        let frame = format!("{row}\n").into_bytes();
        let returned =
            validate_body_row_event(&envelope, &frame, &event).expect("public API succeeds");
        drop(frame);
        assert_eq!(returned.bundle_id(), envelope.bundle_id());
        assert_eq!(returned.sequence(), 1);
        assert_eq!(returned.row_sha256(), event.row_sha256());
        assert_eq!(returned.record_type(), event.record_type());
        // Shared references make mutation impossible; this keeps that contract observable.
        assert_eq!(envelope, envelope_before);
        assert_eq!(event, event_before);
    }
}

#[test]
fn public_errors_expose_structured_sources() {
    let (envelope, row, event) = fixture(0);
    let parse_frame = b"{\n";
    let parse_event = event_for(&envelope, &row, &event, parse_frame);
    let parse_error = validate_body_row_event(&envelope, parse_frame, &parse_event).unwrap_err();
    assert!(matches!(
        parse_error.kind(),
        BodyRowEventErrorKind::Parse(_)
    ));
    assert!(
        Error::source(&parse_error)
            .unwrap()
            .is::<solstone_core_body_source::ParseError>()
    );

    let candidate_frame = b"{}\n";
    let candidate_event = event_for(&envelope, &row, &event, candidate_frame);
    let candidate_error =
        validate_body_row_event(&envelope, candidate_frame, &candidate_event).unwrap_err();
    match candidate_error.kind() {
        BodyRowEventErrorKind::Candidate(inner) => {
            assert_eq!(inner.code.as_str(), "unsupported_schema");
            assert_eq!(inner.field.as_str(), "schema");
        }
        other => panic!("expected candidate error, got {other:?}"),
    }
    assert!(
        Error::source(&candidate_error)
            .unwrap()
            .is::<solstone_core_body_source::CandidateError>()
    );

    let mut value: serde_json::Value = serde_json::from_str(&row).unwrap();
    value["import_id"] = json!("body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z");
    let event_frame = format!("{}\n", serde_json::to_string(&value).unwrap());
    let event = event_for(&envelope, &row, &event, event_frame.as_bytes());
    let event_error =
        validate_body_row_event(&envelope, event_frame.as_bytes(), &event).unwrap_err();
    match event_error.kind() {
        BodyRowEventErrorKind::Event(inner) => {
            assert_eq!(inner.code().as_str(), "reference_mismatch");
            assert_eq!(inner.field().as_str(), "bundle_id");
        }
        other => panic!("expected event error, got {other:?}"),
    }
    assert!(
        Error::source(&event_error)
            .unwrap()
            .is::<solstone_core_body_source::LedgerEventError>()
    );
}
