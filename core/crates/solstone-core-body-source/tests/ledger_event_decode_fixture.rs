// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{
    BodyDigest, decode_body_envelope, decode_body_ledger_event, encode_body_ledger_event,
};

mod support;

use support::{build_ledger_event, ledger_events_fixture, native_bundle_fixture};

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("fixture digest is valid")
}

#[test]
fn fixture_events_decode_to_the_existing_constructor_oracle_and_reencode_exactly() {
    let mut decoded_count = 0;
    for case in native_bundle_fixture()["cases"]
        .as_array()
        .expect("fixture cases")
    {
        let ledger = case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger frame");
        if ledger.is_empty() {
            continue;
        }
        let envelope = decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("envelope frame")
                .as_bytes(),
        )
        .expect("fixture envelope decodes");
        let expected: Value = serde_json::from_str(ledger).expect("ledger object");
        let oracle = build_ledger_event(
            &envelope,
            case["expected_normalized_jsonl"]
                .as_str()
                .expect("normalized row")
                .trim_end_matches('\n'),
            0,
            expected["sequence"].as_u64().expect("sequence"),
            expected["line"].as_u64().expect("line"),
            None,
            digest(expected["value_hash"].as_str().expect("value hash")),
        );
        let actual = decode_body_ledger_event(
            ledger.as_bytes(),
            &envelope,
            expected["sequence"].as_u64().expect("sequence"),
        )
        .expect("fixture ledger event decodes");
        assert_eq!(actual, oracle);
        assert_eq!(
            encode_body_ledger_event(&actual).unwrap(),
            ledger.as_bytes()
        );
        decoded_count += 1;
    }

    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope frame")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let rows = case["shards"]
        .as_array()
        .expect("shards")
        .iter()
        .enumerate()
        .flat_map(|(shard_index, shard)| {
            shard["expected_jsonl"]
                .as_str()
                .expect("shard rows")
                .lines()
                .map(move |row| (row, shard_index as u64))
        });
    for ((row, shard_index), frame) in rows.zip(
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger frames")
            .lines(),
    ) {
        let expected: Value = serde_json::from_str(frame).expect("ledger object");
        let sequence = expected["sequence"].as_u64().expect("sequence");
        let oracle = build_ledger_event(
            &envelope,
            row,
            shard_index,
            sequence,
            expected["line"].as_u64().expect("line"),
            None,
            digest(expected["value_hash"].as_str().expect("value hash")),
        );
        let input = format!("{frame}\n");
        let actual = decode_body_ledger_event(input.as_bytes(), &envelope, sequence)
            .expect("fixture ledger event decodes");
        assert_eq!(actual, oracle);
        assert_eq!(encode_body_ledger_event(&actual).unwrap(), input.as_bytes());
        if sequence == 3 {
            assert_eq!(actual.shard(), "normalized/2026-02.jsonl");
            assert_eq!(actual.line(), 1);
        }
        decoded_count += 1;
    }
    assert_eq!(decoded_count, 5);
}
