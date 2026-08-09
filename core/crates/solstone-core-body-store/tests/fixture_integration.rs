// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_store::{BodyDedupeDisposition, BodyDedupeErrorField, BodyDedupeState};

mod support;

use support::{fixture_observation, observation, set_text};

const _: fn() = || {
    fn assert_copy_eq_hash<T: Copy + Eq + std::hash::Hash>() {}
    assert_copy_eq_hash::<BodyDedupeDisposition>();
    assert_copy_eq_hash::<BodyDedupeErrorField>();
};

#[test]
fn fixture_events_replay_through_the_complete_public_api() {
    let apple_observation = fixture_observation("apple_retain_complete_one_row");
    let apple = apple_observation.validate();
    let apple_key = apple.event().dedupe_key().clone();
    drop(apple_observation);

    let oura_observation = fixture_observation("oura_retain_parsed_one_row");
    let oura = oura_observation.validate();
    let oura_key = oura.event().dedupe_key().clone();
    drop(oura_observation);

    let mut state = BodyDedupeState::new();
    assert_eq!(state.apply(&apple), Ok(BodyDedupeDisposition::Inserted));
    assert_eq!(state.apply(&oura), Ok(BodyDedupeDisposition::Inserted));
    assert_eq!(state.len(), 2);
    assert!(!state.is_empty());

    let apple_row = state.get(&apple_key).expect("Apple row exists");
    assert_eq!(apple_row.dedupe_key(), &apple_key);
    assert_eq!(apple_row.source_family().as_str(), "apple_health");
    assert_eq!(apple_row.source_record_id(), None);
    assert_eq!(apple_row.record_type(), "HKWorkoutActivityTypeRunning");
    assert_eq!(apple_row.start_time(), "2026-01-02 06:30:00 -0700");
    assert_eq!(apple_row.end_time(), Some("2026-01-02 07:15:00 -0700"));
    assert_eq!(apple_row.value_hash(), apple.event().value_hash());
    assert_eq!(apple_row.first_import_id(), apple.event().bundle_id());
    assert_eq!(apple_row.latest_import_id(), apple.event().bundle_id());
    assert_eq!(
        apple_row.normalized_ref(),
        "imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y/normalized/2026-01.jsonl#L1"
    );
    assert_eq!(
        apple_row.raw_ref(),
        Some("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y/raw/apple_health/export.xml#workout-6")
    );

    let oura_row = state.get(&oura_key).expect("Oura row exists");
    assert_eq!(oura_row.dedupe_key(), &oura_key);
    assert_eq!(oura_row.source_family().as_str(), "oura_api");
    assert_eq!(oura_row.source_record_id(), Some("synthetic-readiness-1"));
    assert_eq!(oura_row.record_type(), "oura.daily_readiness");
    assert_eq!(oura_row.start_time(), "2026-01-02");
    assert_eq!(oura_row.end_time(), Some("2026-01-03"));
    assert_eq!(oura_row.value_hash(), oura.event().value_hash());
    assert_eq!(oura_row.first_import_id(), oura.event().bundle_id());
    assert_eq!(oura_row.latest_import_id(), oura.event().bundle_id());
    assert_eq!(
        oura_row.normalized_ref(),
        "imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/normalized/2026-01.jsonl#L1"
    );
    assert_eq!(
        oura_row.raw_ref(),
        Some("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z/raw/oura/daily_readiness-0001.json#item-0")
    );

    let keys: Vec<_> = state.iter().map(|row| row.dedupe_key().clone()).collect();
    assert_eq!(keys, vec![apple_key, oura_key]);
}

#[test]
fn structured_refusal_is_available_after_fixture_replay() {
    let bad = observation(
        "apple_retain_complete_one_row",
        "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Y",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        |row, _| set_text(row, "record_type", [0xd800]),
    );
    let mut state = BodyDedupeState::new();
    assert_eq!(
        state.apply(&bad.validate()).unwrap_err().field(),
        BodyDedupeErrorField::RecordType
    );
}
