// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{NaiveDate, TimeZone};
use serde_json::{Value, json};

use crate::{
    AmbiguityChoiceEntity, AmbiguityChoiceRequest, AmbiguityObservation, EntityLifecycleError,
    create_journal_entity, delete_entity_directory, entity_last_active_day, entity_last_active_ts,
    entity_matches_identity_name, entity_memory_path, entity_path, has_journal_principal,
    is_valid_entity_type, last_active_day_for_ts, read_entity_identity, read_identity_map,
    read_journal_principal, read_visible_history, record_ambiguity_choice,
    record_ambiguity_observation, remove_entity_ambiguity_references,
    restore_journal_entity_version, save_entity_identity, unblock_journal_entity,
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);
const LIFECYCLE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/entity_lifecycle.json"
));

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-lifecycle-{}-{sequence}",
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
fn unblock_clears_blocked_and_refuses_missing_or_unblocked_without_writing() {
    let temporary = TempDir::new();
    save_entity_identity(
        temporary.path(),
        "blocked",
        &json!({"id": "blocked", "blocked": true, "name": "Blocked"}),
        None,
    )
    .unwrap();
    let event = unblock_journal_entity(temporary.path(), "blocked").unwrap();
    assert_eq!(event["kind"], "update");
    assert!(
        read_entity_identity(temporary.path(), "blocked")
            .unwrap()
            .unwrap()
            .value()
            .get("blocked")
            .is_none()
    );

    save_entity_identity(
        temporary.path(),
        "open",
        &json!({"id": "open", "name": "Open"}),
        None,
    )
    .unwrap();
    let open_before = fs::read(temporary.path().join("entities/open/entity.json")).unwrap();
    assert!(matches!(
        unblock_journal_entity(temporary.path(), "open"),
        Err(EntityLifecycleError::EntityNotBlocked { .. })
    ));
    assert_eq!(
        fs::read(temporary.path().join("entities/open/entity.json")).unwrap(),
        open_before
    );
    assert!(matches!(
        unblock_journal_entity(temporary.path(), "missing"),
        Err(EntityLifecycleError::EntityNotFound { .. })
    ));
    assert!(!temporary.path().join("entities/missing").exists());
}

#[test]
fn restore_refuses_target_merge_and_crossing_later_merge() {
    let target_merge = TempDir::new();
    let target_event = save_entity_identity(
        target_merge.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice"}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    rewrite_event_kind(
        target_merge.path(),
        "alice",
        &target_event["version_id"],
        "merge",
    );
    assert!(matches!(
        restore_journal_entity_version(
            target_merge.path(),
            "alice",
            target_event["version_id"].as_str().unwrap(),
            None,
        ),
        Err(EntityLifecycleError::RestoreTargetsRecordedMerge)
    ));

    let later_merge = TempDir::new();
    let first = save_entity_identity(
        later_merge.path(),
        "alice",
        &json!({"id": "alice", "name": "Before"}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    let second = save_entity_identity(
        later_merge.path(),
        "alice",
        &json!({"id": "alice", "name": "After"}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    rewrite_event_kind(later_merge.path(), "alice", &second["version_id"], "merge");
    assert!(matches!(
        restore_journal_entity_version(
            later_merge.path(),
            "alice",
            first["version_id"].as_str().unwrap(),
            None,
        ),
        Err(EntityLifecycleError::RestoreCrossesRecordedMerge)
    ));
}

#[test]
fn restore_validates_snapshot_identity_and_principal_uniqueness() {
    let mismatch = TempDir::new();
    let event = save_entity_identity(
        mismatch.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice"}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    rewrite_event_identity(mismatch.path(), "alice", &event["version_id"], "other");
    assert!(matches!(
        restore_journal_entity_version(
            mismatch.path(),
            "alice",
            event["version_id"].as_str().unwrap(),
            None,
        ),
        Err(EntityLifecycleError::RestoreSnapshotIdentityMismatch { .. })
    ));

    let principal = TempDir::new();
    let principal_version = save_entity_identity(
        principal.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice", "is_principal": true}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    save_entity_identity(
        principal.path(),
        "alice",
        &json!({"id": "alice", "name": "Alice", "is_principal": false}),
        None,
    )
    .unwrap();
    save_entity_identity(
        principal.path(),
        "bob",
        &json!({"id": "bob", "name": "Bob", "is_principal": true}),
        None,
    )
    .unwrap();
    assert!(matches!(
        restore_journal_entity_version(
            principal.path(),
            "alice",
            principal_version["version_id"].as_str().unwrap(),
            None,
        ),
        Err(EntityLifecycleError::RestoreWouldCreateSecondPrincipal {
            existing_entity_id,
            ..
        }) if existing_entity_id == "bob"
    ));

    let self_principal = TempDir::new();
    let self_version = save_entity_identity(
        self_principal.path(),
        "alice",
        &json!({"id": "alice", "name": "Before", "is_principal": true}),
        None,
    )
    .unwrap()
    .event
    .unwrap();
    save_entity_identity(
        self_principal.path(),
        "alice",
        &json!({"id": "alice", "name": "After", "is_principal": true}),
        None,
    )
    .unwrap();
    assert!(
        restore_journal_entity_version(
            self_principal.path(),
            "alice",
            self_version["version_id"].as_str().unwrap(),
            None,
        )
        .is_ok()
    );
}

#[test]
fn restore_replays_snapshot_and_appends_one_restore_event() {
    let temporary = TempDir::new();
    let snapshot = json!({"id": "alice", "name": "Before", "tags": ["one"]});
    let first = save_entity_identity(temporary.path(), "alice", &snapshot, None)
        .unwrap()
        .event
        .unwrap();
    save_entity_identity(
        temporary.path(),
        "alice",
        &json!({"id": "alice", "name": "After", "new": true}),
        None,
    )
    .unwrap();
    let before_count = read_visible_history(temporary.path(), "alice")
        .unwrap()
        .len();

    let restored = restore_journal_entity_version(
        temporary.path(),
        "alice",
        first["version_id"].as_str().unwrap(),
        Some(json!({"source": "test"})),
    )
    .unwrap();
    assert_eq!(restored["kind"], "restore");
    assert_eq!(
        restored["operation"]["restored_version_id"],
        first["version_id"]
    );
    assert_eq!(
        read_entity_identity(temporary.path(), "alice")
            .unwrap()
            .unwrap()
            .value(),
        &snapshot
    );
    assert_eq!(
        read_visible_history(temporary.path(), "alice")
            .unwrap()
            .len(),
        before_count + 1
    );
}

#[test]
fn principal_reads_are_empty_or_return_the_principal() {
    let temporary = TempDir::new();
    assert_eq!(read_journal_principal(temporary.path()).unwrap(), None);
    assert!(!has_journal_principal(temporary.path()).unwrap());
    save_entity_identity(
        temporary.path(),
        "owner",
        &json!({"id": "owner", "is_principal": true}),
        None,
    )
    .unwrap();
    assert_eq!(
        read_journal_principal(temporary.path()).unwrap().unwrap()["id"],
        "owner"
    );
    assert!(has_journal_principal(temporary.path()).unwrap());
}

#[test]
fn guarded_create_refuses_existing_resolved_id_without_changing_identity_bytes() {
    let temporary = TempDir::new();
    save_entity_identity(
        temporary.path(),
        "target",
        &json!({
            "id": "target",
            "name": "Before",
            "aka": ["Alias"],
            "emails": ["before@example.com"],
            "is_principal": true,
            "blocked": true,
        }),
        None,
    )
    .unwrap();
    let identity_path = temporary.path().join("entities/target/entity.json");
    let before = fs::read(&identity_path).unwrap();

    assert!(matches!(
        create_journal_entity(
            temporary.path(),
            "target",
            "After",
            "Person",
            None,
            None,
            &[],
            false,
            None,
        ),
        Err(EntityLifecycleError::EntityAlreadyExists { .. })
    ));
    assert_eq!(fs::read(identity_path).unwrap(), before);
}

#[test]
fn entity_paths_resolve_divergent_directory_and_never_create_unresolved_memory() {
    let temporary = TempDir::new();
    let directory = temporary.path().join("entities/durable-directory");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("entity.json"), b"{\"id\":\"effective-id\"}").unwrap();

    assert_eq!(
        entity_path(temporary.path(), "effective-id").unwrap(),
        directory.join("entity.json")
    );
    assert_eq!(
        entity_memory_path(temporary.path(), "effective-id", true).unwrap(),
        directory
    );
    assert!(matches!(
        entity_memory_path(temporary.path(), "missing", true),
        Err(EntityLifecycleError::EntityNotFound { .. })
    ));
    assert!(!temporary.path().join("entities/missing").exists());
}

#[test]
fn type_validation_rejects_trailing_newline_unlike_python_regex_dollar() {
    assert!(is_valid_entity_type("Vehicle"));
    assert!(is_valid_entity_type("123"));
    assert!(!is_valid_entity_type("AI"));
    assert!(!is_valid_entity_type("Person\n"));
}

fn zone_midnight_ms(year: i32, month: u32, day: u32, zone: chrono_tz::Tz) -> i64 {
    zone.from_local_datetime(
        &NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
    )
    .earliest()
    .unwrap()
    .timestamp_millis()
}

#[test]
fn last_active_ts_converts_last_seen_at_zone_midnight_not_utc() {
    let zone = chrono_tz::America::New_York;
    assert_eq!(
        entity_last_active_ts(&json!({"last_seen": "20260115"}), zone),
        Some(zone_midnight_ms(2026, 1, 15, zone))
    );
}

#[test]
fn last_active_ts_reads_rfc3339_and_millisecond_timestamps_alike() {
    let written_as_text = json!({"attached_at": "2026-09-27T18:30:00.000000Z"});
    let written_as_millis = json!({"attached_at": 1_790_533_800_000i64});
    assert_eq!(
        entity_last_active_ts(&written_as_text, chrono_tz::UTC),
        Some(1_790_533_800_000)
    );
    assert_eq!(
        entity_last_active_ts(&written_as_millis, chrono_tz::UTC),
        Some(1_790_533_800_000)
    );
}

#[test]
fn last_active_ts_is_the_latest_recorded_activity() {
    let entity = json!({
        "last_seen": "20260115",
        "attached_at": "2026-02-01T12:00:00Z",
        "updated_at": "2026-09-27T18:30:00Z",
    });
    assert_eq!(
        entity_last_active_ts(&entity, chrono_tz::UTC),
        Some(1_790_533_800_000)
    );
    let created_only = json!({"created_at": 1_790_533_800_000i64});
    assert_eq!(
        entity_last_active_ts(&created_only, chrono_tz::UTC),
        Some(1_790_533_800_000)
    );
}

#[test]
fn last_active_is_unknown_rather_than_invented_without_activity() {
    let entity = json!({
        "name": "New Person",
        "last_seen": "not-a-day",
        "updated_at": "",
        "attached_at": 0,
    });
    assert_eq!(entity_last_active_ts(&entity, chrono_tz::UTC), None);
    assert_eq!(entity_last_active_day(&entity, chrono_tz::UTC), None);
}

#[test]
fn last_active_day_for_ts_matches_independently_computed_zone_day() {
    let ts = 1_790_533_800_000;
    let zone = chrono_tz::Asia::Tokyo;
    let expected = zone
        .timestamp_millis_opt(ts)
        .single()
        .unwrap()
        .format("%Y%m%d")
        .to_string();
    assert_eq!(last_active_day_for_ts(ts, zone), Some(expected));
}

#[test]
fn last_active_day_keeps_a_latest_last_seen_day() {
    let zone = chrono_tz::America::New_York;
    assert_eq!(
        entity_last_active_day(
            &json!({
                "last_seen": "20260115",
                "attached_at": zone_midnight_ms(2026, 1, 10, zone),
            }),
            zone
        ),
        Some("20260115".to_owned())
    );
}

#[test]
fn identity_name_matching_checks_entity_name_and_aliases_case_insensitively() {
    assert!(entity_matches_identity_name(
        "Unrelated",
        Some(&["Owner Name".to_owned()]),
        &["owner name".to_owned()],
    ));
    assert!(entity_matches_identity_name(
        "OWNER",
        None,
        &["owner".to_owned()]
    ));
    assert!(!entity_matches_identity_name(
        "Owner",
        None,
        &["different".to_owned()],
    ));
}

#[test]
fn removing_entity_ambiguity_references_rewrites_only_target_rows() {
    let temporary = TempDir::new();
    record_observation(temporary.path(), "target", vec!["target", "survivor"]);
    resolve_observation(
        temporary.path(),
        "target",
        "target",
        &["target", "survivor"],
    );
    record_observation(temporary.path(), "other", vec!["survivor"]);
    resolve_observation(temporary.path(), "other", "survivor", &["survivor"]);
    let before_other = ambiguity_line(temporary.path(), 1);

    let report = remove_entity_ambiguity_references(temporary.path(), "target").unwrap();
    assert_eq!(report.rewritten_ambiguity_ids.len(), 1);
    assert!(report.removed_ambiguity_ids.is_empty());
    let rows = crate::read_ambiguities(
        temporary.path(),
        solstone_core_journal_io::MalformedPolicy::Raise,
    )
    .unwrap();
    assert_eq!(rows[0]["status"], "open");
    assert!(rows[0].get("resolved_entity_id").is_none());
    assert_eq!(rows[0]["ranked_candidates"][0]["id"], "survivor");
    assert_eq!(ambiguity_line(temporary.path(), 1), before_other);
}

#[test]
fn removing_entity_ambiguity_references_retains_rows_when_last_candidate_removed() {
    let temporary = TempDir::new();
    record_observation(temporary.path(), "only-target", vec!["target"]);
    let report = remove_entity_ambiguity_references(temporary.path(), "target").unwrap();
    assert_eq!(report.rewritten_ambiguity_ids, Vec::<String>::new());
    assert_eq!(report.removed_ambiguity_ids, Vec::<String>::new());
    let rows = crate::read_ambiguities(
        temporary.path(),
        solstone_core_journal_io::MalformedPolicy::Raise,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ranked_candidates"][0]["id"], "target");
}

#[test]
fn delete_entity_directory_removes_the_effective_directory_and_rebuilds_cache() {
    let temporary = TempDir::new();
    save_entity_identity(
        temporary.path(),
        "effective",
        &json!({"id": "effective", "name": "Entity"}),
        None,
    )
    .unwrap();
    assert!(
        read_identity_map(temporary.path())
            .unwrap()
            .resolved
            .contains_key("effective")
    );
    delete_entity_directory(temporary.path(), "effective").unwrap();
    assert!(!temporary.path().join("entities/effective").exists());
    assert!(
        !read_identity_map(temporary.path())
            .unwrap()
            .resolved
            .contains_key("effective")
    );
}

#[test]
fn lifecycle_fixture_declares_a_target_identity() {
    let fixture: Value = serde_json::from_str(LIFECYCLE_FIXTURE).unwrap();
    assert_eq!(fixture["target_entity_id"], "target");
}

fn rewrite_event_kind(root: &Path, entity: &str, version: &Value, kind: &str) {
    rewrite_event(root, entity, version, |event| event["kind"] = json!(kind));
}

fn rewrite_event_identity(root: &Path, entity: &str, version: &Value, id: &str) {
    rewrite_event(root, entity, version, |event| {
        event["identity_after"]["id"] = json!(id)
    });
}

fn rewrite_event(root: &Path, entity: &str, version: &Value, change: impl FnOnce(&mut Value)) {
    let path = fs::read_dir(root.join("entities").join(entity).join("history/events"))
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .find(|path| {
            fs::read_to_string(path)
                .unwrap()
                .contains(version.as_str().unwrap())
        })
        .unwrap();
    let mut event: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut event);
    fs::write(path, serde_json::to_vec(&event).unwrap()).unwrap();
}

fn record_observation(root: &Path, query: &str, candidates: Vec<&str>) {
    record_ambiguity_observation(
        root,
        &AmbiguityObservation {
            scope: json!({"kind": "journal"}),
            query: query.to_owned(),
            normalized_query: query.to_owned(),
            observed_tier: 5,
            ranked_candidates: candidates
                .into_iter()
                .map(|id| json!({"id": id, "name": id, "tier": 5, "score": 1.0}))
                .collect(),
            origin: json!({"lane": "segment", "day": "20260805", "segment_id": query}),
        },
    )
    .unwrap();
}

fn resolve_observation(root: &Path, query: &str, entity_id: &str, candidates: &[&str]) {
    record_ambiguity_choice(
        root,
        &AmbiguityChoiceRequest {
            scope: json!({"kind": "journal"}),
            query: query.to_owned(),
            entity_id: entity_id.to_owned(),
            origin: None,
        },
        &candidates
            .iter()
            .map(|id| AmbiguityChoiceEntity {
                id: (*id).to_owned(),
                blocked: false,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
}

fn ambiguity_line(root: &Path, index: usize) -> String {
    fs::read_to_string(root.join("entities/ambiguities.jsonl"))
        .unwrap()
        .lines()
        .nth(index)
        .unwrap()
        .to_owned()
}

fn write_identity_config(root: &Path, identity: Value) {
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_vec(&json!({"identity": identity})).unwrap(),
    )
    .unwrap();
}

fn created_is_principal(root: &Path, id: &str, name: &str, skip_principal: bool) -> bool {
    create_journal_entity(
        root,
        id,
        name,
        "Person",
        None,
        None,
        &[],
        skip_principal,
        None,
    )
    .unwrap();
    read_entity_identity(root, id)
        .unwrap()
        .unwrap()
        .value()
        .get("is_principal")
        == Some(&Value::Bool(true))
}

#[test]
fn a_new_entity_named_as_the_configured_owner_becomes_the_principal() {
    let journal = TempDir::new();
    write_identity_config(
        journal.path(),
        json!({"name": "Jordan Rivera", "preferred": "Jo", "aliases": ["J. Rivera"]}),
    );
    assert!(!created_is_principal(
        journal.path(),
        "sam",
        "Sam Lee",
        false
    ));
    assert!(created_is_principal(
        journal.path(),
        "jordan",
        "Jordan Rivera",
        false
    ));
    // One principal only: a later match does not become a second one.
    assert!(!created_is_principal(journal.path(), "jo", "Jo", false));

    let skipped = TempDir::new();
    write_identity_config(skipped.path(), json!({"name": "Jordan Rivera"}));
    assert!(!created_is_principal(
        skipped.path(),
        "jordan",
        "Jordan Rivera",
        true
    ));

    let unconfigured = TempDir::new();
    assert!(!created_is_principal(
        unconfigured.path(),
        "jordan",
        "Jordan Rivera",
        false
    ));
}

#[test]
fn creating_a_non_person_with_an_owner_name_does_not_take_the_principal() {
    for entity_type in ["Company", "Project", "Tool"] {
        let journal = TempDir::new();
        write_identity_config(
            journal.path(),
            json!({"name": "Jordan Rivera", "preferred": "Jo"}),
        );
        create_journal_entity(
            journal.path(),
            "named_non_person",
            "Jordan Rivera",
            entity_type,
            None,
            None,
            &[],
            false,
            None,
        )
        .unwrap();
        assert_ne!(
            principal_flag(journal.path(), "named_non_person"),
            Some(Value::Bool(true))
        );
        assert!(!has_journal_principal(journal.path()).unwrap());
        assert!(created_is_principal(journal.path(), "jo", "Jo", false));
    }
}

#[test]
fn configured_owner_names_come_preferred_then_full_then_aliases() {
    let journal = TempDir::new();
    write_identity_config(
        journal.path(),
        json!({"name": " Jordan Rivera ", "preferred": "Jo", "aliases": ["Jo", "", "J. Rivera"]}),
    );
    assert_eq!(
        crate::journal_identity_names(journal.path()),
        vec![
            "Jo".to_owned(),
            "Jordan Rivera".to_owned(),
            "J. Rivera".to_owned()
        ]
    );
}

fn seed_person(root: &Path, id: &str, name: &str, extra: Value) {
    let mut identity = json!({"id": id, "name": name, "type": "Person"});
    if let (Some(object), Some(extra)) = (identity.as_object_mut(), extra.as_object()) {
        object.extend(extra.clone());
    }
    save_entity_identity(root, id, &identity, None).unwrap();
}

fn principal_flag(root: &Path, id: &str) -> Option<Value> {
    read_entity_identity(root, id)
        .unwrap()
        .unwrap()
        .value()
        .get("is_principal")
        .cloned()
}

#[test]
fn adoption_marks_the_one_existing_person_named_as_the_owner() {
    let journal = TempDir::new();
    write_identity_config(
        journal.path(),
        json!({"name": "Jordan Rivera", "preferred": "Jo"}),
    );
    seed_person(journal.path(), "jordan", "Jordan Rivera", json!({}));
    seed_person(journal.path(), "sam", "Sam Lee", json!({}));
    seed_person(
        journal.path(),
        "jo_project",
        "Jo",
        json!({"type": "Project"}),
    );
    seed_person(journal.path(), "jo_blocked", "Jo", json!({"blocked": true}));

    assert_eq!(
        crate::adopt_configured_principal(journal.path()).unwrap(),
        Some("jordan".to_owned())
    );
    assert_eq!(
        principal_flag(journal.path(), "jordan"),
        Some(Value::Bool(true))
    );
    assert_eq!(principal_flag(journal.path(), "sam"), None);
    // With a principal in place, adoption is a no-op.
    assert_eq!(
        crate::adopt_configured_principal(journal.path()).unwrap(),
        None
    );
}

#[test]
fn adoption_changes_nothing_when_the_owner_match_is_ambiguous_or_unconfigured() {
    let ambiguous = TempDir::new();
    write_identity_config(
        ambiguous.path(),
        json!({"name": "Jordan Rivera", "preferred": "Jo"}),
    );
    seed_person(ambiguous.path(), "jordan", "Jordan Rivera", json!({}));
    seed_person(ambiguous.path(), "jo", "Jo", json!({}));
    assert_eq!(
        crate::adopt_configured_principal(ambiguous.path()).unwrap(),
        None
    );
    assert_eq!(principal_flag(ambiguous.path(), "jordan"), None);
    assert_eq!(principal_flag(ambiguous.path(), "jo"), None);

    let unconfigured = TempDir::new();
    seed_person(unconfigured.path(), "jordan", "Jordan Rivera", json!({}));
    assert_eq!(
        crate::adopt_configured_principal(unconfigured.path()).unwrap(),
        None
    );
    assert_eq!(principal_flag(unconfigured.path(), "jordan"), None);
}
