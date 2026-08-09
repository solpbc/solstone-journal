// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyEnvelope, BodyRawRetention, BodySourceFamily, BodySourceHash,
    BundleId, EnvelopeLedger, EnvelopeShard, encode_body_envelope,
};

#[test]
fn same_family_source_hash_twin_changes_only_source_hash() {
    let baseline = single_shard_envelope(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        10,
        digest('a'),
        10,
        digest('b'),
    );
    let twin = single_shard_envelope(
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        10,
        digest('a'),
        10,
        digest('b'),
    );
    let mut baseline = encoded_object(&baseline);
    let mut twin = encoded_object(&twin);
    assert_ne!(baseline["source_hash"], twin["source_hash"]);
    baseline.as_object_mut().unwrap().remove("source_hash");
    twin.as_object_mut().unwrap().remove("source_hash");
    assert_eq!(baseline, twin);
}

#[test]
fn descriptor_twin_changes_only_shard_and_ledger_bytes_and_digests() {
    let baseline = single_shard_envelope(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        10,
        digest('a'),
        10,
        digest('b'),
    );
    let twin = single_shard_envelope(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        11,
        digest('c'),
        11,
        digest('d'),
    );
    let mut baseline = encoded_object(&baseline);
    let mut twin = encoded_object(&twin);
    assert_ne!(baseline["shards"][0]["bytes"], twin["shards"][0]["bytes"]);
    assert_ne!(baseline["shards"][0]["sha256"], twin["shards"][0]["sha256"]);
    assert_ne!(baseline["ledger"]["bytes"], twin["ledger"]["bytes"]);
    assert_ne!(baseline["ledger"]["sha256"], twin["ledger"]["sha256"]);
    for object in [&mut baseline, &mut twin] {
        object["shards"][0].as_object_mut().unwrap().remove("bytes");
        object["shards"][0]
            .as_object_mut()
            .unwrap()
            .remove("sha256");
        object["ledger"].as_object_mut().unwrap().remove("bytes");
        object["ledger"].as_object_mut().unwrap().remove("sha256");
    }
    assert_eq!(baseline, twin);
}

#[test]
fn two_shard_row_redistribution_changes_only_shard_rows() {
    let baseline = two_shard_envelope(3, 2);
    let twin = two_shard_envelope(2, 3);
    let mut baseline = encoded_object(&baseline);
    let mut twin = encoded_object(&twin);
    assert_eq!(baseline["row_count"], twin["row_count"]);
    assert_eq!(baseline["ledger"]["events"], twin["ledger"]["events"]);
    assert_eq!(baseline["days"], twin["days"]);
    assert_eq!(baseline["summary_plan"], twin["summary_plan"]);
    assert_ne!(baseline["shards"][0]["rows"], twin["shards"][0]["rows"]);
    assert_ne!(baseline["shards"][1]["rows"], twin["shards"][1]["rows"]);
    for object in [&mut baseline, &mut twin] {
        for shard in object["shards"].as_array_mut().unwrap() {
            shard.as_object_mut().unwrap().remove("rows");
        }
    }
    assert_eq!(baseline, twin);
}

fn single_shard_envelope(
    hash: &str,
    shard_bytes: u64,
    shard_digest: BodyDigest,
    ledger_bytes: u64,
    ledger_digest: BodyDigest,
) -> BodyEnvelope {
    let bundle = bundle();
    let day = BodyDay::from_bytes(b"20260102").unwrap();
    BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::OuraApi,
        source_hash(hash),
        BodyRawRetention::RetainParsed,
        1,
        vec![day.clone()],
        vec![EnvelopeShard::new(&bundle, 0, day.month(), shard_bytes, 1, shard_digest).unwrap()],
        EnvelopeLedger::new(&bundle, ledger_bytes, 1, ledger_digest).unwrap(),
        None,
    )
    .unwrap()
}

fn two_shard_envelope(first_rows: u64, second_rows: u64) -> BodyEnvelope {
    let bundle = bundle();
    let days = [
        b"20260129",
        b"20260130",
        b"20260131",
        b"20260201",
        b"20260202",
    ]
    .into_iter()
    .map(|day| BodyDay::from_bytes(day).unwrap())
    .collect::<Vec<_>>();
    BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::OuraApi,
        source_hash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        BodyRawRetention::RetainParsed,
        5,
        days,
        vec![
            EnvelopeShard::new(
                &bundle,
                0,
                solstone_core_body_source::BodyMonth::from_bytes(b"2026-01").unwrap(),
                10,
                first_rows,
                digest('a'),
            )
            .unwrap(),
            EnvelopeShard::new(
                &bundle,
                1,
                solstone_core_body_source::BodyMonth::from_bytes(b"2026-02").unwrap(),
                10,
                second_rows,
                digest('b'),
            )
            .unwrap(),
        ],
        EnvelopeLedger::new(&bundle, 10, 5, digest('c')).unwrap(),
        None,
    )
    .unwrap()
}

fn encoded_object(envelope: &BodyEnvelope) -> Value {
    serde_json::from_slice(&encode_body_envelope(envelope).unwrap()).unwrap()
}

fn bundle() -> BundleId {
    BundleId::from_bytes(b"body-00000000000000000000000000").unwrap()
}

fn source_hash(hash: &str) -> BodySourceHash {
    BodySourceHash::from_bytes_for_family(hash.as_bytes(), &BodySourceFamily::OuraApi).unwrap()
}

fn digest(character: char) -> BodyDigest {
    let value = format!("sha256:{}", character.to_string().repeat(64));
    BodyDigest::from_bytes(value.as_bytes()).unwrap()
}
