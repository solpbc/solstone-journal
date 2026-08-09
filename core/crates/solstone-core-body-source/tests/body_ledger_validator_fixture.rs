// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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

fn finish_with_chunks(envelope: &BodyEnvelope, chunks: impl IntoIterator<Item = Vec<u8>>) {
    let mut validator = BodyLedgerValidator::new(envelope);
    for chunk in chunks {
        validator.push(&chunk);
    }
    let receipt = validator.finish().expect("fixture ledger validates");
    assert_receipt(envelope, &receipt);
}

fn assert_all_schedules(envelope: &BodyEnvelope, data: &[u8]) {
    finish_with_chunks(envelope, [data.to_vec()]);
    finish_with_chunks(envelope, data.iter().map(|byte| vec![*byte]));

    for split in 1..data.len() {
        finish_with_chunks(envelope, [data[..split].to_vec(), data[split..].to_vec()]);
    }

    for offset in [1_usize, 17, 97, 257] {
        if data.len() > 2 {
            let first = offset % (data.len() - 1) + 1;
            let second = first + (offset * 31 % (data.len() - first)) + 1;
            finish_with_chunks(
                envelope,
                [
                    data[..first].to_vec(),
                    data[first..second].to_vec(),
                    data[second..].to_vec(),
                ],
            );
        }
    }

    let midpoint = data.len() / 2;
    finish_with_chunks(
        envelope,
        [
            Vec::new(),
            data[..midpoint].to_vec(),
            Vec::new(),
            data[midpoint..].to_vec(),
            Vec::new(),
        ],
    );
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
        assert_all_schedules(&envelope, data);
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
    assert_all_schedules(&envelope, data);
}
