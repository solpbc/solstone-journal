// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyDigest, Coordinate, PresentationRow, decode_body_envelope, decode_body_ledger_event,
    encode_body_ledger_event, health_value_hash, parse, project, validate_body_row_event,
};

mod support;

use support::{ledger_events_fixture, native_bundle_fixture};

fn digest(bytes: &[u8]) -> BodyDigest {
    let text = format!("sha256:{:x}", Sha256::digest(bytes));
    BodyDigest::from_bytes(text.as_bytes()).expect("SHA-256 digest is valid")
}

#[test]
fn validates_all_committed_nonzero_rows_and_events() {
    let mut validated = 0;
    for case in native_bundle_fixture()["cases"].as_array().expect("cases") {
        let normalized = case["expected_normalized_jsonl"]
            .as_str()
            .expect("normalized");
        let ledger = case["expected_ledger_jsonl"].as_str().expect("ledger");
        if ledger.is_empty() {
            // The two discard fixtures deliberately have no rows to validate.
            assert!(normalized.is_empty());
            continue;
        }
        let envelope =
            decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
                .expect("fixture envelope decodes");
        let row = normalized
            .strip_suffix('\n')
            .expect("one normalized row and final LF");
        let row_frame = format!("{row}\n");
        let event_frame = ledger.as_bytes();
        let event = decode_body_ledger_event(event_frame, &envelope, 1).expect("event decodes");
        assert_eq!(digest(row_frame.as_bytes()), *event.row_sha256());
        let returned = validate_body_row_event(&envelope, row_frame.as_bytes(), &event)
            .expect("committed row validates");
        assert_eq!(returned, event);
        assert_eq!(
            encode_body_ledger_event(&returned).expect("event encodes"),
            event_frame
        );
        if case["name"].as_str() == Some("apple_retain_complete_one_row") {
            let value = parse(row.as_bytes()).expect("row parses");
            let coordinate =
                Coordinate::new(envelope.bundle_id().as_str(), event.shard(), event.line());
            let presentation = PresentationRow::new(&value, &coordinate).expect("object row");
            let candidate = project(&presentation, coordinate).expect("candidate projects");
            assert_ne!(
                health_value_hash(candidate.unit(), candidate.metadata(), candidate.value())
                    .expect("enriched value hashes"),
                event.value_hash().as_str(),
                "the committed Apple value hash remains the pre-enrichment value hash"
            );
        }
        validated += 1;
    }

    let case = &ledger_events_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("fixture envelope decodes");
    let event_frames: Vec<_> = case["expected_ledger_jsonl"]
        .as_str()
        .unwrap()
        .lines()
        .collect();
    let mut sequence = 1;
    for shard in case["shards"].as_array().expect("shards") {
        let expected_hashes = shard["row_sha256"].as_array().expect("row hashes");
        for (row, expected_hash) in shard["expected_jsonl"]
            .as_str()
            .unwrap()
            .lines()
            .zip(expected_hashes)
        {
            let row_frame = format!("{row}\n");
            let computed = digest(row_frame.as_bytes());
            assert_eq!(computed.as_str(), expected_hash.as_str().unwrap());
            let event_frame = format!("{}\n", event_frames[(sequence - 1) as usize]);
            let event = decode_body_ledger_event(event_frame.as_bytes(), &envelope, sequence)
                .expect("committed event decodes");
            let returned = validate_body_row_event(&envelope, row_frame.as_bytes(), &event)
                .expect("committed row validates");
            assert_eq!(returned, event);
            assert_eq!(
                encode_body_ledger_event(&returned).expect("event encodes"),
                event_frame.as_bytes()
            );
            sequence += 1;
            validated += 1;
        }
    }
    assert_eq!(validated, 5);
}
