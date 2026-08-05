// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::MalformedPolicy;

use crate::{
    IdentityMapLoserReason, PreparedHistoryOutcome, classify_prepared_history,
    guard_restore_does_not_cross_merge, guard_visible_event_collision,
    load_resolved_ambiguity_choice, read_ambiguities, read_entity_identity, read_identity_map,
    read_prepared_history, read_visible_history,
};

const ENTITY_STORE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_store.json"
));

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn identity_artifacts_round_trip_with_effective_ids() {
    let fixture = fixture();
    let temporary = TempDir::new();

    write_text(
        temporary.path(),
        "entities/alice_johnson/entity.json",
        artifact(&fixture, "entities/{id}/entity.json"),
    );
    write_text(
        temporary.path(),
        "entities/jose_garcia/entity.json",
        artifact(&fixture, "entities/{id}/entity.json (unicode)"),
    );

    let alice = read_entity_identity(temporary.path(), "alice_johnson")
        .unwrap()
        .expect("identity exists");
    assert_eq!(alice.entity_id(), "alice_johnson");
    assert!(alice.was_written());
    assert_eq!(alice.value()["name"], "Alice Johnson");

    let jose = read_entity_identity(temporary.path(), "jose_garcia")
        .unwrap()
        .expect("identity exists");
    assert_eq!(jose.entity_id(), "jose_garcia");
    assert!(jose.was_written());
    assert_eq!(jose.value()["name"], "José García");

    write_json(
        temporary.path(),
        "entities/directory_id/entity.json",
        &json!({"id": "written_id", "name": "Written identity"}),
    );
    let written = read_entity_identity(temporary.path(), "directory_id")
        .unwrap()
        .expect("identity exists");
    assert_eq!(written.entity_id(), "written_id");
    assert!(written.was_written());
    assert_eq!(written.value()["id"], "written_id");
}

#[test]
fn identity_map_distinguishes_written_self_id_from_directory_fallback() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/gamma/entity.json",
        &json!({"id": "gamma", "name": "Gamma"}),
    );
    write_json(
        temporary.path(),
        "entities/other/entity.json",
        &json!({"id": "gamma", "name": "Other"}),
    );
    write_json(
        temporary.path(),
        "entities/fallback/entity.json",
        &json!({"name": "Fallback"}),
    );

    let gamma = read_entity_identity(temporary.path(), "gamma")
        .unwrap()
        .expect("gamma identity exists");
    let fallback = read_entity_identity(temporary.path(), "fallback")
        .unwrap()
        .expect("fallback identity exists");
    assert!(gamma.was_written());
    assert!(!fallback.was_written());

    let map = read_identity_map(temporary.path()).unwrap();
    assert_eq!(map.resolved.get("gamma"), Some(&"gamma".to_owned()));
    assert_eq!(map.resolved.get("fallback"), Some(&"fallback".to_owned()));
    assert!(map.losers.iter().any(|loser| {
        loser.entity_dir == "other" && loser.reason == IdentityMapLoserReason::CollisionLost
    }));
}

#[test]
fn prepared_history_is_discovered_from_staging_directory() {
    let fixture = fixture();
    let temporary = TempDir::new();
    write_text(
        temporary.path(),
        "entities/alice_johnson/history/prepared/vh_staged/event.json",
        artifact(
            &fixture,
            "entities/{id}/history/events/{seq}-{version_id}.json",
        ),
    );

    let prepared = read_prepared_history(temporary.path(), "alice_johnson").unwrap();
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].staging_id, "vh_staged");
    assert_eq!(
        prepared[0].event.version_id().unwrap(),
        "vh_49d7adcbf786461cb11c00081afa9780"
    );
}

#[test]
fn visible_history_sorts_by_filename_after_reverse_writes() {
    let temporary = TempDir::new();
    let first = history_event(1, "vh_first", "create");
    let second = history_event(2, "vh_second", "update");
    let write_order = [
        ("00000000000000000002-vh_second.json", second),
        ("00000000000000000001-vh_first.json", first),
    ];
    assert!(
        write_order[0].0 > write_order[1].0,
        "fixture is reverse-written"
    );
    for (filename, event) in write_order {
        write_json(
            temporary.path(),
            &format!("entities/alice_johnson/history/events/{filename}"),
            &event,
        );
    }

    let events = read_visible_history(temporary.path(), "alice_johnson").unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn history_read_edges_parse_boolean_sequences_and_variable_width_timestamps() {
    let fixture = fixture();
    let temporary = TempDir::new();
    write_text(
        temporary.path(),
        "entities/alice_johnson/history/events/00000000000000000001-vh_bool.json",
        fixture["read_edges"]["history_event_with_boolean_sequence"]["bytes"]
            .as_str()
            .unwrap(),
    );
    write_text(
        temporary.path(),
        "entities/alice_johnson/history/events/00000000000000000002-vh_timestamp.json",
        fixture["read_edges"]["history_event_without_fractional_seconds"]["bytes"]
            .as_str()
            .unwrap(),
    );

    let events = read_visible_history(temporary.path(), "alice_johnson").unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].sequence().unwrap(), 1);
    assert_eq!(events[1].value()["ts"], "2026-08-05T00:32:02Z");
}

#[test]
fn zero_byte_identity_is_absent_and_discards_create_event() {
    let fixture = fixture();
    let temporary = TempDir::new();
    write_bytes(temporary.path(), "entities/alice_johnson/entity.json", b"");
    write_json(
        temporary.path(),
        "entities/alice_johnson/history/prepared/vh_create/event.json",
        &fixture["inputs"]["history_event"],
    );

    let identity = read_entity_identity(temporary.path(), "alice_johnson").unwrap();
    assert_eq!(identity, None);
    let prepared = read_prepared_history(temporary.path(), "alice_johnson").unwrap();
    assert_eq!(
        classify_prepared_history("alice_johnson", &prepared[0].event, identity.as_ref()).unwrap(),
        PreparedHistoryOutcome::Discard
    );
}

#[test]
fn history_guards_preserve_required_messages() {
    let temporary = TempDir::new();

    let mut mismatch = history_event(1, "vh_mismatch", "create");
    mismatch["entity_id"] = Value::String("other".to_owned());
    write_json(
        temporary.path(),
        "entities/alice_johnson/history/prepared/vh_mismatch/event.json",
        &mismatch,
    );
    let mismatch_event =
        &read_prepared_history(temporary.path(), "alice_johnson").unwrap()[0].event;
    assert_eq!(
        classify_prepared_history("alice_johnson", mismatch_event, None)
            .unwrap_err()
            .to_string(),
        "prepared history for alice_johnson contains event for other"
    );

    let mut collision_a = history_event(1, "vh_collision", "create");
    collision_a["identity_after"] = json!({"created_at": 1});
    let mut collision_b = history_event(1, "vh_collision", "create");
    collision_b["identity_after"] = json!({"created_at": 1.0});
    let mut collision_c = history_event(1, "vh_collision", "create");
    collision_c["caller"] = Value::String("different".to_owned());
    for (filename, event) in [
        ("00000000000000000001-vh_collision_a.json", collision_a),
        ("00000000000000000002-vh_collision_b.json", collision_b),
        ("00000000000000000003-vh_collision_c.json", collision_c),
    ] {
        write_json(
            temporary.path(),
            &format!("entities/alice_johnson/history/events/{filename}"),
            &event,
        );
    }
    let collision_events = read_visible_history(temporary.path(), "alice_johnson").unwrap();
    guard_visible_event_collision(
        "alice_johnson",
        &collision_events[1],
        Some(&collision_events[0]),
    )
    .unwrap();
    assert_eq!(
        guard_visible_event_collision(
            "alice_johnson",
            &collision_events[2],
            Some(&collision_events[0]),
        )
        .unwrap_err()
        .to_string(),
        "visible history event collision for alice_johnson: 00000000000000000001-vh_collision.json"
    );

    let mut invalid_sequence_event = history_event(1, "vh_bad_seq", "create");
    invalid_sequence_event["seq"] = json!(1.5);
    write_json(
        temporary.path(),
        "entities/invalid_seq/history/events/00000000000000000001-vh_bad_seq.json",
        &invalid_sequence_event,
    );
    let invalid_sequence = &read_visible_history(temporary.path(), "invalid_seq").unwrap()[0];
    assert_eq!(
        invalid_sequence.sequence().unwrap_err().to_string(),
        "history event seq must be an integer"
    );

    write_json(
        temporary.path(),
        "entities/invalid_version/history/events/00000000000000000001-bad.json",
        &history_event(1, "bad", "create"),
    );
    let invalid_version = &read_visible_history(temporary.path(), "invalid_version").unwrap()[0];
    assert_eq!(
        invalid_version.version_id().unwrap_err().to_string(),
        "history event has an invalid version_id"
    );

    let merge = history_event(1, "vh_merge", "merge");
    write_json(
        temporary.path(),
        "entities/restore/history/events/00000000000000000001-vh_merge.json",
        &merge,
    );
    let merge_events = read_visible_history(temporary.path(), "restore").unwrap();
    assert_eq!(
        guard_restore_does_not_cross_merge(&merge_events[0], &merge_events)
            .unwrap_err()
            .to_string(),
        "generic identity restore cannot target a recorded merge event; use recorded-merge undo instead"
    );

    write_json(
        temporary.path(),
        "entities/restore_later/history/events/00000000000000000001-vh_target.json",
        &history_event(1, "vh_target", "create"),
    );
    write_json(
        temporary.path(),
        "entities/restore_later/history/events/00000000000000000002-vh_later_merge.json",
        &history_event(2, "vh_later_merge", "merge_undo"),
    );
    let later_events = read_visible_history(temporary.path(), "restore_later").unwrap();
    assert_eq!(
        guard_restore_does_not_cross_merge(&later_events[0], &later_events)
            .unwrap_err()
            .to_string(),
        "generic identity restore cannot cross a recorded merge event; use recorded-merge undo instead"
    );

    write_text(temporary.path(), "entities/not_object/entity.json", "[]");
    assert_eq!(
        read_entity_identity(temporary.path(), "not_object")
            .unwrap_err()
            .to_string(),
        format!(
            "entity identity is not an object: {}",
            temporary
                .path()
                .join("entities/not_object/entity.json")
                .display()
        )
    );

    write_text(
        temporary.path(),
        "entities/not_event/history/events/00000000000000000001-vh_array.json",
        "[]",
    );
    assert_eq!(
        read_visible_history(temporary.path(), "not_event")
            .unwrap_err()
            .to_string(),
        format!(
            "history event is not an object: {}",
            temporary
                .path()
                .join("entities/not_event/history/events/00000000000000000001-vh_array.json")
                .display()
        )
    );
}

#[test]
fn reconciliation_fixture_cases_match_recorded_outcomes() {
    let fixture = fixture();
    let cases = fixture["reconciliation"]["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        fixture["reconciliation"]["case_count"].as_u64().unwrap() as usize
    );
    assert!(
        fixture["reconciliation"]["absent_by_design"]
            .as_str()
            .unwrap()
            .contains("There is no key-order case")
    );

    for case in cases {
        let temporary = TempDir::new();
        let entity_dir = case["entity_dir"].as_str().unwrap_or("alice_johnson");
        write_json(
            temporary.path(),
            &format!("entities/{entity_dir}/entity.json"),
            &case["disk"],
        );
        let mut event = fixture["inputs"]["history_event"].clone();
        event["identity_before"] = case["before"].clone();
        event["identity_after"] = case["after"].clone();
        event["entity_id"] = Value::String(entity_dir.to_owned());
        write_json(
            temporary.path(),
            &format!("entities/{entity_dir}/history/prepared/vh_case/event.json"),
            &event,
        );

        let identity = read_entity_identity(temporary.path(), entity_dir).unwrap();
        let prepared = read_prepared_history(temporary.path(), entity_dir).unwrap();
        let actual =
            classify_prepared_history(entity_dir, &prepared[0].event, identity.as_ref()).unwrap();
        let expected = match case["outcome"].as_str().unwrap() {
            "publish" => PreparedHistoryOutcome::Publish,
            "discard" => PreparedHistoryOutcome::Discard,
            "repair_required" => PreparedHistoryOutcome::RepairRequired,
            outcome => panic!("unknown fixture outcome: {outcome}"),
        };
        assert_eq!(actual, expected, "{}", case["note"]);
    }
}

#[test]
fn numeric_history_comparison_preserves_integer_float_boundaries() {
    let boundary = TempDir::new();
    let mut integer = history_event(1, "vh_boundary", "update");
    integer["identity_after"] = json!({"created_at": i64::MAX});
    let mut rounded_float = history_event(1, "vh_boundary", "update");
    rounded_float["identity_after"] = json!({"created_at": 9223372036854775808.0});
    write_json(
        boundary.path(),
        "entities/alice_johnson/history/events/00000000000000000001-a.json",
        &integer,
    );
    write_json(
        boundary.path(),
        "entities/alice_johnson/history/events/00000000000000000001-b.json",
        &rounded_float,
    );
    let events = read_visible_history(boundary.path(), "alice_johnson").unwrap();
    assert!(guard_visible_event_collision("alice_johnson", &events[1], Some(&events[0])).is_err());

    let normal = TempDir::new();
    let mut integer = history_event(1, "vh_normal", "update");
    integer["identity_after"] = json!({"created_at": 1785889922582_i64});
    let mut float = history_event(1, "vh_normal", "update");
    float["identity_after"] = json!({"created_at": 1785889922582.0});
    write_json(
        normal.path(),
        "entities/alice_johnson/history/events/00000000000000000001-a.json",
        &integer,
    );
    write_json(
        normal.path(),
        "entities/alice_johnson/history/events/00000000000000000001-b.json",
        &float,
    );
    let events = read_visible_history(normal.path(), "alice_johnson").unwrap();
    guard_visible_event_collision("alice_johnson", &events[1], Some(&events[0])).unwrap();
}

#[test]
fn ambiguity_fixture_rows_obey_strict_validation_in_order() {
    let fixture = fixture();
    let rows = fixture["negative"]["ambiguity_rows"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        fixture["negative"]["row_count"].as_u64().unwrap() as usize
    );

    for fixture_row in rows {
        let temporary = TempDir::new();
        write_json(
            temporary.path(),
            "entities/ambiguities.jsonl",
            &fixture_row["row"],
        );
        assert_ambiguity_refusal(
            temporary.path(),
            fixture_row["refusal"].as_str().unwrap(),
            1,
        );
    }
}

#[test]
fn ambiguity_reader_distinguishes_malformed_non_object_and_lenient_rows() {
    let fixture = fixture();
    let temporary = TempDir::new();
    let invalid_row = &fixture["negative"]["ambiguity_rows"][0]["row"];
    let contents = format!(
        "\nnot json\n[1, 2, 3]\n{}\n",
        serde_json::to_string(invalid_row).unwrap()
    );
    write_text(temporary.path(), "entities/ambiguities.jsonl", &contents);

    assert_ambiguity_refusal(temporary.path(), "malformed JSON", 2);

    write_text(
        temporary.path(),
        "entities/ambiguities.jsonl",
        "[1, 2, 3]\n",
    );
    assert_ambiguity_refusal(temporary.path(), "expected object, got list", 1);

    write_json(temporary.path(), "entities/ambiguities.jsonl", invalid_row);
    let lenient = read_ambiguities(temporary.path(), MalformedPolicy::Skip).unwrap();
    assert_eq!(lenient, vec![invalid_row.clone()]);
}

#[test]
fn zero_byte_ambiguities_are_empty_under_strict_reading() {
    let temporary = TempDir::new();
    write_bytes(temporary.path(), "entities/ambiguities.jsonl", b"");
    assert!(
        read_ambiguities(temporary.path(), MalformedPolicy::Raise)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn ambiguity_reader_keeps_u2028_inside_a_single_jsonl_row() {
    let fixture = fixture();
    let temporary = TempDir::new();
    let mut row = fixture["inputs"]["ambiguity_rows"][0].clone();
    row["latest_query"] = Value::String("Alice\u{2028}Example".to_owned());
    write_text(
        temporary.path(),
        "entities/ambiguities.jsonl",
        &format!("{}\n", serde_json::to_string(&row).unwrap()),
    );
    let rows = read_ambiguities(temporary.path(), MalformedPolicy::Raise).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["latest_query"], "Alice\u{2028}Example");
}

#[test]
fn resolved_ambiguity_choice_is_always_strict() {
    let fixture = fixture();
    let temporary = TempDir::new();
    write_text(
        temporary.path(),
        "entities/ambiguities.jsonl",
        artifact(&fixture, "entities/ambiguities.jsonl"),
    );
    let scope = json!({"kind": "facet", "facet": "work"});
    let choice = load_resolved_ambiguity_choice(temporary.path(), &scope, "strasse")
        .unwrap()
        .expect("fixture contains a resolved choice");
    assert_eq!(choice["resolved_entity_id"], "strasse_handels_gmbh");

    write_text(temporary.path(), "entities/ambiguities.jsonl", "not json\n");
    assert_ambiguity_refusal(temporary.path(), "malformed JSON", 1);
    assert!(load_resolved_ambiguity_choice(temporary.path(), &scope, "strasse").is_err());
}

#[test]
fn ambiguity_missing_last_seen_is_rejected() {
    let mut row = valid_ambiguity_row();
    row.as_object_mut().unwrap().remove("last_seen");
    assert_manual_ambiguity_refusal(row, "missing last_seen");
}

#[test]
fn ambiguity_resolved_without_timestamp_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["status"] = json!("resolved");
    row["resolved_entity_id"] = json!("alice_chen");
    assert_manual_ambiguity_refusal(row, "resolved row has no timestamp");
}

#[test]
fn ambiguity_non_object_ranked_candidate_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["ranked_candidates"] = json!(["not an object"]);
    assert_manual_ambiguity_refusal(row, "ranked candidate is not an object");
}

#[test]
fn ambiguity_non_list_origins_or_keys_are_rejected() {
    let mut row = valid_ambiguity_row();
    row["origins"] = json!({});
    assert_manual_ambiguity_refusal(row, "origins/origin_keys is not a list");
}

#[test]
fn ambiguity_non_object_origin_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["origins"] = json!(["not an object"]);
    assert_manual_ambiguity_refusal(row, "origin is not an object");
}

#[test]
fn ambiguity_origin_with_non_string_value_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["origins"][0]["rank"] = json!(1);
    assert_manual_ambiguity_refusal(row, "origin contains a non-string value");
}

#[test]
fn ambiguity_invalid_origin_key_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["origin_keys"] = json!([""]);
    assert_manual_ambiguity_refusal(row, "origin_keys contains an invalid key");
}

#[test]
fn ambiguity_non_object_prior_choice_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["audit"]["prior_choices"] = json!(["not an object"]);
    assert_manual_ambiguity_refusal(row, "prior choice is not an object");
}

#[test]
fn ambiguity_prior_without_resolved_at_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["audit"]["prior_choices"] = json!([valid_prior_choice()]);
    row["audit"]["prior_choices"][0]
        .as_object_mut()
        .unwrap()
        .remove("resolved_at");
    assert_manual_ambiguity_refusal(row, "prior choice has no resolved_at");
}

#[test]
fn ambiguity_prior_without_replaced_at_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["audit"]["prior_choices"] = json!([valid_prior_choice()]);
    row["audit"]["prior_choices"][0]
        .as_object_mut()
        .unwrap()
        .remove("replaced_at");
    assert_manual_ambiguity_refusal(row, "prior choice has no replaced_at");
}

#[test]
fn ambiguity_invalid_prior_choice_origin_is_rejected() {
    let mut row = valid_ambiguity_row();
    row["audit"]["prior_choices"] = json!([valid_prior_choice()]);
    row["audit"]["prior_choices"][0]["replaced_by_origin"] = json!({"lane": ""});
    assert_manual_ambiguity_refusal(row, "invalid prior-choice origin");
}

#[test]
fn identity_map_fixture_cases_match_recorded_results() {
    let fixture = fixture();
    let cases = fixture["identity_map"]["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        fixture["identity_map"]["case_count"].as_u64().unwrap() as usize
    );

    for case in cases {
        let temporary = TempDir::new();
        let store = case["store"].as_object().unwrap();
        for (entity_dir, record) in store {
            let relative = format!("entities/{entity_dir}/entity.json");
            match record.as_str() {
                Some(raw) => write_text(temporary.path(), &relative, raw),
                None => write_json(temporary.path(), &relative, record),
            }
        }

        let map = read_identity_map(temporary.path()).unwrap();
        let expected = case["resolves"].as_object().unwrap();
        assert_eq!(map.resolved.len(), expected.len(), "{}", case["note"]);
        for (identity_id, directory) in expected {
            assert_eq!(
                map.resolved.get(identity_id),
                Some(&directory.as_str().unwrap().to_owned()),
                "{}",
                case["note"]
            );
        }
        if let Some(identity_ids) = case["does_not_resolve"].as_array() {
            for identity_id in identity_ids {
                assert!(!map.resolved.contains_key(identity_id.as_str().unwrap()));
            }
        }
        if let Some(entry_count) = case.get("entry_count").and_then(Value::as_u64) {
            assert_eq!(map.resolved.len(), entry_count as usize);
        }
        if let Some(store_entity_count) = case.get("store_entity_count").and_then(Value::as_u64) {
            assert_eq!(store.len(), store_entity_count as usize);
        }
        if let Some(loser) = case.get("loser").and_then(Value::as_str) {
            let loser = map
                .losers
                .iter()
                .find(|actual| actual.entity_dir == loser)
                .expect("fixture loser is returned");
            if loser.entity_dir == "broken" {
                assert!(matches!(
                    loser.reason,
                    IdentityMapLoserReason::Malformed { .. }
                ));
            } else {
                assert_eq!(loser.reason, IdentityMapLoserReason::CollisionLost);
            }
        }
    }
}

#[test]
fn public_readers_leave_the_fixture_tree_unchanged() {
    let fixture = fixture();
    let temporary = TempDir::new();
    write_text(
        temporary.path(),
        "entities/alice_johnson/entity.json",
        artifact(&fixture, "entities/{id}/entity.json"),
    );
    write_text(
        temporary.path(),
        "entities/alice_johnson/history/events/00000000000000000001-vh_fixture.json",
        artifact(
            &fixture,
            "entities/{id}/history/events/{seq}-{version_id}.json",
        ),
    );
    write_text(
        temporary.path(),
        "entities/alice_johnson/history/prepared/vh_fixture/event.json",
        artifact(
            &fixture,
            "entities/{id}/history/events/{seq}-{version_id}.json",
        ),
    );
    write_text(
        temporary.path(),
        "entities/ambiguities.jsonl",
        artifact(&fixture, "entities/ambiguities.jsonl"),
    );
    write_text(
        temporary.path(),
        "entities/ambiguities.jsonl.lock",
        "lock sidecar",
    );

    let before = tree_hash(temporary.path());
    assert!(
        read_entity_identity(temporary.path(), "alice_johnson")
            .unwrap()
            .is_some()
    );
    assert!(
        !read_visible_history(temporary.path(), "alice_johnson")
            .unwrap()
            .is_empty()
    );
    assert!(
        !read_prepared_history(temporary.path(), "alice_johnson")
            .unwrap()
            .is_empty()
    );
    assert!(
        !read_ambiguities(temporary.path(), MalformedPolicy::Raise)
            .unwrap()
            .is_empty()
    );
    assert!(
        !read_ambiguities(temporary.path(), MalformedPolicy::Skip)
            .unwrap()
            .is_empty()
    );
    assert!(
        !read_identity_map(temporary.path())
            .unwrap()
            .resolved
            .is_empty()
    );
    assert_eq!(tree_hash(temporary.path()), before);
}

fn fixture() -> Value {
    serde_json::from_str(ENTITY_STORE_FIXTURE).unwrap()
}

fn artifact<'a>(fixture: &'a Value, name: &str) -> &'a str {
    fixture["artifacts"][name].as_str().unwrap()
}

fn history_event(sequence: i64, version_id: &str, kind: &str) -> Value {
    json!({
        "schema_version": 1,
        "version_id": version_id,
        "seq": sequence,
        "ts": "2026-08-05T00:32:02Z",
        "entity_id": "alice_johnson",
        "kind": kind,
        "caller": null,
        "actor": null,
        "identity_before": null,
        "identity_after": {"id": "alice_johnson"},
        "operation": {},
    })
}

fn valid_ambiguity_row() -> Value {
    fixture()["inputs"]["ambiguity_rows"][0].clone()
}

fn valid_prior_choice() -> Value {
    json!({
        "resolved_entity_id": "alice_chen",
        "resolved_at": "2026-08-04T00:00:00Z",
        "replaced_at": "2026-08-05T00:00:00Z",
    })
}

fn assert_manual_ambiguity_refusal(row: Value, expected_detail: &str) {
    let temporary = TempDir::new();
    write_json(temporary.path(), "entities/ambiguities.jsonl", &row);
    assert_ambiguity_refusal(temporary.path(), expected_detail, 1);
}

fn assert_ambiguity_refusal(root: &Path, expected_detail: &str, line: usize) {
    let error = read_ambiguities(root, MalformedPolicy::Raise)
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        format!(
            "entity ambiguities: invalid row {line} in {}: {expected_detail}",
            root.join("entities/ambiguities.jsonl").display()
        )
    );
}

fn tree_hash(root: &Path) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hash_tree_entry(root, root, &mut hasher);
    hasher.finalize().into()
}

fn hash_tree_entry(root: &Path, path: &Path, hasher: &mut Sha256) {
    let metadata = fs::symlink_metadata(path).unwrap();
    let relative = path.strip_prefix(root).unwrap();
    let kind = if metadata.file_type().is_symlink() {
        b"symlink".as_slice()
    } else if metadata.is_dir() {
        b"directory".as_slice()
    } else {
        b"file".as_slice()
    };
    hasher.update([0]);
    hasher.update(kind);
    hasher.update([0]);
    hasher.update(relative.as_os_str().as_encoded_bytes());
    hasher.update([0]);

    if metadata.file_type().is_symlink() {
        hasher.update(fs::read_link(path).unwrap().as_os_str().as_encoded_bytes());
    } else if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            hash_tree_entry(root, &entry, hasher);
        }
    } else {
        hasher.update(fs::read(path).unwrap());
    }
}

fn write_text(root: &Path, relative: &str, contents: &str) {
    write_bytes(root, relative, contents.as_bytes());
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    write_bytes(root, relative, &serde_json::to_vec(value).unwrap());
}

fn write_bytes(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}
