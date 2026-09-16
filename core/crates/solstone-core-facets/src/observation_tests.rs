// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;
use std::time::Duration;

use serde_json::json;
use solstone_core_journal_io::{AtomicWriteError, LockError, LockTimeout};

use crate::store_tests::{
    TempDir, create_test_facet, write_facet_relationship, write_journal_entity,
};
use crate::{
    FacetTrustLockError, ObservationPageItem, ObservationReadQuery, ObservationStoreError,
    ObservationWriteError, add_observation, observation_day_counts, read_live_observations,
    record_observation_ops_strict, resolve_observation_entity_dir,
};
use solstone_core_entity::{retry_add_for_test, retry_record_for_test};

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

fn write_test_observations(
    root: &std::path::Path,
    facet: &str,
    entity_dir: &str,
    rows: &[serde_json::Value],
) {
    let path = root.join(format!(
        "facets/{facet}/entities/{entity_dir}/observations.jsonl"
    ));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = String::new();
    for (i, row) in rows.iter().enumerate() {
        let mut row_obj = row.as_object().cloned().unwrap_or_default();
        if !row_obj.contains_key("id") {
            row_obj.insert("id".to_owned(), json!(i + 1));
        }
        if !row_obj.contains_key("observed_at") && !row_obj.contains_key("source_day") {
            row_obj.insert("observed_at".to_owned(), json!(1700000000 + i as i64));
        }
        text.push_str(&serde_json::to_string(&row_obj).unwrap());
        text.push('\n');
    }
    fs::write(path, text).unwrap();
}

fn read_test_observations(
    root: &std::path::Path,
    facet: &str,
    entity_dir: &str,
) -> Result<Vec<ObservationPageItem>, ObservationStoreError> {
    read_live_observations(
        root,
        facet,
        entity_dir,
        ObservationReadQuery {
            order: crate::ObservationReadOrder::Oldest,
            ..Default::default()
        },
    )
    .map(|page| page.items)
}

#[test]
fn record_ops_keyed_by_entity_id_write_the_relationship_dir() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());

    let counts = record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"add","content":"from id"})],
        None,
    )
    .unwrap();
    assert_eq!(counts.add, 1);
    assert_eq!(
        read_test_observations(temporary.path(), "work", "legacy-ada").unwrap()[0].content,
        "from id"
    );
    assert!(
        read_test_observations(temporary.path(), "work", "effective-ada")
            .unwrap()
            .is_empty()
    );
    assert!(
        read_test_observations(temporary.path(), "work", "dir-ada")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn record_ops_keyed_by_entity_id_update_the_relationship_dir() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());
    write_test_observations(
        temporary.path(),
        "work",
        "legacy-ada",
        &[json!({"content":"old","observed_at":1})],
    );

    let counts = record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"update","target_index":0,"target_quote":"old","content":"new"})],
        None,
    )
    .unwrap();
    assert_eq!(counts.update, 1);
    assert_eq!(
        read_test_observations(temporary.path(), "work", "legacy-ada").unwrap()[0].content,
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
    write_test_observations(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"content":"belongs to b"})],
    );

    record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"add","content":"belongs to a"})],
        None,
    )
    .unwrap();
    assert_eq!(
        read_test_observations(temporary.path(), "work", "legacy-ada").unwrap()[0].content,
        "belongs to a"
    );
    assert_eq!(
        read_test_observations(temporary.path(), "work", "effective-ada").unwrap()[0].content,
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

    let error = record_observation_ops_strict(
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
    write_test_observations(
        temporary.path(),
        "work",
        "legacy_label",
        &[json!({"content":"durable"})],
    );

    let resolved =
        resolve_observation_entity_dir(temporary.path(), "work", "current_journal_id").unwrap();
    assert_eq!(
        resolved,
        crate::ObservationEntityResolution::Resolved {
            entity_dir: "legacy_label".to_owned()
        }
    );
    let live = read_test_observations(temporary.path(), "work", "legacy_label").unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].content, "durable");
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
    write_test_observations(
        temporary.path(),
        "work",
        "relationship-label",
        &[json!({"content":"resolved through the winner"})],
    );

    assert_eq!(
        resolve_observation_entity_dir(temporary.path(), "work", "canonical_a").unwrap(),
        crate::ObservationEntityResolution::Resolved {
            entity_dir: "relationship-label".to_owned(),
        }
    );
    let live = read_test_observations(temporary.path(), "work", "relationship-label").unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].content, "resolved through the winner");
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
    let broken_path = temporary
        .path()
        .join("facets/work/entities/broken_label/observations.jsonl");
    fs::create_dir_all(broken_path.parent().unwrap()).unwrap();
    fs::write(&broken_path, "{broken json\n").unwrap();

    let empty = read_test_observations(temporary.path(), "work", "empty_label").unwrap();
    assert!(empty.is_empty());

    let err = read_test_observations(temporary.path(), "work", "broken_label").unwrap_err();
    assert!(matches!(
        err,
        ObservationStoreError::MalformedObservation { .. }
    ));
}

#[test]
fn parsed_counts_and_day_counts_strict_reader() {
    let temporary = TempDir::new();
    let path = temporary
        .path()
        .join("facets/work/entities/person/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "{\"id\":1,\"content\":\"first\",\"source_day\":\"20260401\"}\n{\"id\":2,\"content\":\"second\",\"source_day\":\"20260401\"}\n{\"id\":3,\"content\":\"third\",\"source_day\":\"2026-04-02\"}\n",
    )
    .unwrap();

    assert_eq!(
        read_test_observations(temporary.path(), "work", "person")
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        observation_day_counts(temporary.path(), "work", "person").unwrap(),
        [("20260401".to_owned(), 2), ("2026-04-02".to_owned(), 1)].into()
    );
}

#[test]
fn strict_read_refuses_malformed_lines() {
    let temporary = TempDir::new();
    let path = temporary
        .path()
        .join("facets/work/entities/person/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "{\"id\":1,\"content\":\"kept\",\"observed_at\":1000}\n{bad\n",
    )
    .unwrap();

    let err = read_test_observations(temporary.path(), "work", "person").unwrap_err();
    assert!(matches!(
        err,
        ObservationStoreError::MalformedObservation { line: 2, .. }
    ));
}

#[test]
fn record_observation_ops_ignores_malformed_existing_rows() {
    // Flipped to refuse: malformed existing rows abort without modifying the file.
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    let path = temporary
        .path()
        .join("facets/work/entities/person/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "{\"id\":1,\"content\":\"valid\",\"observed_at\":1000}\n{malformed line\n";
    fs::write(&path, original).unwrap();

    let result = record_observation_ops_strict(
        temporary.path(),
        "work",
        "person",
        &[json!({"op":"add","content":"should not be written"})],
        None,
    );

    assert!(matches!(
        result,
        Err(ObservationWriteError::Read(
            ObservationStoreError::MalformedObservation { line: 2, .. }
        ))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn add_and_operation_forms_for_source_day() {
    let temporary = TempDir::new();
    let (observations, count, _) = add_observation(
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
    assert!(observations[0]["source_day"].is_null());
    assert_eq!(
        observations[0]["relation"],
        json!({"target_entity_id":"other"})
    );

    let counts = record_observation_ops_strict(
        temporary.path(),
        "work",
        "other",
        &[json!({"op":"add","content":"Operation fact"})],
        Some(""),
    )
    .unwrap();
    assert_eq!(counts.add, 1);
    assert_eq!(
        read_test_observations(temporary.path(), "work", "other").unwrap()[0].content,
        "Operation fact"
    );
}

#[test]
fn quote_less_indexed_operations_are_skipped_and_quoted_operations_use_snapshot_indices() {
    let temporary = TempDir::new();
    write_test_observations(
        temporary.path(),
        "work",
        "person",
        &[
            json!({"content":"first row","observed_at":1}),
            json!({"content":"second row","observed_at":2}),
            json!({"content":"third row","observed_at":3}),
        ],
    );

    let skipped = record_observation_ops_strict(
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
        read_test_observations(temporary.path(), "work", "person")
            .unwrap()
            .len(),
        3
    );

    let counts = record_observation_ops_strict(
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
    let observations = read_test_observations(temporary.path(), "work", "person").unwrap();
    assert_eq!(observations[0].content, "second row");
    assert_eq!(observations[1].content, "updated third");
    assert_eq!(observations[2].content, "appended");
}

#[test]
fn dropping_the_last_row_truncates_the_file_without_removing_its_directory() {
    let temporary = TempDir::new();
    write_test_observations(
        temporary.path(),
        "work",
        "person",
        &[json!({"content":"only row"})],
    );

    let counts = record_observation_ops_strict(
        temporary.path(),
        "work",
        "person",
        &[json!({"op":"drop","target_index":0,"target_quote":"only row"})],
        None,
    )
    .unwrap();
    assert_eq!(counts.drop, 1);
    assert!(
        read_test_observations(temporary.path(), "work", "person")
            .unwrap()
            .is_empty()
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
    ObservationWriteError::Write(AtomicWriteError::Io {
        path: "observation".into(),
        source: std::io::Error::other("injected"),
    })
}

#[test]
fn repeated_same_day_add_keeps_one_semantically_identical_row() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());

    // First attempt: adds "Content C"
    let counts1 = record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"add","content":"Content C"})],
        Some("20260813"),
    )
    .unwrap();
    assert_eq!(counts1.add, 1);

    // Repeating the same operation deduplicates it against the current owner rows.
    let counts2 = record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[json!({"op":"add","content":"Content C"})],
        Some("20260813"),
    )
    .unwrap();
    assert_eq!(counts2.keep, 1);
    assert_eq!(counts2.add, 0);

    let observations = read_test_observations(temporary.path(), "work", "legacy-ada").unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].content, "Content C");
}

#[test]
fn stale_quoted_update_preserves_intervening_owner_edit() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());

    // Initial state
    write_test_observations(
        temporary.path(),
        "work",
        "legacy-ada",
        &[json!({"content":"Original C","observed_at":1})],
    );

    // Owner edits "Original C" -> "Owner Edited C"
    write_test_observations(
        temporary.path(),
        "work",
        "legacy-ada",
        &[json!({"content":"Owner Edited C","observed_at":2})],
    );

    // Stale update operation with target_quote "Original C" fails with typed Conflict
    let result = record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[
            json!({"op":"update","target_index":0,"target_quote":"Original C","content":"Worker C2"}),
        ],
        Some("20260813"),
    );
    assert!(matches!(
        result,
        Err(ObservationWriteError::Conflict { .. })
    ));

    // Owner state is preserved
    let observations = read_test_observations(temporary.path(), "work", "legacy-ada").unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].content, "Owner Edited C");
}

#[test]
fn test_two_same_day_same_prose_distinct_relations_both_kept() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());

    let counts = record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[
            json!({"op":"add","content":"Met at coffee shop","relation":{"type":"colleague","name":"Bob"}}),
            json!({"op":"add","content":"Met at coffee shop","relation":{"type":"mentor","name":"Alice"}}),
        ],
        Some("20260813"),
    )
    .unwrap();
    assert_eq!(counts.add, 2);

    let observations = read_test_observations(temporary.path(), "work", "legacy-ada").unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(
        observations[0].relation.as_ref().unwrap()["type"],
        "colleague"
    );
    assert_eq!(observations[1].relation.as_ref().unwrap()["type"], "mentor");
}

#[test]
fn test_atomic_drop_old_plus_add_same_content_day_with_new_relation() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());

    write_test_observations(
        temporary.path(),
        "work",
        "legacy-ada",
        &[json!({"content":"Fact X","source_day":"20260813","observed_at":1})],
    );

    let counts = record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[
            json!({"op":"drop","target_index":0,"target_quote":"Fact X"}),
            json!({"op":"add","content":"Fact X","relation":{"type":"updated_rel"}}),
        ],
        Some("20260813"),
    )
    .unwrap();
    assert_eq!(counts.drop, 1);
    assert_eq!(counts.add, 1);

    let observations = read_test_observations(temporary.path(), "work", "legacy-ada").unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].content, "Fact X");
    assert_eq!(
        observations[0].relation.as_ref().unwrap()["type"],
        "updated_rel"
    );
}

#[test]
fn test_update_c_to_c2_plus_add_new_c_both_exist() {
    let temporary = TempDir::new();
    three_way_ada(temporary.path());

    write_test_observations(
        temporary.path(),
        "work",
        "legacy-ada",
        &[json!({"content":"C","observed_at":1})],
    );

    let counts = record_observation_ops_strict(
        temporary.path(),
        "work",
        "effective-ada",
        &[
            json!({"op":"update","target_index":0,"target_quote":"C","content":"C2"}),
            json!({"op":"add","content":"C"}),
        ],
        Some("20260813"),
    )
    .unwrap();
    assert_eq!(counts.update, 1);
    assert_eq!(counts.add, 1);

    let observations = read_test_observations(temporary.path(), "work", "legacy-ada").unwrap();
    assert_eq!(observations.len(), 2);
    assert_eq!(observations[0].content, "C2");
    assert_eq!(observations[1].content, "C");
}

#[test]
fn append_materializes_legacy_ids_and_keeps_them_stable() {
    // The installed base: rows without ids. Their ids are derived on read and
    // must not shift once a write appends a row after them.
    let temporary = TempDir::new();
    let path = temporary
        .path()
        .join("facets/work/entities/person/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        concat!(
            "{\"content\":\"Older legacy fact\",\"observed_at\":1700000000000,\"source_day\":\"20240101\"}\n",
            "{\"content\":\"Newer legacy fact\",\"observed_at\":1700000001000}\n",
        ),
    )
    .unwrap();

    let before = read_test_observations(temporary.path(), "work", "person").unwrap();
    let before_ids: Vec<u64> = before.iter().map(|row| row.id).collect();
    assert_eq!(before_ids, vec![1, 2]);

    let (_, count, _) = add_observation(
        temporary.path(),
        "work",
        "person",
        "Appended fact",
        None,
        None,
    )
    .unwrap();
    assert_eq!(count, 3);

    // Every row on disk now carries an id, and the legacy rows kept theirs.
    let text = fs::read_to_string(&path).unwrap();
    let ids: Vec<u64> = text
        .lines()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["id"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(ids, vec![1, 2, 3]);
    let after = read_test_observations(temporary.path(), "work", "person").unwrap();
    let after_ids: Vec<(u64, String)> = after
        .iter()
        .map(|row| (row.id, row.content.clone()))
        .collect();
    assert_eq!(
        after_ids,
        vec![
            (1, "Older legacy fact".to_owned()),
            (2, "Newer legacy fact".to_owned()),
            (3, "Appended fact".to_owned())
        ]
    );
    // A never-revised legacy row gains only `id`.
    let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["source_day"], "20240101");
    assert!(first.get("by").is_none());
    assert!(first.get("history").is_none());
    assert!(first.get("retired").is_none());
}

#[test]
fn after_id_cursor_walks_ascending_ids_skip_free_on_mixed_files() {
    // Explicit ids, legacy rows (derived above the max), and time order that
    // agrees with neither: an id cursor still visits every live row exactly once.
    let temporary = TempDir::new();
    let path = temporary
        .path()
        .join("facets/work/entities/person/observations.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        concat!(
            "{\"id\":10,\"content\":\"ten\",\"observed_at\":5}\n",
            "{\"content\":\"legacy a\",\"observed_at\":9}\n",
            "{\"id\":2,\"content\":\"two\",\"observed_at\":1}\n",
            "{\"content\":\"legacy b\",\"observed_at\":3}\n",
            "{\"id\":7,\"content\":\"seven\",\"observed_at\":8,\"retired\":{\"at\":8,\"by\":\"model\"}}\n",
        ),
    )
    .unwrap();

    let mut walked: Vec<u64> = Vec::new();
    let mut after_id = 0;
    let mut pages = 0;
    loop {
        let page = read_live_observations(
            temporary.path(),
            "work",
            "person",
            ObservationReadQuery {
                limit: 2,
                after_id: Some(after_id),
                ..Default::default()
            },
        )
        .unwrap();
        pages += 1;
        assert_eq!(page.total, 4, "total counts live rows only");
        assert_eq!(page.offset, 0, "a cursor page carries no offset");
        let ids: Vec<u64> = page.items.iter().map(|row| row.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "each page is ascending by id");
        assert!(ids.iter().all(|id| *id > after_id));
        walked.extend(ids.iter().copied());
        match ids.last() {
            Some(last) if page.has_more => after_id = *last,
            _ => break,
        }
    }
    assert_eq!(pages, 2);
    assert_eq!(
        walked,
        vec![2, 10, 11, 12],
        "legacy rows derive above the explicit max"
    );
    assert!(!walked.contains(&7), "a retired row is never walked");
}
