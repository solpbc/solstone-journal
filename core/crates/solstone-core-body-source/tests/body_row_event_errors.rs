// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use serde_json::json;
use solstone_core_body_source::{
    BodyRowEventError, BodyRowEventErrorKind, decode_body_envelope, decode_body_ledger_event,
    validate_body_row_event,
};

use crate::support;

use support::{build_ledger_event, native_bundle_fixture, sha256_body_digest};

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

fn error_for(frame: &[u8]) -> BodyRowEventError {
    let (envelope, row, event) = base();
    let event = event_for(&envelope, &row, &event, frame);
    validate_body_row_event(&envelope, frame, &event).expect_err("witness refuses")
}

fn mutate(row: &str, field: &str, replacement: serde_json::Value) -> String {
    let mut value: serde_json::Value = serde_json::from_str(row).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert(field.into(), replacement);
    serde_json::to_string(&value).unwrap()
}

#[test]
fn all_kinds_have_stable_spelling_accessors_sources_and_value_semantics() {
    let (envelope, row, event) = base();
    let malformed = b"{\n";
    let mut import_mismatch: serde_json::Value = serde_json::from_str(&row).unwrap();
    import_mismatch["import_id"] = json!("body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z");
    let import_frame = format!("{}\n", serde_json::to_string(&import_mismatch).unwrap());
    let record_frame = format!("{}\n", mutate(&row, "record_type", json!("changed")));
    let witnesses = [
        error_for(&vec![b'x'; 1_048_577]),
        error_for(b"\n"),
        validate_body_row_event(&envelope, format!("{row} \n").as_bytes(), &event).unwrap_err(),
        error_for(malformed),
        error_for(b"{}\n"),
        validate_body_row_event(
            &envelope,
            import_frame.as_bytes(),
            &event_for(&envelope, &row, &event, import_frame.as_bytes()),
        )
        .unwrap_err(),
        validate_body_row_event(
            &envelope,
            record_frame.as_bytes(),
            &event_for(&envelope, &row, &event, record_frame.as_bytes()),
        )
        .unwrap_err(),
    ];
    let spellings = [
        "input_too_large",
        "invalid_framing",
        "row_digest_mismatch",
        "parse",
        "candidate",
        "event",
        "event_mismatch",
    ];
    for (error, spelling) in witnesses.iter().zip(spellings) {
        assert_eq!(error.kind().as_str(), spelling);
        assert_eq!(error.bundle(), event.bundle_id());
        assert_eq!(error.sequence(), event.sequence());
        let display = format!("{error}");
        let debug = format!("{error:?}");
        assert_eq!(debug, display);
        // These are the maximum bundle/sequence values reachable through this
        // public API; the error constructor is crate-private and sequences are
        // bounded by the real fixture envelope.
        for rendered in [&display, &debug] {
            assert!(rendered.is_ascii());
            assert!(rendered.len() <= 256);
        }
        let has_source = matches!(
            error.kind(),
            BodyRowEventErrorKind::Parse(_)
                | BodyRowEventErrorKind::Candidate(_)
                | BodyRowEventErrorKind::Event(_)
        );
        assert_eq!(Error::source(error).is_some(), has_source);
        assert_eq!(error.clone(), *error);
    }
    assert_ne!(witnesses[0], witnesses[1]);
}

#[test]
fn outer_rendering_never_leaks_row_content_or_inner_display() {
    let marker = "forbidden-body-row-event-content";
    let frame = format!("{{\"record_type\":\"{marker}\"}}\n");
    let error = error_for(frame.as_bytes());
    assert!(matches!(error.kind(), BodyRowEventErrorKind::Candidate(_)));
    for rendered in [format!("{error}"), format!("{error:?}")] {
        assert!(!rendered.contains(marker));
        assert_eq!(
            rendered,
            "body-row-event[body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y]#E1 candidate"
        );
    }
}

#[test]
fn precedence_size_before_framing() {
    let error = error_for(&vec![b'x'; 1_048_577]);
    assert_eq!(error.kind(), &BodyRowEventErrorKind::InputTooLarge);
}

#[test]
fn precedence_framing_before_digest() {
    let (envelope, _, event) = base();
    let error = validate_body_row_event(&envelope, b"{\n\n", &event).unwrap_err();
    assert_eq!(error.kind(), &BodyRowEventErrorKind::InvalidFraming);
}

#[test]
fn precedence_digest_before_parse() {
    let (envelope, _, event) = base();
    let error = validate_body_row_event(&envelope, b"{\n", &event).unwrap_err();
    assert_eq!(error.kind(), &BodyRowEventErrorKind::RowDigestMismatch);
}

#[test]
fn precedence_parse_before_candidate() {
    let error = error_for(b"{\"record_type\":\n");
    assert!(matches!(error.kind(), BodyRowEventErrorKind::Parse(_)));
}

#[test]
fn precedence_candidate_before_event() {
    let error = error_for(b"{\"import_id\":\"wrong\"}\n");
    assert!(matches!(error.kind(), BodyRowEventErrorKind::Candidate(_)));
}

#[test]
fn precedence_event_before_final_equality() {
    let (envelope, row, event) = base();
    let frame = format!(
        "{}\n",
        mutate(&row, "import_id", json!("body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z"))
    );
    let event = event_for(&envelope, &row, &event, frame.as_bytes());
    let error = validate_body_row_event(&envelope, frame.as_bytes(), &event).unwrap_err();
    assert!(matches!(error.kind(), BodyRowEventErrorKind::Event(_)));
}
