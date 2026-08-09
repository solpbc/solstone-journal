// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyEnvelope, BodyMonth, BodyShardValidator, BundleId, EnvelopeError,
    EnvelopeErrorCode, EnvelopeErrorField, EnvelopeLedger, EnvelopeShard, ValidatedBodyShard,
    decode_body_envelope,
};

mod support;

use support::{envelope_multimonth_fixture, native_bundle_fixture, sha256_body_digest};

fn native_case(index: usize) -> (BodyEnvelope, Vec<u8>) {
    let case = &native_bundle_fixture()["cases"][index];
    (
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("fixture envelope decodes"),
        case["expected_normalized_jsonl"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec(),
    )
}

fn multimonth_cases() -> Vec<(BodyEnvelope, usize, Vec<u8>)> {
    let case = &envelope_multimonth_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("multi-month envelope decodes");
    case["digest_basis"]["shards"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, shard)| {
            (
                envelope.clone(),
                index,
                shard["exact_bytes"].as_str().unwrap().as_bytes().to_vec(),
            )
        })
        .collect()
}

fn validate(envelope: &BodyEnvelope, index: u64, chunks: &[&[u8]]) -> ValidatedBodyShard {
    let mut validator = BodyShardValidator::new(envelope, index).expect("shard index is valid");
    for chunk in chunks {
        validator.push(chunk).expect("chunk validates");
    }
    validator.finish().expect("complete shard validates")
}

fn assert_receipt(envelope: &BodyEnvelope, index: usize, receipt: &ValidatedBodyShard) {
    let descriptor = &envelope.shards()[index];
    assert_eq!(receipt.bundle_id(), envelope.bundle_id());
    assert_eq!(receipt.index(), index as u64);
    assert_eq!(receipt.descriptor(), descriptor);
    assert_eq!(receipt.path(), descriptor.path());
    assert_eq!(receipt.month(), descriptor.month());
    assert_eq!(receipt.bytes(), descriptor.bytes());
    assert_eq!(receipt.rows(), descriptor.rows());
    assert_eq!(receipt.sha256(), descriptor.sha256());
}

fn assert_schedules(envelope: &BodyEnvelope, index: usize, data: &[u8]) {
    assert_eq!(envelope.shards()[index].bytes(), data.len() as u64);
    assert_eq!(
        envelope.shards()[index].rows(),
        data.iter().filter(|byte| **byte == b'\n').count() as u64
    );
    assert_eq!(envelope.shards()[index].sha256(), &sha256_body_digest(data));

    let expected = validate(envelope, index as u64, &[data]);
    assert_receipt(envelope, index, &expected);
    let bytes = data.iter().map(std::slice::from_ref).collect::<Vec<_>>();
    assert_eq!(validate(envelope, index as u64, &bytes), expected);
    for split in 0..=data.len() {
        assert_eq!(
            validate(
                envelope,
                index as u64,
                &[b"", &data[..split], b"", &data[split..], b""]
            ),
            expected
        );
    }
    if let Some(lf) = data.iter().position(|byte| *byte == b'\n') {
        for split in [lf, lf + 1] {
            assert_eq!(
                validate(envelope, index as u64, &[&data[..split], &data[split..]]),
                expected
            );
        }
    }
}

#[test]
fn committed_nonempty_shards_validate_for_all_schedules() {
    for index in 0..2 {
        let (envelope, data) = native_case(index);
        assert_schedules(&envelope, 0, &data);
    }
    for (envelope, index, data) in multimonth_cases() {
        assert_schedules(&envelope, index, &data);
    }
}

#[test]
fn receipt_is_owned_and_public_surface_is_callable() {
    fn owned_receipt() -> ValidatedBodyShard {
        let (envelope, data) = native_case(0);
        validate(&envelope, 0, &[&data])
    }
    fn api(envelope: &BodyEnvelope) -> Result<ValidatedBodyShard, EnvelopeError> {
        let mut validator = BodyShardValidator::new(envelope, 0)?;
        let pushed: Result<(), EnvelopeError> = validator.push(b"");
        pushed?;
        validator.finish()
    }
    fn traits<T: Clone + std::fmt::Debug + PartialEq + Eq>() {}

    traits::<ValidatedBodyShard>();
    let _: fn(&BodyEnvelope) -> Result<ValidatedBodyShard, EnvelopeError> = api;
    let _: fn(&ValidatedBodyShard) -> &BundleId = ValidatedBodyShard::bundle_id;
    let receipt = owned_receipt();
    assert_eq!(receipt, receipt.clone());

    let (oura, oura_data) = native_case(1);
    let oura_receipt = validate(&oura, 0, &[&oura_data[..17], &oura_data[17..]]);
    assert_ne!(receipt, oura_receipt);
}

fn empty_envelope() -> BodyEnvelope {
    decode_body_envelope(
        native_bundle_fixture()["cases"][2]["expected_envelope_jsonl"]
            .as_str()
            .unwrap()
            .as_bytes(),
    )
    .unwrap()
}

fn assert_error(
    error: &EnvelopeError,
    code: EnvelopeErrorCode,
    field: EnvelopeErrorField,
    index: u64,
) {
    assert!(error.bundle().is_some());
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
    assert_eq!(error.index(), Some(index));
}

#[test]
fn constructor_refuses_every_out_of_range_boundary() {
    let (one, _) = native_case(0);
    for index in [one.shards().len() as u64, u64::MAX] {
        let error = BodyShardValidator::new(&one, index)
            .err()
            .expect("index refuses");
        assert_error(
            &error,
            EnvelopeErrorCode::InvalidField,
            EnvelopeErrorField::Shards,
            index,
        );
    }
    let empty = empty_envelope();
    let error = BodyShardValidator::new(&empty, 0)
        .err()
        .expect("empty bundle refuses index zero");
    assert_error(
        &error,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::Shards,
        0,
    );
}

fn envelope_for(data: &[u8], declared_rows: u64, digest: BodyDigest) -> BodyEnvelope {
    let (source, _) = native_case(1);
    let shard = EnvelopeShard::new(
        source.bundle_id(),
        0,
        source.shards()[0].month().clone(),
        data.len() as u64,
        declared_rows,
        digest,
    )
    .expect("test shard descriptor is intrinsically valid");
    BodyEnvelope::new(
        source.bundle_id().clone(),
        source.source_family(),
        source.source_hash().clone(),
        source.raw_retention(),
        declared_rows,
        source.days().to_vec(),
        vec![shard],
        EnvelopeLedger::new(
            source.bundle_id(),
            source.ledger().bytes().max(declared_rows),
            declared_rows,
            source.ledger().sha256().clone(),
        )
        .unwrap(),
        source.summary_plan().cloned(),
    )
    .expect("test envelope is checked")
}

fn matching_envelope(data: &[u8], rows: u64) -> BodyEnvelope {
    envelope_for(data, rows, sha256_body_digest(data))
}

#[test]
fn inventory_semantics_and_error_precedence_are_exact() {
    for data in [b"\n".as_slice(), b"\nunterminated".as_slice()] {
        let envelope = matching_envelope(data, 1);
        validate(&envelope, 0, &[data]);
    }

    let exact = b"x\n";
    let envelope = matching_envelope(exact, 1);
    let mut byte_overrun = BodyShardValidator::new(&envelope, 0).unwrap();
    let error = byte_overrun.push(b"\n\n\n").unwrap_err();
    assert_error(
        &error,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::ShardBytes,
        0,
    );

    let row_data = b"\n\n";
    let row_envelope = envelope_for(row_data, 1, sha256_body_digest(row_data));
    let mut row_overrun = BodyShardValidator::new(&row_envelope, 0).unwrap();
    let error = row_overrun.push(row_data).unwrap_err();
    assert_error(
        &error,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::ShardRows,
        0,
    );

    let short_bytes = BodyShardValidator::new(&envelope, 0)
        .unwrap()
        .finish()
        .unwrap_err();
    assert_error(
        &short_bytes,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::ShardBytes,
        0,
    );

    let no_lf = b"xx";
    let short_rows_envelope = matching_envelope(no_lf, 1);
    let mut short_rows = BodyShardValidator::new(&short_rows_envelope, 0).unwrap();
    short_rows.push(no_lf).unwrap();
    let error = short_rows.finish().unwrap_err();
    assert_error(
        &error,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::ShardRows,
        0,
    );

    let wrong = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .unwrap();
    let digest_envelope = envelope_for(exact, 1, wrong);
    let mut digest = BodyShardValidator::new(&digest_envelope, 0).unwrap();
    digest.push(exact).unwrap();
    let error = digest.finish().unwrap_err();
    assert_error(
        &error,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::ShardSha256,
        0,
    );
}

#[test]
fn first_push_error_poison_is_stable() {
    let data = b"x\n";
    let envelope = matching_envelope(data, 1);
    let mut validator = BodyShardValidator::new(&envelope, 0).unwrap();
    let first = validator.push(b"too long\n").unwrap_err();
    assert_eq!(validator.push(b"").unwrap_err(), first);
    assert_eq!(validator.push(b"different\n\n").unwrap_err(), first);
    assert_eq!(validator.finish().unwrap_err(), first);

    let row_data = b"\n\n";
    let row_envelope = envelope_for(row_data, 1, sha256_body_digest(row_data));
    let mut rows = BodyShardValidator::new(&row_envelope, 0).unwrap();
    let first = rows.push(row_data).unwrap_err();
    assert_eq!(rows.push(b"later").unwrap_err(), first);
    assert_eq!(rows.finish().unwrap_err(), first);
}

#[test]
fn prefixes_and_adversarial_streams_never_panic() {
    let mut shards = (0..2)
        .map(|index| {
            let (envelope, data) = native_case(index);
            (envelope, 0_usize, data)
        })
        .collect::<Vec<_>>();
    shards.extend(multimonth_cases());

    for (envelope, index, data) in shards {
        for end in 0..=data.len() {
            for split in [0, end / 2, end] {
                assert!(
                    std::panic::catch_unwind(|| {
                        let mut validator =
                            BodyShardValidator::new(&envelope, index as u64).unwrap();
                        let _ = validator.push(&data[..split]);
                        let _ = validator.push(&data[split..end]);
                        let _ = validator.finish();
                    })
                    .is_ok()
                );
            }
        }
    }
    let (envelope, _) = native_case(0);
    for input in [
        b"\0\xff\n".as_slice(),
        b"\n\n\n".as_slice(),
        b"no-newline".as_slice(),
    ] {
        assert!(
            std::panic::catch_unwind(|| {
                let mut validator = BodyShardValidator::new(&envelope, 0).unwrap();
                let _ = validator.push(input);
                let _ = validator.finish();
            })
            .is_ok()
        );
    }
}

#[test]
fn maximum_coordinate_errors_are_bounded_ascii_and_redacted() {
    let envelope = empty_envelope();
    let error = BodyShardValidator::new(&envelope, u64::MAX).err().unwrap();
    for rendered in [format!("{error}"), format!("{error:?}")] {
        assert!(rendered.is_ascii());
        assert!(rendered.len() <= 256);
        assert!(!rendered.contains("raw-owner-sentinel"));
        assert!(rendered.contains(u64::MAX.to_string().as_str()));
    }
}

#[test]
fn independently_valid_digest_and_descriptor_twins_differ() {
    let data = b"x\n";
    let first = matching_envelope(data, 1);
    let second_data = b"y\n";
    let second = matching_envelope(second_data, 1);
    let first_receipt = validate(&first, 0, &[data]);
    let second_receipt = validate(&second, 0, &[second_data]);
    assert_ne!(first_receipt, second_receipt);
    assert_ne!(first_receipt.sha256(), second_receipt.sha256());

    let recomputed = format!("sha256:{:x}", Sha256::digest(data));
    assert_eq!(first_receipt.sha256().as_str(), recomputed);
}

fn with_bundle(source: &BodyEnvelope, bundle_id: BundleId) -> BodyEnvelope {
    BodyEnvelope::new(
        bundle_id.clone(),
        source.source_family(),
        source.source_hash().clone(),
        source.raw_retention(),
        source.row_count(),
        source.days().to_vec(),
        source.shards().to_vec(),
        EnvelopeLedger::new(
            &bundle_id,
            source.ledger().bytes(),
            source.ledger().events(),
            source.ledger().sha256().clone(),
        )
        .unwrap(),
        source.summary_plan().cloned(),
    )
    .unwrap()
}

fn oura_envelope_with(
    source: &BodyEnvelope,
    row_count: u64,
    days: Vec<BodyDay>,
    shards: Vec<EnvelopeShard>,
) -> BodyEnvelope {
    BodyEnvelope::new(
        source.bundle_id().clone(),
        source.source_family(),
        source.source_hash().clone(),
        source.raw_retention(),
        row_count,
        days,
        shards,
        EnvelopeLedger::new(
            source.bundle_id(),
            source.ledger().bytes().max(row_count),
            row_count,
            source.ledger().sha256().clone(),
        )
        .unwrap(),
        None,
    )
    .unwrap()
}

#[test]
fn receipt_equality_observes_bundle_and_index_independently() {
    let (source, data) = native_case(1);
    let rebound = with_bundle(
        &source,
        BundleId::from_bytes(b"body-00000000000000000000000000").unwrap(),
    );
    let original = validate(&source, 0, &[&data]);
    let other_bundle = validate(&rebound, 0, &[&data]);
    assert_eq!(original.descriptor(), other_bundle.descriptor());
    assert_eq!(original.index(), other_bundle.index());
    assert_ne!(original.bundle_id(), other_bundle.bundle_id());
    assert_ne!(original, other_bundle);

    let january = BodyDay::from_bytes(b"20260102").unwrap();
    let february = BodyDay::from_bytes(b"20260201").unwrap();
    let first_data = b"first\n";
    let target_data = b"target\n";
    let first = EnvelopeShard::new(
        source.bundle_id(),
        0,
        BodyMonth::from_bytes(b"2026-01").unwrap(),
        first_data.len() as u64,
        1,
        sha256_body_digest(first_data),
    )
    .unwrap();
    let target = EnvelopeShard::new(
        source.bundle_id(),
        0,
        BodyMonth::from_bytes(b"2026-02").unwrap(),
        target_data.len() as u64,
        1,
        sha256_body_digest(target_data),
    )
    .unwrap();
    let at_zero = oura_envelope_with(&source, 1, vec![february.clone()], vec![target.clone()]);
    let at_one = oura_envelope_with(&source, 2, vec![january, february], vec![first, target]);
    let zero = validate(&at_zero, 0, &[target_data]);
    let one = validate(&at_one, 1, &[target_data]);
    assert_eq!(zero.bundle_id(), one.bundle_id());
    assert_eq!(zero.descriptor(), one.descriptor());
    assert_ne!(zero.index(), one.index());
    assert_ne!(zero, one);
}

#[test]
fn maximum_constructible_counters_are_overflow_safe() {
    let (source, _) = native_case(1);
    let shard = EnvelopeShard::new(
        source.bundle_id(),
        0,
        source.shards()[0].month().clone(),
        u64::MAX,
        u64::MAX,
        source.shards()[0].sha256().clone(),
    )
    .unwrap();
    let envelope = oura_envelope_with(&source, u64::MAX, source.days().to_vec(), vec![shard]);
    let mut validator = BodyShardValidator::new(&envelope, 0).unwrap();
    validator
        .push(b"x")
        .expect("one byte is within the maximum");
    let error = validator.finish().unwrap_err();
    assert_error(
        &error,
        EnvelopeErrorCode::CountMismatch,
        EnvelopeErrorField::ShardBytes,
        0,
    );
}
