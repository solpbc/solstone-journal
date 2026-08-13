// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyDigest, BodyLedgerEvent, BodyString, Coordinate, PresentationRow, decode_body_envelope,
    health_value_hash, parse, project,
};

use crate::support;

use support::{ledger_events_fixture, native_bundle_fixture};

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("fixture digest is valid")
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn text(value: &BodyString) -> String {
    value
        .code_points()
        .iter()
        .map(|code_point| char::from_u32(*code_point).expect("fixture text is scalar"))
        .collect()
}

fn candidate(
    row: &str,
    bundle: &str,
    shard: &str,
    line: u64,
) -> solstone_core_body_source::LedgerCandidate {
    let value = parse(row.as_bytes()).expect("fixture normalized row parses");
    let presentation = PresentationRow::new(&value, &Coordinate::new(bundle, shard, line))
        .expect("fixture normalized row is an object");
    project(&presentation, Coordinate::new(bundle, shard, line))
        .expect("fixture normalized row projects")
}

fn assert_event(event: &BodyLedgerEvent, expected: &Value) {
    assert_eq!(event.schema(), expected["schema"].as_str().unwrap());
    assert_eq!(
        event.bundle_id().as_str(),
        expected["bundle_id"].as_str().unwrap()
    );
    assert_eq!(event.sequence(), expected["sequence"].as_u64().unwrap());
    assert_eq!(
        event.row_schema().as_str(),
        expected["row_schema"].as_str().unwrap()
    );
    assert_eq!(event.shard(), expected["shard"].as_str().unwrap());
    assert_eq!(event.line(), expected["line"].as_u64().unwrap());
    assert_eq!(
        text(event.normalized_ref()),
        expected["normalized_ref"].as_str().unwrap()
    );
    assert_eq!(
        event.row_sha256().as_str(),
        expected["row_sha256"].as_str().unwrap()
    );
    assert_eq!(
        event.dedupe_key().as_str(),
        expected["dedupe_key"].as_str().unwrap()
    );
    assert_eq!(
        event.source_family().as_str(),
        expected["source_family"].as_str().unwrap()
    );
    assert_eq!(
        event.source_record_id().map(text),
        expected["source_record_id"].as_str().map(str::to_owned)
    );
    assert_eq!(
        text(event.record_type()),
        expected["record_type"].as_str().unwrap()
    );
    assert_eq!(
        text(event.start_time()),
        expected["start_time"].as_str().unwrap()
    );
    assert_eq!(
        event.end_time().map(text),
        expected["end_time"].as_str().map(str::to_owned)
    );
    assert_eq!(event.day().as_str(), expected["day"].as_str().unwrap());
    assert_eq!(
        event.value_hash().as_str(),
        expected["value_hash"].as_str().unwrap()
    );
    assert_eq!(
        event.raw_ref().map(text),
        expected["raw_ref"].as_str().map(str::to_owned)
    );
}

fn assert_events(
    envelope_jsonl: &str,
    rows: Vec<(String, usize)>,
    ledger_jsonl: &str,
    expected_ledger_sha256: &str,
    pre_enrichment_value_hash: bool,
) -> usize {
    let envelope =
        decode_body_envelope(envelope_jsonl.as_bytes()).expect("fixture envelope decodes");
    assert_eq!(sha256(ledger_jsonl.as_bytes()), expected_ledger_sha256);
    assert_eq!(envelope.ledger().sha256().as_str(), expected_ledger_sha256);
    let events: Vec<Value> = ledger_jsonl
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), events.len());
    for ((row, shard_index), expected) in rows.into_iter().zip(&events) {
        let row_frame = format!("{row}\n");
        let computed_row_sha256 = sha256(row_frame.as_bytes());
        assert_eq!(
            computed_row_sha256,
            expected["row_sha256"].as_str().unwrap()
        );
        let shard = expected["shard"].as_str().unwrap();
        let line = expected["line"].as_u64().unwrap();
        let candidate = candidate(&row, envelope.bundle_id().as_str(), shard, line);
        let computed_value_hash =
            health_value_hash(candidate.unit(), candidate.metadata(), candidate.value()).unwrap();
        let expected_value_hash = expected["value_hash"].as_str().unwrap();
        if pre_enrichment_value_hash {
            assert_ne!(
                computed_value_hash, expected_value_hash,
                "the stored enriched Apple workout must not certify its pre-enrichment hash"
            );
        } else {
            assert_eq!(computed_value_hash, expected_value_hash);
        }
        let event = BodyLedgerEvent::new(
            &envelope,
            expected["sequence"].as_u64().unwrap(),
            shard_index as u64,
            line,
            digest(&computed_row_sha256),
            digest(expected_value_hash),
            &candidate,
        )
        .expect("fixture event binds");
        assert_event(&event, expected);
    }
    events.len()
}

#[test]
fn binds_every_nonzero_native_and_multishard_fixture_event() {
    let mut count = 0;
    for case in native_bundle_fixture()["cases"].as_array().unwrap() {
        let normalized = case["expected_normalized_jsonl"].as_str().unwrap();
        let ledger = case["expected_ledger_jsonl"].as_str().unwrap();
        assert_eq!(
            sha256(normalized.as_bytes()),
            case["expected_normalized_sha256"].as_str().unwrap()
        );
        assert_eq!(
            sha256(ledger.as_bytes()),
            case["expected_ledger_sha256"].as_str().unwrap()
        );
        if ledger.is_empty() {
            assert!(normalized.is_empty());
            continue;
        }
        count += assert_events(
            case["expected_envelope_jsonl"].as_str().unwrap(),
            normalized.lines().map(|row| (row.to_owned(), 0)).collect(),
            ledger,
            case["expected_ledger_sha256"].as_str().unwrap(),
            case["name"].as_str().unwrap() == "apple_retain_complete_one_row",
        );
    }

    let case = &ledger_events_fixture()["cases"][0];
    let mut rows = Vec::new();
    for (index, shard) in case["shards"].as_array().unwrap().iter().enumerate() {
        let shard_jsonl = shard["expected_jsonl"].as_str().unwrap();
        assert_eq!(
            sha256(shard_jsonl.as_bytes()),
            shard["sha256"].as_str().unwrap()
        );
        let row_hashes = shard["row_sha256"].as_array().unwrap();
        assert_eq!(row_hashes.len(), shard_jsonl.lines().count());
        for (line, expected_hash) in shard_jsonl.lines().zip(row_hashes) {
            assert_eq!(
                sha256(format!("{line}\n").as_bytes()),
                expected_hash.as_str().unwrap()
            );
        }
        rows.extend(shard_jsonl.lines().map(|row| (row.to_owned(), index)));
    }
    count += assert_events(
        case["expected_envelope_jsonl"].as_str().unwrap(),
        rows,
        case["expected_ledger_jsonl"].as_str().unwrap(),
        case["expected_ledger_sha256"].as_str().unwrap(),
        false,
    );
    assert_eq!(count, 5);
}
