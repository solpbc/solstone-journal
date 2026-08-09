// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_body_source::{BodyDigest, decode_body_envelope, encode_body_ledger_event};

mod support;

use support::{build_ledger_event, ledger_events_fixture, native_bundle_fixture};

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("fixture digest is valid")
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[test]
fn fixture_events_encode_to_exact_canonical_jsonl_frames() {
    let mut count = 0;
    for case in native_bundle_fixture()["cases"].as_array().unwrap() {
        let normalized = case["expected_normalized_jsonl"].as_str().unwrap();
        let ledger = case["expected_ledger_jsonl"].as_str().unwrap();
        if ledger.is_empty() {
            assert!(normalized.is_empty());
            continue;
        }
        let envelope =
            decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
                .expect("fixture envelope decodes");
        let expected = serde_json::from_str::<serde_json::Value>(ledger).unwrap();
        let event = build_ledger_event(
            &envelope,
            normalized.trim_end_matches('\n'),
            0,
            expected["sequence"].as_u64().unwrap(),
            expected["line"].as_u64().unwrap(),
            None,
            digest(expected["value_hash"].as_str().unwrap()),
        );
        assert_eq!(sha256(normalized.as_bytes()), event.row_sha256().as_str());
        let frame = encode_body_ledger_event(&event).unwrap();
        assert_eq!(frame, ledger.as_bytes());
        assert_eq!(frame.len() as u64, envelope.ledger().bytes());
        assert_eq!(sha256(&frame), envelope.ledger().sha256().as_str());
        count += 1;
    }

    let case = &ledger_events_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()).unwrap();
    let expected_lines = case["expected_ledger_jsonl"].as_str().unwrap().lines();
    let rows = case["shards"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .flat_map(|(shard_index, shard)| {
            shard["expected_jsonl"]
                .as_str()
                .unwrap()
                .lines()
                .map(move |row| (row, shard_index as u64))
        });
    let mut frames = Vec::new();
    for ((row, shard_index), expected_line) in rows.zip(expected_lines) {
        let expected = serde_json::from_str::<serde_json::Value>(expected_line).unwrap();
        let event = build_ledger_event(
            &envelope,
            row,
            shard_index,
            expected["sequence"].as_u64().unwrap(),
            expected["line"].as_u64().unwrap(),
            None,
            digest(expected["value_hash"].as_str().unwrap()),
        );
        assert_eq!(
            sha256(format!("{row}\n").as_bytes()),
            event.row_sha256().as_str()
        );
        let frame = encode_body_ledger_event(&event).unwrap();
        assert_eq!(frame, format!("{expected_line}\n").as_bytes());
        frames.extend(frame);
        count += 1;
    }
    assert_eq!(
        frames,
        case["expected_ledger_jsonl"].as_str().unwrap().as_bytes()
    );
    assert_eq!(
        sha256(&frames),
        case["expected_ledger_sha256"].as_str().unwrap()
    );
    assert_eq!(
        frames.len(),
        case["expected_ledger_bytes"].as_u64().unwrap() as usize
    );
    assert_eq!(count, 5);
}
