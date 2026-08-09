// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::panic::{AssertUnwindSafe, catch_unwind};

use solstone_core_body_source::{
    BodyEnvelope, BodyLedgerEvent, BodyRowEventErrorKind, decode_body_envelope,
    decode_body_ledger_event, validate_body_row_event,
};

mod support;

use support::{
    build_ledger_event, ledger_events_fixture, native_bundle_fixture, sha256_body_digest,
};

const MAX_ROW_FRAME_BYTES: usize = 1_048_576;

fn base() -> (BodyEnvelope, String, BodyLedgerEvent) {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("envelope decodes");
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
    .expect("event decodes");
    (envelope, row, event)
}

fn event_for_frame(
    envelope: &BodyEnvelope,
    row: &str,
    event: &BodyLedgerEvent,
    frame: &[u8],
) -> BodyLedgerEvent {
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

fn assert_kind(frame: &[u8], spelling: &str) {
    let (envelope, row, event) = base();
    let event = event_for_frame(&envelope, &row, &event, frame);
    let error = validate_body_row_event(&envelope, frame, &event).expect_err("frame refuses");
    assert_eq!(error.kind().as_str(), spelling);
}

#[test]
fn framing_and_parse_boundaries_are_classified() {
    let (_, row, _) = base();
    assert_kind(b"", "invalid_framing");
    assert_kind(b"\n", "invalid_framing");
    assert_kind(row.as_bytes(), "invalid_framing");
    assert_kind(format!("{{\n{row}}}\n").as_bytes(), "invalid_framing");
    assert_kind(format!("{row}\n{row}\n").as_bytes(), "invalid_framing");
    assert_kind(format!("{row}{row}\n").as_bytes(), "parse");
    assert_kind(b"{\n", "parse");
    assert_kind(b"{\"day\":\n", "parse");
    for frame in [b"42\n".as_slice(), b"[1,2,3]\n", b"\"x\"\n", b"null\n"] {
        let (envelope, row, event) = base();
        let event = event_for_frame(&envelope, &row, &event, frame);
        let error =
            validate_body_row_event(&envelope, frame, &event).expect_err("nonobject refuses");
        assert!(matches!(error.kind(), BodyRowEventErrorKind::Candidate(_)));
    }
}

#[test]
fn crlf_and_size_boundary_are_accepted_before_later_stages() {
    let (envelope, row, event) = base();
    let crlf = format!("{row}\r\n");
    let crlf_event = event_for_frame(&envelope, &row, &event, crlf.as_bytes());
    assert_eq!(
        validate_body_row_event(&envelope, crlf.as_bytes(), &crlf_event),
        Ok(crlf_event)
    );

    let mut exact = row.into_bytes();
    exact.extend(std::iter::repeat_n(
        b' ',
        MAX_ROW_FRAME_BYTES - exact.len() - 1,
    ));
    exact.push(b'\n');
    let exact_event = event_for_frame(
        &envelope,
        std::str::from_utf8(&exact[..exact.len() - 1])
            .unwrap()
            .trim_end(),
        &event,
        &exact,
    );
    // Whitespace changes neither the projected candidate nor reconstructed event.
    assert!(validate_body_row_event(&envelope, &exact, &exact_event).is_ok());

    let mut over = exact[..exact.len() - 1].to_vec();
    over.push(b' ');
    over.push(b'\n');
    let over_event = event_for_frame(
        &envelope,
        std::str::from_utf8(&over[..over.len() - 1])
            .unwrap()
            .trim_end(),
        &event,
        &over,
    );
    let error =
        validate_body_row_event(&envelope, &over, &over_event).expect_err("oversize refuses");
    assert_eq!(error.kind(), &BodyRowEventErrorKind::Oversized);
}

#[test]
fn malformed_large_input_does_not_leak_content() {
    let (envelope, row, event) = base();
    let marker = "body-row-event-secret-".to_owned() + &"X".repeat(100_000);
    let frame = format!("{{\"marker\":\"{marker}\"}}\n");
    let event = event_for_frame(&envelope, &row, &event, frame.as_bytes());
    let error = validate_body_row_event(&envelope, frame.as_bytes(), &event)
        .expect_err("candidate refuses");
    assert!(matches!(error.kind(), BodyRowEventErrorKind::Candidate(_)));
    assert!(!format!("{error}").contains(&marker));
    assert!(!format!("{error:?}").contains(&marker));
}

#[test]
fn every_proper_committed_prefix_and_adversarial_bytes_are_panic_free() {
    let mut cases = Vec::new();
    for case in native_bundle_fixture()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| !case["expected_ledger_jsonl"].as_str().unwrap().is_empty())
    {
        let envelope =
            decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
                .unwrap();
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
        cases.push((envelope, row, event, 0));
    }
    let ledger = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        ledger["expected_envelope_jsonl"]
            .as_str()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    for (sequence, row) in ledger["shards"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|shard| shard["expected_jsonl"].as_str().unwrap().lines())
        .enumerate()
    {
        let frame = format!(
            "{}\n",
            ledger["expected_ledger_jsonl"]
                .as_str()
                .unwrap()
                .lines()
                .nth(sequence)
                .unwrap()
        );
        let event =
            decode_body_ledger_event(frame.as_bytes(), &envelope, sequence as u64 + 1).unwrap();
        cases.push((
            envelope.clone(),
            row.to_owned(),
            event,
            if sequence == 2 { 1 } else { 0 },
        ));
    }
    for (envelope, row, event, shard_index) in &cases {
        let frame = format!("{row}\n");
        for length in 0..frame.len() {
            let prefix = &frame.as_bytes()[..length];
            let event = build_ledger_event(
                envelope,
                row,
                *shard_index,
                event.sequence(),
                event.line(),
                Some(sha256_body_digest(prefix)),
                event.value_hash().clone(),
            );
            assert!(
                catch_unwind(AssertUnwindSafe(|| {
                    let _ = validate_body_row_event(envelope, prefix, &event);
                }))
                .is_ok(),
                "proper prefix length {length} panicked"
            );
        }
    }
    for bytes in [
        b"\0\0\0".as_slice(),
        b"\n\n\n",
        b"\xff\xfe\n",
        b"[[[[[[[[[[\n",
    ] {
        let (envelope, row, event) = base();
        let event = event_for_frame(&envelope, &row, &event, bytes);
        assert!(
            catch_unwind(AssertUnwindSafe(|| {
                let _ = validate_body_row_event(&envelope, bytes, &event);
            }))
            .is_ok()
        );
    }
}
