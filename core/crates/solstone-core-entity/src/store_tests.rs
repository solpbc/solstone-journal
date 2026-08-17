// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    LockError, LockOptions, MalformedPolicy, contained_path, hold_lock,
};

use crate::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation, EntityIdentityRepairError,
    EntityIdentityRepairGuard, EntityIdentityRepairSkipReason, EntityWriteError,
    IdentityMapLoserReason, PreparedHistoryOutcome, ambiguity_id, classify_prepared_history,
    guard_restore_does_not_cross_merge, guard_visible_event_collision, load_all_journal_entities,
    load_resolved_ambiguity_choice, read_ambiguities, read_entity_identity, read_identity_map,
    read_prepared_history, read_visible_history, record_ambiguity_choice,
    record_ambiguity_observation, refresh_identity_map_cache, repair_entity_identities,
    rescope_facet_ambiguities, save_entity_identity, save_entity_identity_with_timeout,
    set_forced_identity_write_failure, set_repair_identity_write_failure_on_attempt,
    write_history_event_json_for_test,
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
        Self {
            path: fs::canonicalize(path).unwrap(),
        }
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
fn identity_group_map_retains_every_collision_candidate_in_precedence_order() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/written_z/entity.json",
        &json!({"id": "shared"}),
    );
    write_json(
        temporary.path(),
        "entities/written_a/entity.json",
        &json!({"id": "shared"}),
    );
    write_json(temporary.path(), "entities/shared/entity.json", &json!({}));

    let groups = crate::read_identity_group_map(temporary.path()).unwrap();

    assert_eq!(
        groups.groups.get("shared"),
        Some(&vec![
            "written_a".to_owned(),
            "written_z".to_owned(),
            "shared".to_owned(),
        ])
    );
    assert!(groups.losers.is_empty());
    let resolved = read_identity_map(temporary.path()).unwrap();
    assert_eq!(
        resolved.resolved.get("shared"),
        Some(&"written_a".to_owned())
    );
    assert_eq!(
        resolved
            .losers
            .iter()
            .filter(|loser| loser.reason == IdentityMapLoserReason::CollisionLost)
            .map(|loser| loser.entity_dir.as_str())
            .collect::<Vec<_>>(),
        vec!["shared", "written_z"]
    );
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
fn identity_writer_runs_every_reconciliation_fixture_case() {
    let fixture = fixture();
    let cases = fixture["reconciliation"]["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        fixture["reconciliation"]["case_count"].as_u64().unwrap() as usize
    );

    for case in cases {
        let temporary = TempDir::new();
        let entity_dir = case["entity_dir"].as_str().unwrap_or("alice_johnson");
        let mut disk = case["disk"].clone();
        if disk.get("id").is_none() {
            disk["id"] = Value::String(entity_dir.to_owned());
        }
        write_json(
            temporary.path(),
            &format!("entities/{entity_dir}/entity.json"),
            &disk,
        );
        let mut event = fixture["inputs"]["history_event"].clone();
        event["entity_id"] = Value::String(entity_dir.to_owned());
        event["identity_before"] = case["before"].clone();
        event["identity_after"] = case["after"].clone();
        write_json(
            temporary.path(),
            &format!("entities/{entity_dir}/history/prepared/vh_case/event.json"),
            &event,
        );

        let outcome = case["outcome"].as_str().unwrap();
        let result = save_entity_identity(temporary.path(), entity_dir, &disk, None);
        match outcome {
            "publish" => {
                assert!(!result.unwrap().changed, "{}", case["note"]);
                assert_eq!(
                    read_visible_history(temporary.path(), entity_dir)
                        .unwrap()
                        .len(),
                    1
                );
                assert!(
                    read_prepared_history(temporary.path(), entity_dir)
                        .unwrap()
                        .is_empty()
                );
            }
            "discard" => {
                assert!(!result.unwrap().changed, "{}", case["note"]);
                assert!(
                    read_visible_history(temporary.path(), entity_dir)
                        .unwrap()
                        .is_empty()
                );
                assert!(
                    read_prepared_history(temporary.path(), entity_dir)
                        .unwrap()
                        .is_empty()
                );
            }
            "repair_required" => {
                assert!(matches!(
                    result,
                    Err(EntityWriteError::ReconciliationRepairRequired { .. })
                ));
                assert_eq!(
                    read_prepared_history(temporary.path(), entity_dir)
                        .unwrap()
                        .len(),
                    1
                );
            }
            unexpected => panic!("unexpected reconciliation outcome: {unexpected}"),
        }
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
fn ambiguity_rescope_updates_a_facet_scope_and_its_identifier() {
    let temporary = TempDir::new();
    let mut row = valid_ambiguity_row();
    row["scope"] = json!({"kind": "facet", "facet": "old"});
    let normalized_query = row["normalized_query"].as_str().unwrap().to_owned();
    row["ambiguity_id"] = json!(ambiguity_id(&format!("facet:old|{normalized_query}")));
    write_json(temporary.path(), "entities/ambiguities.jsonl", &row);

    let report = rescope_facet_ambiguities(temporary.path(), "old", "new").unwrap();
    let rows = read_ambiguities(temporary.path(), MalformedPolicy::Raise).unwrap();

    assert_eq!(report.rewritten_ambiguity_ids.len(), 1);
    assert_eq!(rows[0]["scope"]["facet"], "new");
    assert_eq!(
        rows[0]["ambiguity_id"],
        ambiguity_id(&format!("facet:new|{normalized_query}"))
    );
}

#[test]
fn ambiguity_rescope_updates_facet_origins_without_changing_journal_scope() {
    let temporary = TempDir::new();
    let mut row = valid_ambiguity_row();
    row["origins"] = json!([{
        "lane": "facet",
        "facet": "old",
        "path": "facets/old/entities/alice/entity.json"
    }]);
    row["origin_keys"] = json!(["stale"]);
    write_json(temporary.path(), "entities/ambiguities.jsonl", &row);

    let report = rescope_facet_ambiguities(temporary.path(), "old", "new").unwrap();
    let rows = read_ambiguities(temporary.path(), MalformedPolicy::Raise).unwrap();

    assert_eq!(
        report.rewritten_ambiguity_ids,
        vec![row["ambiguity_id"].as_str().unwrap().to_owned()]
    );
    assert_eq!(rows[0]["scope"]["kind"], "journal");
    assert!(rows[0]["scope"].get("facet").is_none());
    assert_eq!(rows[0]["origins"][0]["facet"], "new");
    assert_eq!(
        rows[0]["origins"][0]["path"],
        "facets/new/entities/alice/entity.json"
    );
    assert_eq!(
        rows[0]["origin_keys"][0],
        "{\"facet\":\"new\",\"lane\":\"facet\",\"path\":\"facets/new/entities/alice/entity.json\"}"
    );
}

#[test]
fn ambiguity_rescope_updates_prior_choice_replacement_origins() {
    let temporary = TempDir::new();
    let mut row = valid_ambiguity_row();
    row["audit"]["prior_choices"] = json!([{
        "resolved_entity_id": "alice_chen",
        "resolved_at": "2026-08-04T00:00:00Z",
        "replaced_at": "2026-08-05T00:00:00Z",
        "replaced_by_origin": {
            "lane": "facet",
            "facet": "old",
            "path": "facets/old/entities/alice/entity.json"
        }
    }]);
    write_json(temporary.path(), "entities/ambiguities.jsonl", &row);

    let report = rescope_facet_ambiguities(temporary.path(), "old", "new").unwrap();
    let rows = read_ambiguities(temporary.path(), MalformedPolicy::Raise).unwrap();

    assert_eq!(
        report.rewritten_ambiguity_ids,
        vec![row["ambiguity_id"].as_str().unwrap().to_owned()]
    );
    assert_eq!(
        rows[0]["audit"]["prior_choices"][0]["replaced_by_origin"]["facet"],
        "new"
    );
    assert_eq!(
        rows[0]["audit"]["prior_choices"][0]["replaced_by_origin"]["path"],
        "facets/new/entities/alice/entity.json"
    );
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

#[test]
fn identity_writer_addresses_the_effective_identity_not_the_directory_name() {
    let fixture = fixture();
    let case = &fixture["identity_map"]["cases"][3];
    let temporary = TempDir::new();
    for (directory, record) in case["store"].as_object().unwrap() {
        write_json(
            temporary.path(),
            &format!("entities/{directory}/entity.json"),
            record,
        );
    }
    let mut identity = case["store"]["alpha"].clone();
    identity["name"] = json!("Alpha updated");

    let result = save_entity_identity(temporary.path(), "beta", &identity, None).unwrap();

    assert_eq!(result.entity_dir, "alpha");
    assert_eq!(
        read_entity_identity(temporary.path(), "alpha")
            .unwrap()
            .unwrap()
            .value()["name"],
        "Alpha updated"
    );
    assert_eq!(
        read_entity_identity(temporary.path(), "beta")
            .unwrap()
            .unwrap()
            .value()["name"],
        "Beta"
    );
    let event = result.event.unwrap();
    assert_eq!(event["entity_id"], "alpha");
    assert_eq!(event["identity_after"]["id"], "beta");
}

#[test]
fn identity_writer_preserves_identity_artifact_bytes_and_updates_cache() {
    let fixture = fixture();
    let temporary = TempDir::new();
    let identity: Value =
        serde_json::from_str(artifact(&fixture, "entities/{id}/entity.json")).unwrap();

    let result = save_entity_identity(temporary.path(), "alice_johnson", &identity, None).unwrap();

    assert!(result.changed);
    assert_eq!(
        fs::read(temporary.path().join("entities/alice_johnson/entity.json")).unwrap(),
        artifact(&fixture, "entities/{id}/entity.json").as_bytes()
    );
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(
            &fs::metadata(temporary.path().join("entities/alice_johnson/entity.json"))
                .unwrap()
                .permissions()
        ) & 0o777,
        0o600
    );
    let cache = refresh_identity_map_cache(temporary.path()).unwrap();
    assert!(!cache.rebuilt);
    assert_eq!(
        cache.resolved.get("alice_johnson"),
        Some(&"alice_johnson".to_owned())
    );
}

#[test]
fn identity_writer_preserves_unicode_identity_artifact_bytes() {
    let fixture = fixture();
    let temporary = TempDir::new();
    let identity: Value =
        serde_json::from_str(artifact(&fixture, "entities/{id}/entity.json (unicode)")).unwrap();

    save_entity_identity(temporary.path(), "jose_garcia", &identity, None).unwrap();

    assert_eq!(
        fs::read(temporary.path().join("entities/jose_garcia/entity.json")).unwrap(),
        artifact(&fixture, "entities/{id}/entity.json (unicode)").as_bytes()
    );
}

#[test]
fn history_writer_serializes_the_history_artifact_byte_exactly() {
    let fixture = fixture();
    let temporary = TempDir::new();
    let event: Value = serde_json::from_str(artifact(
        &fixture,
        "entities/{id}/history/events/{seq}-{version_id}.json",
    ))
    .unwrap();
    let path = temporary.path().join("event.json");

    write_history_event_json_for_test(&path, &event).unwrap();

    assert_eq!(
        fs::read(path).unwrap(),
        artifact(
            &fixture,
            "entities/{id}/history/events/{seq}-{version_id}.json"
        )
        .as_bytes()
    );
}

#[test]
fn identity_writer_stamps_the_addressed_id_and_refuses_to_clobber_a_create_destination() {
    let temporary = TempDir::new();
    let payload = json!({"name": "Alice"});

    save_entity_identity(temporary.path(), "alice", &payload, None).unwrap();
    let written = read_entity_identity(temporary.path(), "alice")
        .unwrap()
        .expect("writer created identity");
    assert_eq!(written.value()["id"], "alice");

    write_text(
        temporary.path(),
        "entities/different/entity.json",
        "not valid JSON\n",
    );
    let before = fs::read(temporary.path().join("entities/different/entity.json")).unwrap();
    let error = save_entity_identity(
        temporary.path(),
        "different",
        &json!({"id": "different", "name": "Different"}),
        None,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        EntityWriteError::CreateDestinationOccupied { .. }
    ));
    assert_eq!(
        fs::read(temporary.path().join("entities/different/entity.json")).unwrap(),
        before
    );
}

#[cfg(unix)]
#[test]
fn identity_writer_refuses_a_dangling_create_destination() {
    let temporary = TempDir::new();
    let directory = temporary.path().join("entities/dangling");
    fs::create_dir_all(&directory).unwrap();
    let destination = directory.join("entity.json");
    symlink(directory.join("missing-target.json"), &destination).unwrap();

    let error = save_entity_identity(
        temporary.path(),
        "dangling",
        &json!({"id": "dangling", "name": "Dangling"}),
        None,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        EntityWriteError::CreateDestinationOccupied { .. }
    ));
    assert!(
        fs::symlink_metadata(destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn identity_writer_noop_leaves_identity_history_and_cache_unchanged() {
    let temporary = TempDir::new();
    let payload = json!({"id": "alice", "name": "Alice"});
    save_entity_identity(temporary.path(), "alice", &payload, None).unwrap();
    let identity_path = temporary.path().join("entities/alice/entity.json");
    let cache_path = temporary.path().join("entities/.identity-map-cache.json");
    let identity_before = fs::read(&identity_path).unwrap();
    let cache_before = fs::read(&cache_path).unwrap();
    let events_before = fs::read_dir(temporary.path().join("entities/alice/history/events"))
        .unwrap()
        .count();

    let result = save_entity_identity(temporary.path(), "alice", &payload, None).unwrap();

    assert!(!result.changed);
    assert!(result.event.is_none());
    assert_eq!(fs::read(identity_path).unwrap(), identity_before);
    assert_eq!(fs::read(cache_path).unwrap(), cache_before);
    assert_eq!(
        fs::read_dir(temporary.path().join("entities/alice/history/events"))
            .unwrap()
            .count(),
        events_before
    );
}

#[test]
fn identity_writer_reconciles_every_recorded_crash_boundary_before_mutating() {
    let fixture = fixture();
    let cases = fixture["crash_boundaries"]["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        fixture["crash_boundaries"]["case_count"].as_u64().unwrap() as usize
    );

    // Keep this divergent so directory reconstruction cannot satisfy the case.
    let entity_dir = "alpha";
    let identity_id = "beta";
    let before = json!({"id": identity_id, "name": "Before"});
    let after = json!({"id": identity_id, "name": "After"});
    for case in cases {
        let temporary = TempDir::new();
        let on_disk = if case["identity_on_disk"] == "before" {
            &before
        } else {
            &after
        };
        write_json(
            temporary.path(),
            &format!("entities/{entity_dir}/entity.json"),
            on_disk,
        );
        if case["staged_events"].as_u64() == Some(1) {
            let mut event = history_event(1, "vh_crash", "update");
            event["entity_id"] = Value::String(entity_dir.to_owned());
            event["identity_before"] = before.clone();
            event["identity_after"] = after.clone();
            write_json(
                temporary.path(),
                &format!("entities/{entity_dir}/history/prepared/vh_crash/event.json"),
                &event,
            );
        }
        if case["staged_events"].as_u64() == Some(0)
            && case["visible_events_after"].as_u64().unwrap_or(0) > 0
        {
            let mut event = history_event(1, "vh_crash", "update");
            event["entity_id"] = Value::String(entity_dir.to_owned());
            event["identity_before"] = before.clone();
            event["identity_after"] = after.clone();
            write_json(
                temporary.path(),
                &format!("entities/{entity_dir}/history/events/00000000000000000001-vh_crash.json"),
                &event,
            );
        }

        let requested = if case["change_survives"] == true {
            &after
        } else {
            &before
        };
        let result = save_entity_identity(temporary.path(), identity_id, requested, None).unwrap();

        assert!(!result.changed, "{}", case["note"]);
        assert_eq!(
            read_visible_history(temporary.path(), entity_dir)
                .unwrap()
                .len(),
            case["visible_events_after"].as_u64().unwrap() as usize,
            "{}",
            case["note"]
        );
        assert!(
            read_prepared_history(temporary.path(), entity_dir)
                .unwrap()
                .is_empty(),
            "{}",
            case["note"]
        );
    }
}

#[test]
fn identity_writer_uses_the_reader_sequence_accessor() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/alice/entity.json",
        &json!({"id": "alice", "name": "Before"}),
    );
    let mut boolean_sequence = history_event(1, "vh_boolean", "create");
    boolean_sequence["seq"] = Value::Bool(true);
    write_json(
        temporary.path(),
        "entities/alice/history/events/00000000000000000001-vh_boolean.json",
        &boolean_sequence,
    );
    let result = save_entity_identity(
        temporary.path(),
        "alice",
        &json!({"id": "alice", "name": "After"}),
        None,
    )
    .unwrap();
    assert_eq!(result.event.unwrap()["seq"], 2);
}

#[test]
fn identity_write_failure_cannot_publish_a_visible_event_first() {
    let temporary = TempDir::new();
    set_forced_identity_write_failure(true);

    let result = save_entity_identity(
        temporary.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice"}),
        None,
    );
    set_forced_identity_write_failure(false);
    assert!(result.is_err());
    assert!(
        read_visible_history(temporary.path(), "alice")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        read_prepared_history(temporary.path(), "alice")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn prepared_publish_is_idempotent_and_keeps_staging_on_visible_collision() {
    let identity = json!({"id": "alice", "name": "After"});
    let mut event = history_event(1, "vh_same", "update");
    event["entity_id"] = json!("alice");
    event["identity_before"] = json!({"id": "alice", "name": "Before"});
    event["identity_after"] = identity.clone();

    let identical = TempDir::new();
    write_json(identical.path(), "entities/alice/entity.json", &identity);
    write_json(
        identical.path(),
        "entities/alice/history/prepared/vh_same/event.json",
        &event,
    );
    write_json(
        identical.path(),
        "entities/alice/history/events/00000000000000000001-vh_same.json",
        &event,
    );
    let visible_before = fs::read(
        identical
            .path()
            .join("entities/alice/history/events/00000000000000000001-vh_same.json"),
    )
    .unwrap();
    assert!(
        !save_entity_identity(identical.path(), "alice", &identity, None)
            .unwrap()
            .changed
    );
    assert!(
        read_prepared_history(identical.path(), "alice")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read(
            identical
                .path()
                .join("entities/alice/history/events/00000000000000000001-vh_same.json"),
        )
        .unwrap(),
        visible_before
    );

    let collision = TempDir::new();
    write_json(collision.path(), "entities/alice/entity.json", &identity);
    write_json(
        collision.path(),
        "entities/alice/history/prepared/vh_same/event.json",
        &event,
    );
    let mut different = event.clone();
    different["caller"] = json!("different");
    write_json(
        collision.path(),
        "entities/alice/history/events/00000000000000000001-vh_same.json",
        &different,
    );
    let staged_path = collision
        .path()
        .join("entities/alice/history/prepared/vh_same/event.json");
    let staged_before = fs::read(&staged_path).unwrap();
    assert!(matches!(
        save_entity_identity(collision.path(), "alice", &identity, None),
        Err(EntityWriteError::Read(_))
    ));
    assert_eq!(fs::read(staged_path).unwrap(), staged_before);
}

#[test]
fn reconciliation_refusal_keeps_only_the_expected_partial_reconciliation() {
    let actual = TempDir::new();
    let expected = TempDir::new();
    let current = json!({"id": "alice", "name": "After"});
    let before = json!({"id": "alice", "name": "Before"});
    let unrelated = json!({"id": "alice", "name": "Unrelated"});
    let mut publish = history_event(1, "vh_publish", "update");
    publish["entity_id"] = json!("alice");
    publish["identity_before"] = before;
    publish["identity_after"] = current.clone();
    let mut repair = history_event(2, "vh_repair", "update");
    repair["entity_id"] = json!("alice");
    repair["identity_before"] = json!({"id": "alice", "name": "Else"});
    repair["identity_after"] = unrelated;

    write_json(actual.path(), "entities/alice/entity.json", &current);
    write_json(
        actual.path(),
        "entities/alice/history/prepared/vh_a/event.json",
        &publish,
    );
    write_json(
        actual.path(),
        "entities/alice/history/prepared/vh_b/event.json",
        &repair,
    );
    write_json(expected.path(), "entities/alice/entity.json", &current);
    write_history_event_json_for_test(
        &expected
            .path()
            .join("entities/alice/history/events/00000000000000000001-vh_publish.json"),
        &publish,
    )
    .unwrap();
    write_json(
        expected.path(),
        "entities/alice/history/prepared/vh_b/event.json",
        &repair,
    );
    fs::create_dir_all(expected.path().join("health/locks")).unwrap();

    assert!(matches!(
        save_entity_identity(actual.path(), "alice", &current, None),
        Err(EntityWriteError::ReconciliationRepairRequired { .. })
    ));
    assert_eq!(
        tree_hash_excluding_locks(actual.path()),
        tree_hash_excluding_locks(expected.path())
    );
}

#[test]
fn identity_writer_trust_lock_timeout_refuses_without_writing() {
    let temporary = TempDir::new();
    let lock_path = contained_path(temporary.path(), "health/locks/entity-trust").unwrap();
    let held = hold_lock(&lock_path, LockOptions::default()).unwrap();
    let options = LockOptions {
        timeout: Duration::from_millis(50),
        ..LockOptions::default()
    };

    let error = save_entity_identity_with_timeout(
        temporary.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice"}),
        None,
        options,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        EntityWriteError::TrustLock(crate::EntityTrustLockError::Lock(LockError::Timeout(_)))
    ));
    assert!(!temporary.path().join("entities/alice/entity.json").exists());
    drop(held);
}

#[test]
fn ambiguity_writer_preserves_python_default_artifact_bytes_on_a_duplicate_observation() {
    let fixture = fixture();
    let temporary = TempDir::new();
    write_text(
        temporary.path(),
        "entities/ambiguities.jsonl",
        artifact(&fixture, "entities/ambiguities.jsonl"),
    );
    let artifact_rows = artifact(&fixture, "entities/ambiguities.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let input = &artifact_rows[0];
    let observation = AmbiguityObservation {
        scope: input["scope"].clone(),
        query: input["original_query"].as_str().unwrap().to_owned(),
        normalized_query: input["normalized_query"].as_str().unwrap().to_owned(),
        observed_tier: input["observed_tier"].as_i64().unwrap(),
        ranked_candidates: input["ranked_candidates"].as_array().unwrap().clone(),
        origin: input["origins"][0].clone(),
    };

    record_ambiguity_observation(temporary.path(), &observation).unwrap();

    assert_eq!(
        fs::read_to_string(temporary.path().join("entities/ambiguities.jsonl")).unwrap(),
        artifact(&fixture, "entities/ambiguities.jsonl")
    );
}

#[test]
fn ambiguity_writer_ascii_escapes_sorted_non_ascii_origin_keys() {
    let temporary = TempDir::new();
    let observation = AmbiguityObservation {
        scope: json!({"kind": "facet", "facet": "work"}),
        query: "Straße".to_owned(),
        normalized_query: "strasse".to_owned(),
        observed_tier: 8,
        ranked_candidates: vec![json!({
            "id": "strasse_handels_gmbh",
            "name": "Straße Handels GmbH",
            "tier": 8,
            "score": 90.0,
        })],
        origin: json!({"source_id": "Straße Verlag", "lane": "import", "field": "author"}),
    };

    let row = record_ambiguity_observation(temporary.path(), &observation).unwrap();

    assert_eq!(
        row["origin_keys"][0],
        "{\"field\":\"author\",\"lane\":\"import\",\"source_id\":\"Stra\\u00dfe Verlag\"}"
    );
}

#[test]
fn ambiguity_writers_validate_before_writing_and_record_changed_choices() {
    let fixture = fixture();
    let temporary = TempDir::new();
    let artifact_rows = artifact(&fixture, "entities/ambiguities.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let mut invalid_observation = AmbiguityObservation {
        scope: json!({"kind": "journal"}),
        query: "Alice".to_owned(),
        normalized_query: "alice".to_owned(),
        observed_tier: 5,
        ranked_candidates: vec![json!({"id": "alice", "name": "Alice", "tier": 4, "score": 1})],
        origin: json!({"lane": "segment", "day": "20260804", "segment_id": "s1"}),
    };
    write_text(
        temporary.path(),
        "entities/ambiguities.jsonl",
        artifact(&fixture, "entities/ambiguities.jsonl"),
    );
    let before = fs::read(temporary.path().join("entities/ambiguities.jsonl")).unwrap();
    assert!(record_ambiguity_observation(temporary.path(), &invalid_observation).is_err());
    assert_eq!(
        fs::read(temporary.path().join("entities/ambiguities.jsonl")).unwrap(),
        before
    );

    invalid_observation.scope = artifact_rows[1]["scope"].clone();
    invalid_observation.query = "Straße".to_owned();
    invalid_observation.normalized_query = "strasse".to_owned();
    invalid_observation.observed_tier = 8;
    invalid_observation.ranked_candidates = artifact_rows[1]["ranked_candidates"]
        .as_array()
        .unwrap()
        .clone();
    invalid_observation.origin = artifact_rows[1]["origins"][0].clone();
    record_ambiguity_observation(temporary.path(), &invalid_observation).unwrap();

    let request = AmbiguityChoiceRequest {
        scope: artifact_rows[1]["scope"].clone(),
        query: "Straße".to_owned(),
        entity_id: "alice_chen".to_owned(),
        origin: Some(json!({"lane": "manual"})),
    };
    let updated = record_ambiguity_choice(
        temporary.path(),
        &request,
        &[
            AmbiguityChoiceEntity {
                id: "alice_chen".to_owned(),
                blocked: false,
            },
            AmbiguityChoiceEntity {
                id: "strasse_handels_gmbh".to_owned(),
                blocked: false,
            },
        ],
    )
    .unwrap();
    assert_eq!(updated["resolved_entity_id"], "alice_chen");
    assert_eq!(
        updated["audit"]["prior_choices"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn ambiguity_writer_refuses_all_validator_rules_not_covered_by_the_corpus() {
    let rules = [
        "missing_last_seen",
        "resolved_without_timestamp",
        "non_object_candidate",
        "origins_not_list",
        "non_object_origin",
        "origin_non_string_value",
        "invalid_origin_key",
        "non_object_prior_choice",
        "prior_without_resolved_at",
        "prior_without_replaced_at",
        "invalid_prior_origin",
    ];
    assert_eq!(rules.len(), 11);

    for rule in rules {
        let temporary = TempDir::new();
        let mut row = valid_ambiguity_row();
        match rule {
            "missing_last_seen" => {
                row.as_object_mut().unwrap().remove("last_seen");
            }
            "resolved_without_timestamp" => {
                row["status"] = json!("resolved");
                row["resolved_entity_id"] = json!("alice_chen");
            }
            "non_object_candidate" => row["ranked_candidates"] = json!(["not an object"]),
            "origins_not_list" => row["origins"] = json!({}),
            "non_object_origin" => row["origins"] = json!(["not an object"]),
            "origin_non_string_value" => row["origins"][0]["rank"] = json!(1),
            "invalid_origin_key" => row["origin_keys"] = json!([""]),
            "non_object_prior_choice" => row["audit"]["prior_choices"] = json!(["bad"]),
            "prior_without_resolved_at" => {
                row["audit"]["prior_choices"] = json!([valid_prior_choice()]);
                row["audit"]["prior_choices"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("resolved_at");
            }
            "prior_without_replaced_at" => {
                row["audit"]["prior_choices"] = json!([valid_prior_choice()]);
                row["audit"]["prior_choices"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("replaced_at");
            }
            "invalid_prior_origin" => {
                row["audit"]["prior_choices"] = json!([valid_prior_choice()]);
                row["audit"]["prior_choices"][0]["replaced_by_origin"] = json!({"lane": ""});
            }
            _ => unreachable!(),
        }
        write_json(temporary.path(), "entities/ambiguities.jsonl", &row);
        let before = fs::read(temporary.path().join("entities/ambiguities.jsonl")).unwrap();
        let observation = valid_observation();

        assert!(
            record_ambiguity_observation(temporary.path(), &observation).is_err(),
            "{rule}"
        );
        assert_eq!(
            fs::read(temporary.path().join("entities/ambiguities.jsonl")).unwrap(),
            before,
            "{rule}"
        );
    }
}

#[test]
fn ambiguity_writer_refuses_occurrence_count_overflow_without_writing() {
    let temporary = TempDir::new();
    let mut row = valid_ambiguity_row();
    row["occurrence_count"] = Value::from(i64::MAX);
    write_json(temporary.path(), "entities/ambiguities.jsonl", &row);
    let before = fs::read(temporary.path().join("entities/ambiguities.jsonl")).unwrap();
    let mut observation = valid_observation();
    observation.origin = json!({"lane": "segment", "day": "20260805", "segment_id": "s2"});

    let error = record_ambiguity_observation(temporary.path(), &observation).unwrap_err();

    assert!(matches!(
        error,
        EntityWriteError::AmbiguityCountOverflow { .. }
    ));
    assert_eq!(
        fs::read(temporary.path().join("entities/ambiguities.jsonl")).unwrap(),
        before
    );
}

#[test]
fn ambiguity_writer_blocks_preexisting_corrupt_rows_without_changing_them() {
    for contents in ["[1, 2, 3]\n", "{\"schema_version\": 99}\n"] {
        let temporary = TempDir::new();
        write_text(temporary.path(), "entities/ambiguities.jsonl", contents);
        assert!(read_ambiguities(temporary.path(), MalformedPolicy::Skip).is_ok());
        assert!(read_ambiguities(temporary.path(), MalformedPolicy::Raise).is_err());

        assert!(record_ambiguity_observation(temporary.path(), &valid_observation()).is_err());
        assert_eq!(
            fs::read_to_string(temporary.path().join("entities/ambiguities.jsonl")).unwrap(),
            contents
        );
    }
}

#[test]
fn unreadable_identity_map_cache_is_rebuilt_and_reports_it() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/alice/entity.json",
        &json!({"id": "alice", "name": "Alice"}),
    );
    write_text(
        temporary.path(),
        "entities/.identity-map-cache.json",
        "not json\n",
    );

    let cache = refresh_identity_map_cache(temporary.path()).unwrap();

    assert!(cache.rebuilt);
    assert_eq!(cache.resolved.get("alice"), Some(&"alice".to_owned()));
    assert!(
        !fs::read_to_string(temporary.path().join("entities/.identity-map-cache.json"))
            .unwrap()
            .contains(&temporary.path().display().to_string())
    );
}

#[test]
fn identity_map_cache_reproduces_every_fixture_case() {
    let fixture = fixture();
    let cases = fixture["identity_map"]["cases"].as_array().unwrap();
    assert_eq!(
        cases.len(),
        fixture["identity_map"]["case_count"].as_u64().unwrap() as usize
    );

    for case in cases {
        let temporary = TempDir::new();
        for (directory, identity) in case["store"].as_object().unwrap() {
            let relative = format!("entities/{directory}/entity.json");
            if let Some(raw) = identity.as_str() {
                write_text(temporary.path(), &relative, raw);
            } else {
                write_json(temporary.path(), &relative, identity);
            }
        }
        let cache = refresh_identity_map_cache(temporary.path()).unwrap();
        assert!(cache.rebuilt, "{}", case["note"]);
        let expected = case["resolves"].as_object().unwrap();
        assert_eq!(cache.resolved.len(), expected.len(), "{}", case["note"]);
        for (id, directory) in expected {
            assert_eq!(
                cache.resolved.get(id),
                Some(&directory.as_str().unwrap().to_owned()),
                "{}",
                case["note"]
            );
        }
    }
}

#[test]
fn identity_map_cache_bytes_are_portable_across_journal_roots() {
    let source = TempDir::new();
    let destination = TempDir::new();
    write_json(
        source.path(),
        "entities/alice/entity.json",
        &json!({"id": "alice", "name": "Alice"}),
    );
    let source_cache = refresh_identity_map_cache(source.path()).unwrap();
    let bytes = fs::read(source.path().join("entities/.identity-map-cache.json")).unwrap();
    fs::create_dir_all(destination.path().join("entities")).unwrap();
    fs::write(
        destination.path().join("entities/.identity-map-cache.json"),
        &bytes,
    )
    .unwrap();

    let loaded = refresh_identity_map_cache(destination.path()).unwrap();

    assert!(!loaded.rebuilt);
    assert_eq!(loaded.resolved, source_cache.resolved);
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains(&source.path().display().to_string())
    );
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

fn valid_observation() -> AmbiguityObservation {
    let row = valid_ambiguity_row();
    AmbiguityObservation {
        scope: row["scope"].clone(),
        query: row["latest_query"].as_str().unwrap().to_owned(),
        normalized_query: row["normalized_query"].as_str().unwrap().to_owned(),
        observed_tier: row["observed_tier"].as_i64().unwrap(),
        ranked_candidates: row["ranked_candidates"].as_array().unwrap().clone(),
        origin: row["origins"][0].clone(),
    }
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

fn tree_hash_excluding_locks(root: &Path) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hash_tree_entry_excluding_locks(root, root, &mut hasher);
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

fn hash_tree_entry_excluding_locks(root: &Path, path: &Path, hasher: &mut Sha256) {
    if path
        .extension()
        .is_some_and(|extension| extension == "lock")
    {
        return;
    }
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
            hash_tree_entry_excluding_locks(root, &entry, hasher);
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

#[test]
fn identity_repair_fixture_cases_stamp_directory_ids() {
    let fixture = fixture();
    let cases = fixture["identity_repair"]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    assert_eq!(
        cases.len(),
        fixture["identity_repair"]["case_count"].as_u64().unwrap() as usize
    );

    for case in cases {
        let temporary = TempDir::new();
        let entity_dir = case["entity_dir"].as_str().unwrap();
        write_json(
            temporary.path(),
            &format!("entities/{entity_dir}/entity.json"),
            &case["before_repair"],
        );

        let report = repair_entity_identities(temporary.path()).unwrap();

        let actual = read_entity_identity(temporary.path(), entity_dir)
            .unwrap()
            .unwrap()
            .value()
            .clone();
        assert_eq!(actual, case["after_repair"], "{}", case["note"]);
        match case["before_repair"].get("id").and_then(Value::as_str) {
            None => assert_eq!(report.added, vec![entity_dir.to_owned()]),
            Some(id) if id == entity_dir => {
                assert_eq!(report.left_alone, vec![entity_dir.to_owned()])
            }
            Some(_) => assert_eq!(report.overwritten, vec![entity_dir.to_owned()]),
        }
    }
}

#[test]
fn identity_repair_adds_id_first_with_exact_ascii_and_unicode_artifacts() {
    let fixture = fixture();
    for (entity_dir, artifact_name) in [
        ("alice_johnson", "entities/{id}/entity.json"),
        ("jose_garcia", "entities/{id}/entity.json (unicode)"),
    ] {
        let temporary = TempDir::new();
        let artifact_identity: Value =
            serde_json::from_str(artifact(&fixture, artifact_name)).unwrap();
        let identity = Value::Object(
            artifact_identity
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.as_str() != "id")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        write_json(
            temporary.path(),
            &format!("entities/{entity_dir}/entity.json"),
            &identity,
        );

        repair_entity_identities(temporary.path()).unwrap();

        let path = temporary
            .path()
            .join(format!("entities/{entity_dir}/entity.json"));
        assert_eq!(
            fs::read(&path).unwrap(),
            artifact(&fixture, artifact_name).as_bytes()
        );
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&fs::metadata(path).unwrap().permissions())
                & 0o777,
            0o600
        );
    }
}

#[test]
fn identity_repair_leaves_correct_identity_inode_unchanged_and_preserves_overwrite_fields() {
    let temporary = TempDir::new();
    let correct = json!({"id": "correct", "name": "Correct", "aka": ["C"]});
    let divergent = json!({"name": "Divergent", "id": "elsewhere", "aka": ["D"]});
    write_json(temporary.path(), "entities/correct/entity.json", &correct);
    write_json(
        temporary.path(),
        "entities/divergent/entity.json",
        &divergent,
    );
    let correct_path = temporary.path().join("entities/correct/entity.json");
    let correct_before = fs::read(&correct_path).unwrap();
    #[cfg(unix)]
    let correct_inode = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&correct_path).unwrap());

    let report = repair_entity_identities(temporary.path()).unwrap();

    assert_eq!(report.left_alone, vec!["correct"]);
    assert_eq!(report.overwritten, vec!["divergent"]);
    assert_eq!(fs::read(&correct_path).unwrap(), correct_before);
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::MetadataExt::ino(&fs::metadata(&correct_path).unwrap()),
        correct_inode
    );
    let repaired = read_entity_identity(temporary.path(), "divergent")
        .unwrap()
        .unwrap()
        .value()
        .clone();
    let mut expected = divergent;
    expected["id"] = json!("divergent");
    assert_eq!(repaired, expected);
}

#[test]
fn identity_repair_collects_every_report_branch_without_writing_refused_or_skipped_entries() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/added/entity.json",
        &json!({"name": "Added"}),
    );
    write_json(
        temporary.path(),
        "entities/overwritten/entity.json",
        &json!({"id": "old", "name": "Overwrite"}),
    );
    write_json(
        temporary.path(),
        "entities/left/entity.json",
        &json!({"id": "left", "name": "Left"}),
    );
    write_json(
        temporary.path(),
        "entities/staged/entity.json",
        &json!({"id": "old", "name": "Staged"}),
    );
    let staged = history_event(1, "vh_staged", "update");
    write_json(
        temporary.path(),
        "entities/staged/history/prepared/vh_staged/event.json",
        &staged,
    );
    write_text(
        temporary.path(),
        "entities/malformed/entity.json",
        "not json",
    );
    fs::create_dir_all(temporary.path().join("entities/not_an_entity")).unwrap();
    write_bytes(temporary.path(), "entities/empty/entity.json", b"");
    write_text(temporary.path(), "entities/null/entity.json", "null");
    let staged_before = fs::read(temporary.path().join("entities/staged/entity.json")).unwrap();
    let malformed_before =
        fs::read(temporary.path().join("entities/malformed/entity.json")).unwrap();

    let report = incomplete_repair_report(temporary.path());

    assert_eq!(report.added, vec!["added"]);
    assert_eq!(report.overwritten, vec!["overwritten"]);
    assert_eq!(report.left_alone, vec!["left"]);
    assert_eq!(
        report
            .refused
            .iter()
            .map(|refusal| (&refusal.entity_dir, &refusal.guard))
            .collect::<Vec<_>>(),
        vec![
            (
                &"malformed".to_owned(),
                &EntityIdentityRepairGuard::Malformed
            ),
            (
                &"staged".to_owned(),
                &EntityIdentityRepairGuard::StagedPreparedHistory
            ),
        ]
    );
    assert!(report.refused.iter().all(|refusal| {
        refusal.detail.contains(&refusal.entity_dir)
            && refusal.detail.contains("guard")
            && refusal.detail.contains("before re-running")
    }));
    assert_eq!(
        report.skipped,
        vec![
            crate::EntityIdentityRepairSkip {
                entity_dir: "empty".to_owned(),
                reason: EntityIdentityRepairSkipReason::EmptyIdentityFile,
            },
            crate::EntityIdentityRepairSkip {
                entity_dir: "not_an_entity".to_owned(),
                reason: EntityIdentityRepairSkipReason::NotAnEntity,
            },
            crate::EntityIdentityRepairSkip {
                entity_dir: "null".to_owned(),
                reason: EntityIdentityRepairSkipReason::EmptyIdentityFile,
            },
        ]
    );
    assert_eq!(
        fs::read(temporary.path().join("entities/staged/entity.json")).unwrap(),
        staged_before
    );
    assert_eq!(
        fs::read(temporary.path().join("entities/malformed/entity.json")).unwrap(),
        malformed_before
    );
    assert!(
        !temporary
            .path()
            .join("entities/not_an_entity/entity.json")
            .exists()
    );
    assert!(!repair_marker_path(temporary.path()).exists());
}

#[test]
fn identity_repair_refuses_publish_classified_staged_history_without_reconciling() {
    let temporary = TempDir::new();
    let current = json!({"id": "old", "name": "Current"});
    write_json(temporary.path(), "entities/staged/entity.json", &current);
    write_json(
        temporary.path(),
        "entities/added/entity.json",
        &json!({"name": "Added"}),
    );
    let mut event = history_event(1, "vh_publish", "update");
    event["entity_id"] = json!("staged");
    event["identity_before"] = json!({"id": "old", "name": "Before"});
    event["identity_after"] = current.clone();
    write_json(
        temporary.path(),
        "entities/staged/history/prepared/vh_publish/event.json",
        &event,
    );
    let before = fs::read(temporary.path().join("entities/staged/entity.json")).unwrap();
    assert_prepared_outcome(temporary.path(), "staged", PreparedHistoryOutcome::Publish);

    let report = incomplete_repair_report(temporary.path());

    assert_eq!(report.added, vec!["added"]);
    assert_eq!(
        fs::read(temporary.path().join("entities/staged/entity.json")).unwrap(),
        before
    );
    assert_prepared_outcome(temporary.path(), "staged", PreparedHistoryOutcome::Publish);
    assert_eq!(
        read_prepared_history(temporary.path(), "staged")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn identity_repair_refuses_discard_classified_staged_history_without_reconciling() {
    let temporary = TempDir::new();
    let current = json!({"id": "old", "name": "Current"});
    write_json(temporary.path(), "entities/staged/entity.json", &current);
    let mut event = history_event(1, "vh_discard", "update");
    event["entity_id"] = json!("staged");
    event["identity_before"] = current.clone();
    event["identity_after"] = json!({"id": "old", "name": "After"});
    write_json(
        temporary.path(),
        "entities/staged/history/prepared/vh_discard/event.json",
        &event,
    );
    assert_prepared_outcome(temporary.path(), "staged", PreparedHistoryOutcome::Discard);

    let report = incomplete_repair_report(temporary.path());

    assert_eq!(report.refused.len(), 1);
    assert_prepared_outcome(temporary.path(), "staged", PreparedHistoryOutcome::Discard);
    assert_eq!(
        read_prepared_history(temporary.path(), "staged")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn identity_repair_resolves_every_non_refused_directory_to_its_own_id_including_swaps() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/added/entity.json",
        &json!({"name": "Added"}),
    );
    write_json(
        temporary.path(),
        "entities/x/entity.json",
        &json!({"id": "y", "name": "X"}),
    );
    write_json(
        temporary.path(),
        "entities/y/entity.json",
        &json!({"id": "x", "name": "Y"}),
    );

    repair_entity_identities(temporary.path()).unwrap();

    let map = read_identity_map(temporary.path()).unwrap();
    assert_eq!(map.resolved.len(), 3);
    assert!(map.losers.is_empty());
    for (identity_id, entity_dir) in map.resolved {
        assert_eq!(identity_id, entity_dir);
    }
}

#[test]
fn identity_repair_leaves_a_refused_entity_as_the_sole_loser_when_its_stale_id_collides() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/bob/entity.json",
        &json!({"id": "bob", "name": "Bob"}),
    );
    write_json(
        temporary.path(),
        "entities/zed/entity.json",
        &json!({"id": "bob", "name": "Zed"}),
    );
    let mut staged = history_event(1, "vh_staged", "update");
    staged["entity_id"] = json!("zed");
    write_json(
        temporary.path(),
        "entities/zed/history/prepared/vh_staged/event.json",
        &staged,
    );

    let report = incomplete_repair_report(temporary.path());
    let map = read_identity_map(temporary.path()).unwrap();

    assert_eq!(report.refused.len(), 1);
    assert_eq!(report.refused[0].entity_dir, "zed");
    assert_eq!(
        report.refused[0].guard,
        EntityIdentityRepairGuard::StagedPreparedHistory
    );
    assert_eq!(
        map.resolved,
        std::collections::HashMap::from([(String::from("bob"), String::from("bob"))])
    );
    assert_eq!(map.losers.len(), 1);
    assert_eq!(map.losers[0].entity_dir, "zed");
    assert_eq!(map.losers[0].reason, IdentityMapLoserReason::CollisionLost);
}

#[test]
fn identity_repair_uses_marker_presence_for_one_shot_refusal() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/alice/entity.json",
        &json!({"name": "Alice"}),
    );
    repair_entity_identities(temporary.path()).unwrap();
    let path = temporary.path().join("entities/alice/entity.json");
    write_json(
        temporary.path(),
        "entities/alice/entity.json",
        &json!({"name": "Changed"}),
    );
    let before_second_run = fs::read(&path).unwrap();

    let error = repair_entity_identities(temporary.path()).unwrap_err();

    assert!(matches!(
        error,
        EntityIdentityRepairError::AlreadyCompleted { .. }
    ));
    assert_eq!(fs::read(path).unwrap(), before_second_run);
}

#[test]
fn identity_repair_marks_a_clean_zero_write_run_complete() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/alice/entity.json",
        &json!({"id": "alice", "name": "Alice"}),
    );

    let report = repair_entity_identities(temporary.path()).unwrap();

    assert_eq!(report.left_alone, vec!["alice"]);
    assert!(repair_marker_path(temporary.path()).is_file());
}

#[test]
fn identity_repair_aborts_without_marker_and_resumes_after_a_partial_write_failure() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/alpha/entity.json",
        &json!({"name": "Alpha"}),
    );
    write_json(
        temporary.path(),
        "entities/beta/entity.json",
        &json!({"name": "Beta"}),
    );
    set_repair_identity_write_failure_on_attempt(Some(2));

    let error = repair_entity_identities(temporary.path()).unwrap_err();
    set_repair_identity_write_failure_on_attempt(None);

    let partial = match error {
        EntityIdentityRepairError::IdentityWrite { report, .. } => *report,
        other => panic!("unexpected repair error: {other}"),
    };
    assert_eq!(partial.added, vec!["alpha"]);
    assert!(!repair_marker_path(temporary.path()).exists());
    assert!(
        read_entity_identity(temporary.path(), "alpha")
            .unwrap()
            .unwrap()
            .was_written()
    );
    assert!(
        !read_entity_identity(temporary.path(), "beta")
            .unwrap()
            .unwrap()
            .was_written()
    );

    let resumed = repair_entity_identities(temporary.path()).unwrap();

    assert_eq!(resumed.left_alone, vec!["alpha"]);
    assert_eq!(resumed.added, vec!["beta"]);
    assert!(repair_marker_path(temporary.path()).is_file());
}

fn incomplete_repair_report(root: &Path) -> crate::EntityIdentityRepairReport {
    match repair_entity_identities(root).unwrap_err() {
        EntityIdentityRepairError::Incomplete { report } => *report,
        other => panic!("unexpected repair error: {other}"),
    }
}

fn repair_marker_path(root: &Path) -> PathBuf {
    root.join("health/migrations/entity-identity-repair.json")
}

fn assert_prepared_outcome(root: &Path, entity_dir: &str, expected: PreparedHistoryOutcome) {
    let current = read_entity_identity(root, entity_dir).unwrap();
    let prepared = read_prepared_history(root, entity_dir).unwrap();
    assert_eq!(
        classify_prepared_history(entity_dir, &prepared[0].event, current.as_ref()).unwrap(),
        expected
    );
}

#[test]
fn direct_journal_entity_scan_is_empty_when_entities_are_absent() {
    let temporary = TempDir::new();
    assert!(
        load_all_journal_entities(temporary.path())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn direct_journal_entity_scan_preserves_identity_fields_and_sorts_ids() {
    let temporary = TempDir::new();
    for (directory, value) in [
        (
            "zeta",
            json!({"id": "zeta", "name": "Zeta", "type": "Person", "blocked": true}),
        ),
        (
            "alpha",
            json!({"id": "alpha", "name": "Alpha", "type": "Person", "is_principal": "yes", "aka": ["A"], "emails": ["a@example.test"]}),
        ),
        ("middle", json!({"id": "middle", "name": "Middle"})),
    ] {
        let path = temporary
            .path()
            .join("entities")
            .join(directory)
            .join("entity.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.to_string()).unwrap();
    }

    let entities = load_all_journal_entities(temporary.path()).unwrap();
    assert_eq!(
        entities
            .iter()
            .map(|entity| entity.id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "middle", "zeta"]
    );
    assert_eq!(entities[0].entity_type(), Some("Person"));
    assert!(entities[0].is_principal());
    assert!(!entities[0].is_blocked());
    assert_eq!(entities[0].resolution_entity().aka, ["A"]);
    assert_eq!(entities[0].resolution_entity().emails, ["a@example.test"]);
    assert_eq!(entities[1].entity_type(), None);
    assert!(entities[2].is_blocked());
}

#[test]
fn direct_journal_entity_scan_skips_one_corrupt_identity() {
    let temporary = TempDir::new();
    let valid = temporary.path().join("entities/valid/entity.json");
    fs::create_dir_all(valid.parent().unwrap()).unwrap();
    fs::write(valid, json!({"id": "valid", "name": "Valid"}).to_string()).unwrap();
    let corrupt = temporary.path().join("entities/corrupt/entity.json");
    fs::create_dir_all(corrupt.parent().unwrap()).unwrap();
    fs::write(corrupt, b"{").unwrap();

    let entities = load_all_journal_entities(temporary.path()).unwrap();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].id, "valid");
}
