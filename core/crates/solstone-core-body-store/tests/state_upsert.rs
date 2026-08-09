// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_store::{BodyDedupeDisposition, BodyDedupeState};

mod support;

use support::{observation, set_null, set_text, text};

const APPLE_BUNDLE: &str = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y";
const OURA_BUNDLE: &str = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z";

fn digest(hex: char) -> String {
    format!("sha256:{}", hex.to_string().repeat(64))
}

fn first_observation(key: &str) -> support::Observation {
    observation(
        "apple_retain_complete_one_row",
        APPLE_BUNDLE,
        key,
        &digest('a'),
        |row, _| {
            set_text(row, "source_record_id", text("first-source"));
            set_text(row, "record_type", text("first-type"));
            set_text(row, "start_date", text("first-start"));
            set_text(row, "end_date", text("first-end"));
            set_text(
                row,
                "raw_ref",
                text(&format!("imports/{APPLE_BUNDLE}/raw/first")),
            );
        },
    )
}

fn second_observation(key: &str) -> support::Observation {
    observation(
        "oura_retain_parsed_one_row",
        OURA_BUNDLE,
        key,
        &digest('b'),
        |row, _| {
            set_null(row, "source_record_id");
            set_text(row, "record_type", text("second-type"));
            set_text(row, "start_date", text("second-start"));
            set_null(row, "end_date");
            set_null(row, "raw_ref");
        },
    )
}

#[test]
fn applies_ordered_upsert_rules_with_distinct_bundle_observations() {
    let key = digest('c');
    let first = first_observation(&key).validate();
    let second = second_observation(&key).validate();
    let second_key = second.event().dedupe_key().clone();
    let second_value_hash = second.event().value_hash().clone();
    let second_normalized_ref = second.event().normalized_ref().clone();

    let mut state = BodyDedupeState::new();
    assert_eq!(state.apply(&first), Ok(BodyDedupeDisposition::Inserted));
    assert_eq!(state.apply(&second), Ok(BodyDedupeDisposition::Updated));

    let row = state.get(&second_key).expect("dedupe row exists");
    assert_eq!(row.source_family().as_str(), "apple_health");
    assert_eq!(row.record_type(), "first-type");
    assert_eq!(row.first_import_id(), Some(APPLE_BUNDLE));
    assert_eq!(row.source_record_id(), Some("first-source"));
    assert_eq!(
        row.raw_ref(),
        Some(format!("imports/{APPLE_BUNDLE}/raw/first").as_str())
    );
    assert_eq!(row.start_time(), "second-start");
    assert_eq!(row.end_time(), None);
    assert_eq!(row.latest_import_id(), Some(OURA_BUNDLE));
    assert_eq!(row.value_hash(), Some(&second_value_hash));
    assert_eq!(
        row.normalized_ref()
            .expect("native row has normalized ref")
            .chars()
            .map(u32::from)
            .collect::<Vec<_>>(),
        second_normalized_ref.code_points()
    );
}

#[test]
fn replay_order_not_bundle_identity_determines_the_sticky_observation() {
    let key = digest('d');
    let first = first_observation(&key).validate();
    let second = second_observation(&key).validate();
    let first_value_hash = first.event().value_hash().clone();

    let mut state = BodyDedupeState::new();
    assert_eq!(state.apply(&second), Ok(BodyDedupeDisposition::Inserted));
    assert_eq!(state.apply(&first), Ok(BodyDedupeDisposition::Updated));

    let row = state
        .get(first.event().dedupe_key())
        .expect("dedupe row exists");
    assert_eq!(row.source_family().as_str(), "oura_api");
    assert_eq!(row.record_type(), "second-type");
    assert_eq!(row.first_import_id(), Some(OURA_BUNDLE));
    assert_eq!(row.start_time(), "first-start");
    assert_eq!(row.end_time(), Some("first-end"));
    assert_eq!(row.latest_import_id(), Some(APPLE_BUNDLE));
    assert_eq!(row.value_hash(), Some(&first_value_hash));
}
