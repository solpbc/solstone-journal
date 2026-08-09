// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, BodyLedgerValidator, EnvelopeLedger, LedgerEventErrorCode,
    LedgerEventErrorField, decode_body_envelope, decode_body_ledger_event,
    encode_body_ledger_event,
};

mod support;

use support::{build_ledger_event, ledger_events_fixture, native_bundle_fixture};

const MAX_LEDGER_EVENT_FRAME_BYTES: usize = 65_537;
const MAX_VALIDATOR_OVERHEAD_BYTES: u64 = 128 * 1024;

fn max_frame() -> (BodyEnvelope, Vec<u8>) {
    let case = &native_bundle_fixture()["cases"][1];
    let fixture_envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let row = case["expected_normalized_jsonl"]
        .as_str()
        .expect("normalized row");
    let value_hash = BodyDigest::from_bytes(
        serde_json::from_str::<serde_json::Value>(case["expected_ledger_jsonl"].as_str().unwrap())
            .unwrap()["value_hash"]
            .as_str()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let prefix = format!(
        "imports/{}/raw/oura/",
        fixture_envelope.bundle_id().as_str()
    );
    let baseline = build_ledger_event(
        &fixture_envelope,
        &serde_json::to_string(&serde_json::json!({
            "day": "20260102",
            "dedupe_key": "sha256:cf5b6fc199a3bcbc4d9361346d957f9098c356fe75f226803d2bd57580d95258",
            "end_date": "2026-01-03",
            "import_id": fixture_envelope.bundle_id().as_str(),
            "month": "2026-01",
            "normalized_ref": format!("imports/{}/normalized/2026-01.jsonl#L1", fixture_envelope.bundle_id().as_str()),
            "raw_ref": format!("{prefix}a"),
            "record_type": "oura.daily_readiness",
            "schema": "solstone.health.oura.v1",
            "source_family": "oura_api",
            "source_record_id": "synthetic-readiness-1",
            "start_date": "2026-01-02"
        }))
        .unwrap(),
        0,
        1,
        1,
        None,
        value_hash.clone(),
    );
    let extra = MAX_LEDGER_EVENT_FRAME_BYTES - encode_body_ledger_event(&baseline).unwrap().len();
    let raw = row.replace(
        "imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/raw/oura/daily_readiness-0001.json#item-0",
        &format!("{}{}", prefix, "a".repeat(extra + 1)),
    );
    let event = build_ledger_event(&fixture_envelope, &raw, 0, 1, 1, None, value_hash);
    let frame = encode_body_ledger_event(&event).expect("maximum event encodes");
    assert_eq!(frame.len(), MAX_LEDGER_EVENT_FRAME_BYTES);

    let digest = format!("sha256:{:x}", Sha256::digest(&frame));
    let envelope = BodyEnvelope::new(
        fixture_envelope.bundle_id().clone(),
        fixture_envelope.source_family(),
        fixture_envelope.source_hash().clone(),
        fixture_envelope.raw_retention(),
        fixture_envelope.row_count(),
        fixture_envelope.days().to_vec(),
        fixture_envelope.shards().to_vec(),
        EnvelopeLedger::new(
            fixture_envelope.bundle_id(),
            frame.len() as u64,
            1,
            BodyDigest::from_bytes(digest.as_bytes()).unwrap(),
        )
        .unwrap(),
        fixture_envelope.summary_plan().cloned(),
    )
    .unwrap();
    (envelope, frame)
}

#[test]
fn maximum_frame_validator_overhead_is_bounded() {
    let (envelope, frame) = max_frame();
    let baseline = allocation_counter::measure(|| {
        decode_body_ledger_event(&frame, &envelope, 1).expect("maximum frame decodes");
    });
    let validator = allocation_counter::measure(|| {
        let mut validator = BodyLedgerValidator::new(&envelope);
        validator.push(&frame);
        validator.finish().expect("maximum frame validates");
    });
    assert!(
        validator.bytes_max.saturating_sub(baseline.bytes_max) <= MAX_VALIDATOR_OVERHEAD_BYTES,
        "validator peak {} exceeded decoder baseline {} by more than {} bytes",
        validator.bytes_max,
        baseline.bytes_max,
        MAX_VALIDATOR_OVERHEAD_BYTES
    );
}

#[test]
fn one_byte_chunks_have_bounded_overhead_for_the_largest_committed_ledger() {
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
    assert_eq!(data.len(), 2493);

    let baseline = allocation_counter::measure(|| {
        for (index, frame) in data.split_inclusive(|byte| *byte == b'\n').enumerate() {
            decode_body_ledger_event(frame, &envelope, index as u64 + 1)
                .expect("fixture frame decodes");
        }
    });
    let validator = allocation_counter::measure(|| {
        let mut validator = BodyLedgerValidator::new(&envelope);
        for byte in data {
            validator.push(std::slice::from_ref(byte));
        }
        validator.finish().expect("fixture ledger validates");
    });
    assert!(
        validator.bytes_max.saturating_sub(baseline.bytes_max) <= MAX_VALIDATOR_OVERHEAD_BYTES,
        "validator peak {} exceeded decoder baseline {} by more than {} bytes",
        validator.bytes_max,
        baseline.bytes_max,
        MAX_VALIDATOR_OVERHEAD_BYTES
    );
}

#[test]
fn unterminated_oversized_frame_has_bounded_allocation() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let frame = vec![b'x'; MAX_LEDGER_EVENT_FRAME_BYTES + 1];
    let info = allocation_counter::measure(|| {
        let mut validator = BodyLedgerValidator::new(&envelope);
        validator.push(&frame);
        let error = validator.finish().expect_err("oversized frame refuses");
        assert_eq!(error.code(), LedgerEventErrorCode::InputTooLarge);
        assert_eq!(error.field(), LedgerEventErrorField::Ledger);
        assert_eq!(error.line(), 1);
    });
    assert!(
        info.bytes_max <= MAX_VALIDATOR_OVERHEAD_BYTES,
        "peak was {} bytes",
        info.bytes_max
    );
}

#[test]
fn trailing_megabyte_is_not_buffered_after_the_declared_event_count() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let mut input = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger")
        .as_bytes()
        .to_vec();
    input.extend_from_slice(b"before-overrun-sentinel");
    input.extend(std::iter::repeat_n(b'x', 1_048_576));
    input.extend_from_slice(b"after-overrun-sentinel");
    let info = allocation_counter::measure(|| {
        let mut validator = BodyLedgerValidator::new(&envelope);
        validator.push(&input);
        let error = validator.finish().expect_err("trailing bytes refuse");
        assert_eq!(error.code(), LedgerEventErrorCode::CountMismatch);
        assert_eq!(error.field(), LedgerEventErrorField::Ledger);
        assert_eq!(error.line(), 2);
    });
    assert!(
        info.bytes_max <= MAX_VALIDATOR_OVERHEAD_BYTES,
        "peak was {} bytes",
        info.bytes_max
    );
}

fn fixture_ledgers() -> Vec<(BodyEnvelope, Vec<u8>)> {
    let mut ledgers = native_bundle_fixture()["cases"]
        .as_array()
        .expect("native cases")
        .iter()
        .map(|case| {
            (
                decode_body_envelope(
                    case["expected_envelope_jsonl"]
                        .as_str()
                        .expect("envelope")
                        .as_bytes(),
                )
                .expect("fixture envelope decodes"),
                case["expected_ledger_jsonl"]
                    .as_str()
                    .expect("ledger")
                    .as_bytes()
                    .to_vec(),
            )
        })
        .collect::<Vec<_>>();
    let case = &ledger_events_fixture()["cases"][0];
    ledgers.push((
        decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("envelope")
                .as_bytes(),
        )
        .expect("fixture envelope decodes"),
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger")
            .as_bytes()
            .to_vec(),
    ));
    ledgers
}

fn assert_panic_free(envelope: &BodyEnvelope, chunks: &[&[u8]]) {
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut validator = BodyLedgerValidator::new(envelope);
            for chunk in chunks {
                validator.push(chunk);
            }
            let _ = validator.finish();
        }))
        .is_ok()
    );
}

#[test]
fn every_fixture_prefix_and_deterministic_split_is_panic_free() {
    for (envelope, data) in fixture_ledgers() {
        let prefix_lengths = if data.is_empty() {
            vec![0]
        } else {
            (0..data.len()).collect()
        };
        for length in prefix_lengths {
            let prefix = &data[..length];
            assert_panic_free(&envelope, &[prefix]);
            if length > 1 {
                for offset in [1_usize, 17, 97] {
                    let split = offset % (length - 1) + 1;
                    assert_panic_free(&envelope, &[&prefix[..split], &prefix[split..]]);
                }
            }
            if length > 2 {
                for offset in [1_usize, 17, 97] {
                    let first = offset % (length - 1) + 1;
                    let second = first + (offset * 31 % (length - first)) + 1;
                    assert_panic_free(
                        &envelope,
                        &[&prefix[..first], &prefix[first..second], &prefix[second..]],
                    );
                }
            }
        }
    }
}
