// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyEnvelope, BodyShardValidator, EnvelopeErrorCode, EnvelopeErrorField, EnvelopeLedger,
    EnvelopeShard, decode_body_envelope,
};

mod support;

use support::{native_bundle_fixture, sha256_body_digest};

const MAX_VALIDATOR_ALLOCATION: u64 = 128 * 1024;

fn fixture_envelope() -> BodyEnvelope {
    decode_body_envelope(
        native_bundle_fixture()["cases"][1]["expected_envelope_jsonl"]
            .as_str()
            .unwrap()
            .as_bytes(),
    )
    .unwrap()
}

fn envelope_for(data: &[u8]) -> BodyEnvelope {
    let source = fixture_envelope();
    let shard = EnvelopeShard::new(
        source.bundle_id(),
        0,
        source.shards()[0].month().clone(),
        data.len() as u64,
        1,
        sha256_body_digest(data),
    )
    .unwrap();
    BodyEnvelope::new(
        source.bundle_id().clone(),
        source.source_family(),
        source.source_hash().clone(),
        source.raw_retention(),
        1,
        source.days().to_vec(),
        vec![shard],
        EnvelopeLedger::new(
            source.bundle_id(),
            source.ledger().bytes(),
            1,
            source.ledger().sha256().clone(),
        )
        .unwrap(),
        source.summary_plan().cloned(),
    )
    .unwrap()
}

#[test]
fn successful_large_shard_does_not_allocate_proportionally() {
    let mut data = vec![b'x'; 300 * 1024];
    *data.last_mut().unwrap() = b'\n';
    let envelope = envelope_for(&data);

    let one_chunk = allocation_counter::measure(|| {
        let mut validator = BodyShardValidator::new(&envelope, 0).unwrap();
        validator.push(&data).unwrap();
        validator.finish().unwrap();
    });
    assert!(
        one_chunk.bytes_max <= MAX_VALIDATOR_ALLOCATION,
        "one-chunk peak was {} bytes",
        one_chunk.bytes_max
    );

    let one_byte = allocation_counter::measure(|| {
        let mut validator = BodyShardValidator::new(&envelope, 0).unwrap();
        for byte in &data {
            validator.push(std::slice::from_ref(byte)).unwrap();
        }
        validator.finish().unwrap();
    });
    assert!(
        one_byte.bytes_max <= MAX_VALIDATOR_ALLOCATION,
        "one-byte peak was {} bytes",
        one_byte.bytes_max
    );
}

#[test]
fn over_boundary_megabytes_refuse_before_proportional_allocation() {
    let declared = b"x\n";
    let envelope = envelope_for(declared);
    let mut oversized = vec![b'x'; 2 * 1024 * 1024];
    oversized.extend_from_slice(b"raw-owner-sentinel\n");

    let info = allocation_counter::measure(|| {
        let mut validator = BodyShardValidator::new(&envelope, 0).unwrap();
        let error = validator.push(&oversized).unwrap_err();
        assert_eq!(error.code(), EnvelopeErrorCode::CountMismatch);
        assert_eq!(error.field(), EnvelopeErrorField::ShardBytes);
        assert_eq!(error.index(), Some(0));
        assert!(!format!("{error:?}").contains("raw-owner-sentinel"));
    });
    assert!(
        info.bytes_max <= MAX_VALIDATOR_ALLOCATION,
        "over-boundary peak was {} bytes",
        info.bytes_max
    );
}
