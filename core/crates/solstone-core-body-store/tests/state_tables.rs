// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use solstone_core_body_store::{BodyDedupeDisposition, BodyDedupeState};

mod support;

use support::{observation, set_null, set_text, text};

const BUNDLE: &str = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y";

fn digest(hex: char) -> String {
    format!("sha256:{}", hex.to_string().repeat(64))
}

fn apply(
    state: &mut BodyDedupeState,
    key: &str,
    value_hash: char,
    source_record_id: Option<&str>,
    raw_ref: Option<&str>,
    end_time: Option<&str>,
) -> BodyDedupeDisposition {
    let observation = observation(
        "apple_retain_complete_one_row",
        BUNDLE,
        key,
        &digest(value_hash),
        |row, _| {
            match source_record_id {
                Some(value) => set_text(row, "source_record_id", text(value)),
                None => set_null(row, "source_record_id"),
            }
            match raw_ref {
                Some(value) => set_text(
                    row,
                    "raw_ref",
                    text(&format!("imports/{BUNDLE}/raw/{value}")),
                ),
                None => set_null(row, "raw_ref"),
            }
            match end_time {
                Some(value) => set_text(row, "end_date", text(value)),
                None => set_null(row, "end_date"),
            }
        },
    );
    state
        .apply(&observation.validate())
        .expect("apply succeeds")
}

#[test]
fn empty_state_has_no_rows() {
    let state = BodyDedupeState::new();
    assert_eq!(state.len(), 0);
    assert!(state.is_empty());
    assert_eq!(state.iter().next(), None);
}

#[test]
fn table_driven_coalesce_and_clear_rules_match_ordered_upserts() {
    struct Step {
        source_record_id: Option<&'static str>,
        raw_ref: Option<&'static str>,
        end_time: Option<&'static str>,
        expected_source_record_id: Option<&'static str>,
        expected_raw_ref: Option<&'static str>,
        expected_end_time: Option<&'static str>,
    }

    let steps = [
        Step {
            source_record_id: None,
            raw_ref: None,
            end_time: Some("one"),
            expected_source_record_id: None,
            expected_raw_ref: None,
            expected_end_time: Some("one"),
        },
        Step {
            source_record_id: Some("present"),
            raw_ref: Some("present"),
            end_time: None,
            expected_source_record_id: Some("present"),
            expected_raw_ref: Some("present"),
            expected_end_time: None,
        },
        Step {
            source_record_id: None,
            raw_ref: None,
            end_time: Some("three"),
            expected_source_record_id: Some("present"),
            expected_raw_ref: Some("present"),
            expected_end_time: Some("three"),
        },
    ];
    let key = digest('e');
    let mut state = BodyDedupeState::new();

    for (index, step) in steps.iter().enumerate() {
        let disposition = apply(
            &mut state,
            &key,
            char::from_u32(u32::from(b'a') + index as u32).expect("ASCII"),
            step.source_record_id,
            step.raw_ref,
            step.end_time,
        );
        assert_eq!(
            disposition,
            if index == 0 {
                BodyDedupeDisposition::Inserted
            } else {
                BodyDedupeDisposition::Updated
            }
        );
        let row = state
            .get(&solstone_core_body_source::BodyDigest::from_bytes(key.as_bytes()).unwrap())
            .unwrap();
        assert_eq!(row.source_record_id(), step.expected_source_record_id);
        assert_eq!(
            row.raw_ref(),
            step.expected_raw_ref
                .map(|value| format!("imports/{BUNDLE}/raw/{value}"))
                .as_deref()
        );
        assert_eq!(row.end_time(), step.expected_end_time);
    }
}

#[test]
fn identical_validated_event_replay_updates_without_changing_the_row() {
    let observation = observation(
        "apple_retain_complete_one_row",
        BUNDLE,
        &digest('d'),
        &digest('e'),
        |row, _| {
            set_text(row, "source_record_id", text("fixed-source-record"));
            set_text(row, "raw_ref", text(&format!("imports/{BUNDLE}/raw/fixed")));
            set_text(row, "end_date", text("fixed-end-time"));
        },
    );
    let validated = observation.validate();
    let key = validated.event().dedupe_key().clone();
    let mut state = BodyDedupeState::new();

    assert_eq!(state.apply(&validated), Ok(BodyDedupeDisposition::Inserted));
    let after_first = state.get(&key).expect("inserted row exists").clone();

    assert_eq!(state.apply(&validated), Ok(BodyDedupeDisposition::Updated));
    assert_eq!(state.get(&key), Some(&after_first));
    assert_eq!(state.len(), 1);
}

#[test]
fn multiple_keys_iterate_in_digest_order_and_count_revisions() {
    let keys = [digest('f'), digest('a'), digest('c')];
    let mut state = BodyDedupeState::new();
    let mut inserted = 0;
    let mut updated = 0;

    for (index, key) in keys.iter().enumerate() {
        let disposition = apply(
            &mut state,
            key,
            char::from_u32(u32::from(b'a') + index as u32).expect("ASCII"),
            Some("id"),
            Some("raw"),
            Some("end"),
        );
        inserted += usize::from(disposition == BodyDedupeDisposition::Inserted);
        updated += usize::from(disposition == BodyDedupeDisposition::Updated);
    }
    let replay = apply(
        &mut state,
        &keys[0],
        'f',
        Some("id"),
        Some("raw"),
        Some("end"),
    );
    inserted += usize::from(replay == BodyDedupeDisposition::Inserted);
    updated += usize::from(replay == BodyDedupeDisposition::Updated);
    assert_eq!(replay, BodyDedupeDisposition::Updated);

    let expected: BTreeSet<_> = keys.iter().map(String::as_str).collect();
    let actual: Vec<_> = state.iter().map(|row| row.dedupe_key()).collect();
    assert_eq!(actual, expected.into_iter().collect::<Vec<_>>());
    assert_eq!(state.len(), actual.len());
    assert_eq!(inserted + updated, 4);
    assert_eq!(inserted, 3);
    assert_eq!(updated, 1);
}
