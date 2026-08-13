// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyLedgerValidator, LedgerEventErrorCode, LedgerEventErrorField, decode_body_envelope,
};

use crate::support;

use support::native_bundle_fixture;

#[test]
fn first_malformed_frame_poisoning_wins_over_later_data() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let valid = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger")
        .as_bytes();
    let mut validator = BodyLedgerValidator::new(&envelope);
    let first = validator
        .push(&[b"{\n".as_slice(), valid].concat())
        .expect_err("malformed first frame refuses during push");
    assert_eq!(first.code(), LedgerEventErrorCode::MalformedJson);
    assert_eq!(first.field(), LedgerEventErrorField::Ledger);
    assert_eq!(first.line(), 1);
    assert_eq!(
        validator
            .push(valid)
            .expect_err("later valid data replays poison"),
        first
    );
    assert_eq!(
        validator
            .push(b"")
            .expect_err("later empty chunk replays poison"),
        first
    );
    assert_eq!(
        validator
            .push(b"extra")
            .expect_err("later nonempty data replays poison"),
        first
    );
    assert_eq!(
        validator.finish().expect_err("finish replays poison"),
        first
    );
}

#[test]
fn overrun_in_the_same_chunk_is_not_inspected_as_a_frame() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let valid = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger")
        .as_bytes();
    let mut input = valid.to_vec();
    input.extend_from_slice(b"{");

    let mut validator = BodyLedgerValidator::new(&envelope);
    let first = validator
        .push(&input)
        .expect_err("overrun refuses during push");
    assert_eq!(first.code(), LedgerEventErrorCode::CountMismatch);
    assert_eq!(first.field(), LedgerEventErrorField::Ledger);
    assert_eq!(first.line(), 2);
    assert_eq!(
        validator
            .push(b"\n")
            .expect_err("later bytes replay overrun poison"),
        first
    );
    assert_eq!(
        validator.finish().expect_err("finish replays poison"),
        first
    );
}

#[test]
fn empty_pushes_do_not_change_poisoned_or_unpoisoned_state() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(b"").expect("empty push is a no-op");
    let first = validator
        .push(b"{\n")
        .expect_err("malformed frame refuses during push");
    assert_eq!(
        validator.push(b"").expect_err("empty push replays poison"),
        first
    );
    assert_eq!(
        validator.finish().expect_err("finish replays poison"),
        first
    );
}

#[test]
fn field_error_poison_is_bounded_redacted_and_replayed() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let valid = case["expected_ledger_jsonl"].as_str().expect("ledger");
    let sentinel = "owner-body-private-sentinel-x";
    assert_eq!(sentinel.len(), "solstone.body.ledger_event.v1".len());
    let invalid = valid.replacen("solstone.body.ledger_event.v1", sentinel, 1);
    assert_eq!(invalid.len(), valid.len());

    let mut validator = BodyLedgerValidator::new(&envelope);
    let first = validator
        .push(invalid.as_bytes())
        .expect_err("invalid schema refuses during push");
    assert_eq!(first.bundle(), Some(envelope.bundle_id()));
    assert_eq!(first.code(), LedgerEventErrorCode::InvalidField);
    assert_eq!(first.field(), LedgerEventErrorField::Schema);
    assert_eq!(first.line(), 1);
    let display = first.to_string();
    let debug = format!("{first:?}");
    assert_eq!(display, debug);
    assert!(display.is_ascii());
    assert!(display.len() <= 256);
    assert!(!display.contains(sentinel));
    assert!(!display.contains("sha256:"));
    assert!(!display.contains("imports/"));

    assert_eq!(
        validator
            .push(valid.as_bytes())
            .expect_err("later input replays field poison"),
        first
    );
    assert_eq!(
        validator.finish().expect_err("finish replays field poison"),
        first
    );
}
