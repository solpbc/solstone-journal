// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyEnvelope, BodyLedgerValidator, EnvelopeLedger, LedgerEventError, LedgerEventErrorCode,
    LedgerEventErrorField, decode_body_envelope,
};

use crate::support;

use support::{ledger_events_fixture, native_bundle_fixture};

fn assert_error(validator: BodyLedgerValidator<'_>, code: LedgerEventErrorCode, line: u64) {
    let error = validator.finish().expect_err("framing refuses");
    assert_error_value(&error, code, LedgerEventErrorField::Ledger, line);
}

fn assert_error_value(
    error: &LedgerEventError,
    code: LedgerEventErrorCode,
    field: LedgerEventErrorField,
    line: u64,
) {
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
    assert_eq!(error.line(), line);
}

fn assert_push_error_schedules(
    envelope: &BodyEnvelope,
    data: &[u8],
    code: LedgerEventErrorCode,
    field: LedgerEventErrorField,
    line: u64,
) -> LedgerEventError {
    let mut one_chunk = BodyLedgerValidator::new(envelope);
    let expected = one_chunk
        .push(data)
        .expect_err("one-chunk input refuses during push");
    assert_error_value(&expected, code, field, line);

    let mut splits = [1, data.len() / 2, data.len() - 1];
    splits.sort_unstable();
    for split in splits {
        let mut validator = BodyLedgerValidator::new(envelope);
        validator
            .push(&data[..split])
            .expect("prefix remains incomplete and valid");
        let actual = validator
            .push(&data[split..])
            .expect_err("adversarial split refuses during final push");
        assert_eq!(actual, expected);
    }
    expected
}

fn assert_finish_error_schedules(
    envelope: &BodyEnvelope,
    data: &[u8],
    code: LedgerEventErrorCode,
    line: u64,
) {
    let mut one_chunk = BodyLedgerValidator::new(envelope);
    one_chunk
        .push(data)
        .expect("unterminated frame remains buffered");
    let expected = one_chunk.finish().expect_err("unterminated frame refuses");
    assert_error_value(&expected, code, LedgerEventErrorField::Ledger, line);

    let mut splits = [1, data.len() / 2, data.len() - 1];
    splits.sort_unstable();
    for split in splits {
        let mut validator = BodyLedgerValidator::new(envelope);
        validator
            .push(&data[..split])
            .expect("unterminated prefix remains buffered");
        validator
            .push(&data[split..])
            .expect("unterminated suffix remains buffered");
        assert_eq!(
            validator.finish().expect_err("unterminated frame refuses"),
            expected
        );
    }
}

fn with_ledger_bytes(envelope: &BodyEnvelope, bytes: u64) -> BodyEnvelope {
    BodyEnvelope::new(
        envelope.bundle_id().clone(),
        envelope.source_family(),
        envelope.source_hash().clone(),
        envelope.raw_retention(),
        envelope.row_count(),
        envelope.days().to_vec(),
        envelope.shards().to_vec(),
        EnvelopeLedger::new(
            envelope.bundle_id(),
            bytes,
            envelope.ledger().events(),
            envelope.ledger().sha256().clone(),
        )
        .expect("replacement ledger is intrinsically valid"),
        envelope.summary_plan().cloned(),
    )
    .expect("replacement envelope is checked")
}

#[test]
fn missing_final_lf_uses_the_shared_decoder_path() {
    for case in [
        &native_bundle_fixture()["cases"][0],
        &ledger_events_fixture()["cases"][0],
    ] {
        let envelope = decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("envelope")
                .as_bytes(),
        )
        .expect("fixture envelope decodes");
        let frame = case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger")
            .as_bytes();
        assert_finish_error_schedules(
            &envelope,
            &frame[..frame.len() - 1],
            LedgerEventErrorCode::NoncanonicalJson,
            frame.iter().filter(|byte| **byte == b'\n').count() as u64,
        );
    }
}

#[test]
fn blank_and_partial_frames_are_checked_as_first_ledger_event() {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");

    let mut blank = BodyLedgerValidator::new(&envelope);
    let error = blank
        .push(b"\n")
        .expect_err("blank complete frame refuses during push");
    assert_error_value(
        &error,
        LedgerEventErrorCode::MalformedJson,
        LedgerEventErrorField::Ledger,
        1,
    );

    let mut partial = BodyLedgerValidator::new(&envelope);
    partial
        .push(b"{")
        .expect("bounded partial frame is buffered until finish");
    assert_error(partial, LedgerEventErrorCode::MalformedJson, 1);

    let mut validator = BodyLedgerValidator::new(&envelope);
    let error = validator
        .push(b"{}\n")
        .expect_err("missing schema refuses during push");
    assert_eq!(error.code(), LedgerEventErrorCode::MissingField);
    assert_eq!(error.field(), LedgerEventErrorField::Schema);
    assert_eq!(error.line(), 1);
}

#[test]
fn frame_cap_is_checked_before_scanning_or_buffering() {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");

    let mut exact_cap = vec![b'x'; 65_536];
    exact_cap.push(b'\n');
    let exact_envelope = with_ledger_bytes(&envelope, exact_cap.len() as u64);
    assert_push_error_schedules(
        &exact_envelope,
        &exact_cap,
        LedgerEventErrorCode::MalformedJson,
        LedgerEventErrorField::Ledger,
        1,
    );

    let mut over_cap = vec![b'x'; 65_537];
    over_cap.push(b'\n');
    let over_envelope = with_ledger_bytes(&envelope, over_cap.len() as u64);
    assert_push_error_schedules(
        &over_envelope,
        &over_cap,
        LedgerEventErrorCode::InputTooLarge,
        LedgerEventErrorField::Ledger,
        1,
    );
}

#[test]
fn semantic_and_reference_errors_pass_through_unchanged() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let frame = case["expected_ledger_jsonl"].as_str().expect("ledger");
    let expected_bundle = envelope.bundle_id().as_str();
    let alternate_bundle = "body-00000000000000000000000000";
    assert_eq!(alternate_bundle.len(), expected_bundle.len());

    for (mutated, code, field) in [
        (
            frame.replacen(
                "\"schema\":\"solstone.body.ledger_event.v1\"",
                "\"schema\":\"xolstone.body.ledger_event.v1\"",
                1,
            ),
            LedgerEventErrorCode::InvalidField,
            LedgerEventErrorField::Schema,
        ),
        (
            frame.replacen(
                &format!("\"bundle_id\":\"{expected_bundle}\""),
                &format!("\"bundle_id\":\"{alternate_bundle}\""),
                1,
            ),
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::BundleId,
        ),
    ] {
        assert_eq!(mutated.len(), frame.len(), "mutation remains byte-isolated");
        let error = assert_push_error_schedules(&envelope, mutated.as_bytes(), code, field, 1);
        assert_eq!(error.bundle(), Some(envelope.bundle_id()));
    }
}

#[test]
fn blank_between_valid_frames_is_line_two_for_all_chunk_schedules() {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let data = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger")
        .as_bytes();
    let first_end = data.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    let mut input = data[..first_end].to_vec();
    input.push(b'\n');
    input.extend_from_slice(&data[first_end..]);

    let mut one_chunk = BodyLedgerValidator::new(&envelope);
    let one_error = one_chunk
        .push(&input)
        .expect_err("blank second frame refuses");
    assert_error_value(
        &one_error,
        LedgerEventErrorCode::MalformedJson,
        LedgerEventErrorField::Ledger,
        2,
    );

    let mut split = BodyLedgerValidator::new(&envelope);
    split
        .push(&input[..first_end])
        .expect("first valid frame succeeds");
    let split_error = split
        .push(&input[first_end..])
        .expect_err("blank second frame refuses after a split");
    assert_eq!(split_error, one_error);
}
