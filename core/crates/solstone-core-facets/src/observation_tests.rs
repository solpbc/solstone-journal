// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;
use std::time::Duration;

use serde_json::json;
use solstone_core_journal_io::{AtomicWriteError, LockError, LockTimeout};

use crate::store::{retry_add_for_test, retry_record_for_test};
use crate::store_tests::{
    TempDir, create_test_facet, write_facet_relationship, write_journal_entity,
};
use crate::{
    FacetTrustLockError, FacetWriteError, ObservationLookup, ObservationLookupError,
    ObservationWriteError, add_observation, count_observations, load_observations,
    load_observations_for_query, observation_day_counts, read_facet_entity_observations,
    record_observation_ops, resolve_observation_entity_dir, save_observations,
};

fn three_way_ada(root: &std::path::Path) {
    create_test_facet(root, "work");
    write_journal_entity(root, "dir-ada", Some("effective-ada"));
    write_facet_relationship(
        root,
        "work",
        "legacy-ada",
        json!({"entity_id":"effective-ada"}),
    );
}

#[test]
fn record_ops_keyed_by_entity_id_write_the_relationship_dir() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());

    let counts = record_observation_ops(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"add","content":"from id"})],
        None,
    )
    .unwrap();
    assert_eq!(counts.add, 1);
    assert_eq!(
        load_observations(temporary.path(), "work", "legacy-ada").unwrap()[0]["content"],
        "from id"
    );
    assert!(
        load_observations(temporary.path(), "work", "effective-ada")
            .unwrap()
            .is_empty()
    );
    assert!(
        load_observations(temporary.path(), "work", "dir-ada")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn record_ops_keyed_by_entity_id_update_the_relationship_dir() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());
    save_observations(
        temporary.path(),
        "work",
        "legacy-ada",
        &[json!({"content":"old","observed_at":1})],
    )
    .unwrap();

    let counts = record_observation_ops(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"update","target_index":0,"target_quote":"old","content":"new"})],
        None,
    )
    .unwrap();
    assert_eq!(counts.update, 1);
    assert_eq!(
        load_observations(temporary.path(), "work", "legacy-ada").unwrap()[0]["content"],
        "new"
    );
    assert!(
        !temporary
            .path()
            .join("facets/work/entities/effective-ada/observations.jsonl")
            .exists()
    );
}

#[test]
fn resolver_matches_entity_id_identity_dir_and_relationship_dir() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());
    for query in ["effective-ada", "dir-ada", "legacy-ada"] {
        assert_eq!(
            resolve_observation_entity_dir(temporary.path(), "work", query).unwrap(),
            crate::ObservationEntityResolution::Resolved {
                entity_dir: "legacy-ada".to_owned()
            },
            "{query}"
        );
    }
}

#[test]
fn entity_id_match_wins_when_it_equals_another_relationship_dir() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());
    write_journal_entity(temporary.path(), "dir-b", Some("id-b"));
    write_facet_relationship(
        temporary.path(),
        "work",
        "effective-ada",
        json!({"entity_id":"id-b"}),
    );
    save_observations(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"content":"belongs to b"})],
    )
    .unwrap();

    record_observation_ops(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"add","content":"belongs to a"})],
        None,
    )
    .unwrap();
    assert_eq!(
        load_observations(temporary.path(), "work", "legacy-ada").unwrap()[0]["content"],
        "belongs to a"
    );
    assert_eq!(
        load_observations(temporary.path(), "work", "effective-ada").unwrap()[0]["content"],
        "belongs to b"
    );
}

#[test]
fn resolve_error_does_not_create_a_query_named_directory() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    let entities = temporary.path().join("entities");
    fs::create_dir_all(&entities).unwrap();
    let mut permissions = fs::metadata(&entities).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o000);
        fs::set_permissions(&entities, permissions).unwrap();
    }

    let error = record_observation_ops(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"add","content":"should not land"})],
        None,
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut restore = fs::metadata(&entities).unwrap().permissions();
        restore.set_mode(0o755);
        fs::set_permissions(&entities, restore).unwrap();
    }
    let error = error.unwrap_err();
    assert!(matches!(error, ObservationWriteError::Resolve(_)));
    assert!(
        !temporary
            .path()
            .join("facets/work/entities/effective-ada")
            .exists()
    );
}

#[test]
fn query_lookup_resolves_a_journal_id_to_a_divergent_relationship_directory() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "current_journal_id",
        &json!({"id":"current_journal_id","name":"Renamed Person"}),
        None,
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "work",
        "legacy_label",
        json!({"entity_id":"current_journal_id"}),
    );
    save_observations(
        temporary.path(),
        "work",
        "legacy_label",
        &[json!({"content":"durable"})],
    )
    .unwrap();

    let resolved =
        resolve_observation_entity_dir(temporary.path(), "work", "current_journal_id").unwrap();
    assert_eq!(
        resolved,
        crate::ObservationEntityResolution::Resolved {
            entity_dir: "legacy_label".to_owned()
        }
    );
    assert_eq!(
        load_observations_for_query(temporary.path(), "work", "current_journal_id").unwrap(),
        ObservationLookup::Resolved {
            entity_dir: "legacy_label".to_owned(),
            observations: vec![json!({"content":"durable"})],
        }
    );
}

#[test]
fn resolution_matches_the_resolved_directory_not_the_raw_stored_link_id() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    write_journal_entity(temporary.path(), "canonical_a", Some("shared-effective-id"));
    write_journal_entity(temporary.path(), "other_z", Some("shared-effective-id"));
    assert_eq!(
        solstone_core_entity::read_identity_map(temporary.path())
            .unwrap()
            .resolved
            .get("shared-effective-id"),
        Some(&"canonical_a".to_owned())
    );
    write_facet_relationship(
        temporary.path(),
        "work",
        "relationship-label",
        json!({"entity_id":"shared-effective-id"}),
    );
    save_observations(
        temporary.path(),
        "work",
        "relationship-label",
        &[json!({"content":"resolved through the winner"})],
    )
    .unwrap();

    assert_eq!(
        resolve_observation_entity_dir(temporary.path(), "work", "canonical_a").unwrap(),
        crate::ObservationEntityResolution::Resolved {
            entity_dir: "relationship-label".to_owned(),
        }
    );
    assert_eq!(
        load_observations_for_query(temporary.path(), "work", "canonical_a").unwrap(),
        ObservationLookup::Resolved {
            entity_dir: "relationship-label".to_owned(),
            observations: vec![json!({"content":"resolved through the winner"})],
        }
    );
}

#[test]
fn query_lookup_distinguishes_an_empty_file_from_a_read_failure() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "empty_current",
        &json!({"id":"empty_current","name":"Empty"}),
        None,
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "work",
        "empty_label",
        json!({"entity_id":"empty_current"}),
    );
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "broken_current",
        &json!({"id":"broken_current","name":"Broken"}),
        None,
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "work",
        "broken_label",
        json!({"entity_id":"broken_current"}),
    );
    fs::create_dir_all(
        temporary
            .path()
            .join("facets/work/entities/broken_label/observations.jsonl"),
    )
    .unwrap();

    assert_eq!(
        load_observations_for_query(temporary.path(), "work", "empty_current").unwrap(),
        ObservationLookup::Resolved {
            entity_dir: "empty_label".to_owned(),
            observations: Vec::new(),
        }
    );
    assert!(matches!(
        load_observations_for_query(temporary.path(), "work", "broken_current"),
        Err(ObservationLookupError::Read { entity_dir, .. }) if entity_dir == "broken_label"
    ));
}

#[test]
fn parsed_counts_and_day_counts_share_the_tolerant_reader() {
    let temporary = TempDir::new();
    let path = temporary
        .path()
        .join("facets/work/entities/person/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "{\"content\":\"first\",\"source_day\":\"20260401\"}\n{bad\n\"scalar\"\n{\"content\":\"second\",\"source_day\":\"20260401\"}\n{\"source_day\":\"2026-04-02\"}\n",
    )
    .unwrap();

    assert_eq!(
        load_observations(temporary.path(), "work", "person")
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        count_observations(temporary.path(), "work", "person").unwrap(),
        4
    );
    assert_eq!(
        observation_day_counts(temporary.path(), "work", "person").unwrap(),
        [("20260401".to_owned(), 2)].into()
    );
}

#[test]
fn tolerant_read_preserves_valid_non_objects_and_save_rewrites_only_valid_rows() {
    let temporary = TempDir::new();
    let path = temporary
        .path()
        .join("facets/work/entities/person/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{\"content\":\"kept\"}\n{bad\n[\"also kept\"]\n").unwrap();

    let observations = load_observations(temporary.path(), "work", "person").unwrap();
    assert_eq!(
        observations,
        vec![json!({"content":"kept"}), json!(["also kept"])]
    );
    save_observations(temporary.path(), "work", "person", &observations).unwrap();
    assert_eq!(
        read_facet_entity_observations(temporary.path(), "work", "person").unwrap(),
        Some("{\"content\":\"kept\"}\n[\"also kept\"]\n".to_owned())
    );
}

#[test]
fn add_and_operation_forms_intentionally_differ_for_empty_source_day() {
    let temporary = TempDir::new();
    let (observations, count) = add_observation(
        temporary.path(),
        "work",
        "person",
        "  Added fact  ",
        Some(""),
        Some(&json!({"target_entity_id":"other"})),
    )
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(observations[0]["content"], "Added fact");
    assert!(observations[0].get("source_day").is_none());
    assert_eq!(
        observations[0]["relation"],
        json!({"target_entity_id":"other"})
    );

    let counts = record_observation_ops(
        temporary.path(),
        "work",
        "other",
        &[json!({"op":"add","content":"Operation fact"})],
        Some(""),
    )
    .unwrap();
    assert_eq!(counts.add, 1);
    assert_eq!(
        load_observations(temporary.path(), "work", "other").unwrap()[0]["source_day"],
        ""
    );
}

#[test]
fn quote_less_indexed_operations_are_skipped_and_quoted_operations_use_snapshot_indices() {
    let temporary = TempDir::new();
    save_observations(
        temporary.path(),
        "work",
        "person",
        &[
            json!({"content":"first row","observed_at":1}),
            json!({"content":"second row","observed_at":2}),
            json!({"content":"third row","observed_at":3}),
        ],
    )
    .unwrap();

    let skipped = record_observation_ops(
        temporary.path(),
        "work",
        "person",
        &[json!({"op":"drop","target_index":0})],
        None,
    )
    .unwrap();
    assert_eq!(skipped.skipped, 1);
    assert_eq!(skipped.drop, 0);
    assert_eq!(
        load_observations(temporary.path(), "work", "person")
            .unwrap()
            .len(),
        3
    );

    let counts = record_observation_ops(
        temporary.path(),
        "work",
        "person",
        &[
            json!({"op":"drop","target_index":0,"target_quote":"FIRST"}),
            json!({"op":"update","target_index":2,"target_quote":"third","content":"updated third"}),
            json!({"op":"add","content":"appended"}),
        ],
        Some("20260403"),
    )
    .unwrap();
    assert_eq!(counts.drop, 1);
    assert_eq!(counts.update, 1);
    assert_eq!(counts.add, 1);
    let observations = load_observations(temporary.path(), "work", "person").unwrap();
    assert_eq!(observations[0]["content"], "second row");
    assert_eq!(observations[1]["content"], "updated third");
    assert_eq!(observations[2]["content"], "appended");
}

#[test]
fn dropping_the_last_row_truncates_the_file_without_removing_its_directory() {
    let temporary = TempDir::new();
    save_observations(
        temporary.path(),
        "work",
        "person",
        &[json!({"content":"only row"})],
    )
    .unwrap();

    let counts = record_observation_ops(
        temporary.path(),
        "work",
        "person",
        &[json!({"op":"drop","target_index":0,"target_quote":"only row"})],
        None,
    )
    .unwrap();
    assert_eq!(counts.drop, 1);
    assert_eq!(
        read_facet_entity_observations(temporary.path(), "work", "person").unwrap(),
        Some(String::new())
    );
    assert!(
        temporary
            .path()
            .join("facets/work/entities/person")
            .is_dir()
    );
}

#[test]
fn add_retries_io_but_not_lock_timeout_while_record_retries_both() {
    let mut add_timeout_attempts = 0;
    let add_timeout = retry_add_for_test(|| {
        add_timeout_attempts += 1;
        Err::<(), _>(timeout_error())
    });
    assert!(matches!(
        add_timeout,
        Err(ObservationWriteError::TrustLock(_))
    ));
    assert_eq!(add_timeout_attempts, 1);

    let mut add_io_attempts = 0;
    retry_add_for_test(|| {
        add_io_attempts += 1;
        if add_io_attempts == 1 {
            Err(io_error())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(add_io_attempts, 2);

    let mut record_timeout_attempts = 0;
    retry_record_for_test(|| {
        record_timeout_attempts += 1;
        if record_timeout_attempts < 3 {
            Err(timeout_error())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(record_timeout_attempts, 3);
}

fn timeout_error() -> ObservationWriteError {
    ObservationWriteError::TrustLock(FacetTrustLockError::Lock(LockError::Timeout(LockTimeout {
        path: "observation".into(),
        timeout: Duration::from_millis(1),
    })))
}

fn io_error() -> ObservationWriteError {
    ObservationWriteError::Write(FacetWriteError::ContentWrite(AtomicWriteError::Io {
        path: "observation".into(),
        source: std::io::Error::other("injected"),
    }))
}
