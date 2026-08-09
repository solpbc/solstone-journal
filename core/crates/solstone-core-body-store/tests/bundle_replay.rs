// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::Value;
use solstone_core_body_source::{
    BodyEnvelope, BodyRowEventErrorKind, EnvelopeErrorCode, EnvelopeErrorField,
    LedgerEventErrorCode, LedgerEventErrorField, decode_body_envelope,
};
use solstone_core_body_store::{
    BodyBundleReplay, BodyBundleReplayError, BodyBundleReplayErrorKind, BodyDedupeState,
    ValidatedBodyBundleReplay,
};

mod support;

use support::fixture_observation;

struct PhysicalBundle {
    envelope: BodyEnvelope,
    shards: Vec<Vec<u8>>,
    ledger: Vec<u8>,
}

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/fixtures");
    let file = if name == "apple_three_events_two_months_two_shards" {
        "body_source_ledger_events_v1.json"
    } else {
        "body_source_native_bundle_v1.json"
    };
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(path.join(file)).expect("fixture reads"))
            .expect("fixture parses");
    fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"].as_str() == Some(name))
        .expect("fixture case exists")
        .clone()
}

fn physical(name: &str) -> PhysicalBundle {
    let case = fixture(name);
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes())
            .expect("fixture envelope decodes");
    let shards = if let Some(shards) = case["shards"].as_array() {
        shards
            .iter()
            .map(|shard| {
                shard["expected_jsonl"]
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .to_vec()
            })
            .collect()
    } else {
        let row = case["expected_normalized_jsonl"].as_str().unwrap();
        if row.is_empty() {
            Vec::new()
        } else {
            vec![row.as_bytes().to_vec()]
        }
    };
    PhysicalBundle {
        envelope,
        shards,
        ledger: case["expected_ledger_jsonl"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec(),
    }
}

fn frames(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.split_inclusive(|byte| *byte == b'\n').collect()
}

fn replay(bundle: &PhysicalBundle, state: BodyDedupeState) -> ValidatedBodyBundleReplay {
    let ledger = frames(&bundle.ledger);
    let mut ledger_index = 0;
    let mut replay = BodyBundleReplay::with_state(&bundle.envelope, state).unwrap();
    for (shard_index, shard) in bundle.shards.iter().enumerate() {
        for row in frames(shard) {
            replay
                .push(shard_index as u64, row, ledger[ledger_index])
                .expect("fixture pair replays");
            ledger_index += 1;
        }
    }
    assert_eq!(ledger_index, ledger.len());
    replay.finish().expect("fixture replay finishes")
}

#[test]
fn every_native_shape_replays_without_skips_or_partial_state() {
    let mut state = BodyDedupeState::new();
    for name in [
        "apple_retain_complete_one_row",
        "oura_retain_parsed_one_row",
        "apple_discard_zero_rows",
        "oura_discard_zero_rows",
        "apple_three_events_two_months_two_shards",
    ] {
        let bundle = physical(name);
        let before = state.len();
        let validated = replay(&bundle, state);
        assert_eq!(validated.bundle_id(), bundle.envelope.bundle_id());
        assert_eq!(validated.event_count(), bundle.envelope.row_count());
        assert_eq!(validated.shards().len(), bundle.envelope.shards().len());
        assert_eq!(validated.ledger().events(), bundle.envelope.row_count());
        assert_eq!(validated.ledger().bytes(), bundle.envelope.ledger().bytes());
        for (index, shard) in validated.shards().iter().enumerate() {
            assert_eq!(shard.index(), index as u64);
            assert_eq!(shard.descriptor(), &bundle.envelope.shards()[index]);
        }
        assert_eq!(
            validated.state().len(),
            before + bundle.envelope.row_count() as usize
        );
        state = validated.into_state();
        assert_eq!(state.len(), before + bundle.envelope.row_count() as usize);
    }
    assert_eq!(state.len(), 5);
}

type FramePair<'a> = (&'a [u8], &'a [u8]);

fn first_two(bundle: &PhysicalBundle) -> (FramePair<'_>, FramePair<'_>) {
    let rows = frames(&bundle.shards[0]);
    let ledger = frames(&bundle.ledger);
    ((rows[0], ledger[0]), (rows[1], ledger[1]))
}

#[test]
fn skip_duplicate_reorder_and_wrong_shard_all_fail_closed() {
    let bundle = physical("apple_three_events_two_months_two_shards");
    let ((row1, event1), (row2, event2)) = first_two(&bundle);

    let mut skipped = BodyBundleReplay::new(&bundle.envelope).unwrap();
    let error = skipped.push(0, row2, event2).unwrap_err();
    assert_eq!(error.kind(), BodyBundleReplayErrorKind::Ledger);
    let BodyBundleReplayError::Ledger(error) = error else {
        unreachable!()
    };
    assert_eq!(error.code(), LedgerEventErrorCode::InvalidSequence);
    assert_eq!(error.field(), LedgerEventErrorField::Sequence);
    assert_eq!(error.line(), 1);

    let mut duplicate = BodyBundleReplay::new(&bundle.envelope).unwrap();
    duplicate.push(0, row1, event1).unwrap();
    let error = duplicate.push(0, row1, event1).unwrap_err();
    assert_eq!(error.kind(), BodyBundleReplayErrorKind::Ledger);

    let mut wrong_shard = BodyBundleReplay::new(&bundle.envelope).unwrap();
    let error = wrong_shard.push(1, row1, event1).unwrap_err();
    assert_eq!(error.kind(), BodyBundleReplayErrorKind::Location);
    assert_eq!(error.bundle(), Some(bundle.envelope.bundle_id()));
    assert_eq!(error.sequence(), Some(1));

    let other = physical("oura_retain_parsed_one_row");
    let other_event = frames(&other.ledger)[0];
    let mut cross_bundle = BodyBundleReplay::new(&bundle.envelope).unwrap();
    let error = cross_bundle.push(0, row1, other_event).unwrap_err();
    let BodyBundleReplayError::Ledger(error) = error else {
        panic!("cross-bundle event must fail at ledger validation")
    };
    assert_eq!(error.code(), LedgerEventErrorCode::ReferenceMismatch);
    assert_eq!(error.field(), LedgerEventErrorField::BundleId);

    let rows = bundle
        .shards
        .iter()
        .flat_map(|shard| frames(shard))
        .collect::<Vec<_>>();
    let events = frames(&bundle.ledger);
    let mut complete = BodyBundleReplay::new(&bundle.envelope).unwrap();
    complete.push(0, rows[0], events[0]).unwrap();
    complete.push(0, rows[1], events[1]).unwrap();
    complete.push(1, rows[2], events[2]).unwrap();
    let error = complete.push(1, rows[2], events[2]).unwrap_err();
    assert_eq!(error.kind(), BodyBundleReplayErrorKind::Location);
    assert_eq!(error.sequence(), Some(4));
}

#[test]
fn every_physical_byte_must_be_one_exact_row_frame() {
    let bundle = physical("apple_retain_complete_one_row");
    let row = frames(&bundle.shards[0])[0];
    let event = frames(&bundle.ledger)[0];

    for bad in [b"\n".as_slice(), b"unterminated".as_slice()] {
        let mut replay = BodyBundleReplay::new(&bundle.envelope).unwrap();
        let error = replay.push(0, bad, event).unwrap_err();
        let BodyBundleReplayError::Row(error) = error else {
            panic!("bad row framing must fail at row validation")
        };
        assert_eq!(error.kind(), &BodyRowEventErrorKind::InvalidFraming);
    }

    let mut trailing = row.to_vec();
    trailing.extend_from_slice(b"trailing-owner-sentinel");
    let mut replay = BodyBundleReplay::new(&bundle.envelope).unwrap();
    let error = replay.push(0, &trailing, event).unwrap_err();
    assert_eq!(error.kind(), BodyBundleReplayErrorKind::Row);
    assert!(!format!("{error:?}").contains("trailing-owner-sentinel"));

    let replay = BodyBundleReplay::new(&bundle.envelope).unwrap();
    let error = replay.finish().unwrap_err();
    let BodyBundleReplayError::Envelope(error) = error else {
        panic!("missing shard bytes must fail inventory validation")
    };
    assert_eq!(error.code(), EnvelopeErrorCode::CountMismatch);
    assert_eq!(error.field(), EnvelopeErrorField::ShardBytes);
    assert_eq!(error.index(), Some(0));
}

#[test]
fn poison_replays_the_first_error_and_never_returns_state() {
    let bundle = physical("apple_three_events_two_months_two_shards");
    let ((row1, event1), _) = first_two(&bundle);
    let mut replay = BodyBundleReplay::new(&bundle.envelope).unwrap();
    let first = replay.push(1, row1, event1).unwrap_err();
    assert_eq!(
        replay.push(0, b"different\n", b"different\n").unwrap_err(),
        first
    );
    assert_eq!(replay.finish().unwrap_err(), first);
}

#[test]
fn successful_bundle_returns_prior_state_and_failure_returns_no_state() {
    let observation = fixture_observation("apple_retain_complete_one_row");
    let validated = observation.validate();
    let key = validated.event().dedupe_key().clone();
    let mut prior = BodyDedupeState::new();
    prior.apply(&validated).unwrap();

    let bundle = physical("oura_retain_parsed_one_row");
    let validated_bundle = replay(&bundle, prior);
    let returned = validated_bundle.into_state();
    assert_eq!(returned.len(), 2);
    assert!(returned.get(&key).is_some());

    let failed = BodyBundleReplay::with_state(&bundle.envelope, returned)
        .unwrap()
        .finish();
    assert!(failed.is_err());
}

#[test]
fn public_error_surface_is_bounded_redacted_and_source_aware() {
    use std::error::Error;

    let bundle = physical("apple_three_events_two_months_two_shards");
    let ((row, event), _) = first_two(&bundle);
    let mut replay = BodyBundleReplay::new(&bundle.envelope).unwrap();
    let error = replay.push(1, row, event).unwrap_err();
    assert_eq!(error.kind().as_str(), "location");
    assert!(Error::source(&error).is_none());
    for rendered in [format!("{error}"), format!("{error:?}")] {
        assert!(rendered.is_ascii());
        assert!(rendered.len() <= 256);
        assert!(!rendered.contains("owner"));
    }

    let mut replay = BodyBundleReplay::new(&bundle.envelope).unwrap();
    let error = replay.push(0, b"bad\n", event).unwrap_err();
    assert_eq!(error.kind(), BodyBundleReplayErrorKind::Row);
    assert!(Error::source(&error).is_some());
}
