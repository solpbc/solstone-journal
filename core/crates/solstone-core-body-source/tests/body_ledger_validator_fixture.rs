// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyEnvelope, BodyLedgerValidator, ValidatedBodyLedger, decode_body_envelope,
};

mod support;

use support::{ledger_events_fixture, native_bundle_fixture};

fn assert_receipt(envelope: &BodyEnvelope, receipt: &ValidatedBodyLedger) {
    assert_eq!(receipt.bundle_id(), envelope.bundle_id());
    assert_eq!(receipt.bytes(), envelope.ledger().bytes());
    assert_eq!(receipt.events(), envelope.ledger().events());
    assert_eq!(receipt.sha256(), envelope.ledger().sha256());
}

fn finish_with_chunks(
    envelope: &BodyEnvelope,
    chunks: impl IntoIterator<Item = Vec<u8>>,
) -> ValidatedBodyLedger {
    let mut validator = BodyLedgerValidator::new(envelope);
    for chunk in chunks {
        validator.push(&chunk).expect("fixture chunk validates");
    }
    let receipt = validator.finish().expect("fixture ledger validates");
    assert_receipt(envelope, &receipt);
    receipt
}

fn assert_all_schedules(envelope: &BodyEnvelope, data: &[u8]) {
    let expected = finish_with_chunks(envelope, [data.to_vec()]);
    assert_eq!(
        finish_with_chunks(envelope, data.iter().map(|byte| vec![*byte])),
        expected
    );

    for split in 1..data.len() {
        assert_eq!(
            finish_with_chunks(envelope, [data[..split].to_vec(), data[split..].to_vec()]),
            expected
        );
    }

    for offset in [1_usize, 17, 97, 257] {
        if data.len() > 2 {
            let first = offset % (data.len() - 1) + 1;
            let second = first + (offset * 31 % (data.len() - first)) + 1;
            assert_eq!(
                finish_with_chunks(
                    envelope,
                    [
                        data[..first].to_vec(),
                        data[first..second].to_vec(),
                        data[second..].to_vec(),
                    ],
                ),
                expected
            );
        }
    }

    let midpoint = data.len() / 2;
    assert_eq!(
        finish_with_chunks(
            envelope,
            [
                Vec::new(),
                data[..midpoint].to_vec(),
                Vec::new(),
                data[midpoint..].to_vec(),
                Vec::new(),
            ],
        ),
        expected
    );
}

fn assert_fixture_oracle(envelope: &BodyEnvelope, data: &[u8]) {
    let events = data.iter().filter(|byte| **byte == b'\n').count() as u64;
    let digest = format!("sha256:{:x}", Sha256::digest(data));
    assert_eq!(envelope.ledger().bytes(), data.len() as u64);
    assert_eq!(envelope.ledger().events(), events);
    assert_eq!(envelope.ledger().sha256().as_str(), digest);
}

#[test]
fn every_fixture_ledger_validates_for_all_chunk_schedules() {
    for case in native_bundle_fixture()["cases"]
        .as_array()
        .expect("native cases")
    {
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
        assert_fixture_oracle(&envelope, data);
        let envelope_before = envelope.clone();
        let data_before = data.to_vec();
        assert_all_schedules(&envelope, data);
        assert_eq!(envelope, envelope_before);
        assert_eq!(data, data_before);
    }

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
    assert_fixture_oracle(&envelope, data);
    let envelope_before = envelope.clone();
    let data_before = data.to_vec();
    assert_all_schedules(&envelope, data);
    assert_eq!(envelope, envelope_before);
    assert_eq!(data, data_before);
}
