// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;
use std::io::Read;

use serde_json::json;

use crate::move_facet_entity;
use crate::store_tests::{
    TempDir, create_test_facet, relationship_value, write_facet_relationship,
};

#[test]
fn move_merge_publishes_complete_observations_without_truncating_an_open_reader() {
    let temporary = TempDir::new();
    for facet in ["from", "to"] {
        create_test_facet(temporary.path(), facet);
        write_facet_relationship(
            temporary.path(),
            facet,
            "subject",
            json!({"entity_id":"id"}),
        );
    }
    let source = temporary
        .path()
        .join("facets/from/entities/subject/observations.jsonl");
    let destination = temporary
        .path()
        .join("facets/to/entities/subject/observations.jsonl");
    let original = "{\"content\":\"retained\"}\nunknown preserved line\n";
    fs::write(&destination, original).unwrap();
    fs::write(&source, "{\"content\":\"added\"}\n").unwrap();
    let mut old_reader = fs::File::open(&destination).unwrap();

    move_facet_entity(temporary.path(), "subject", "from", "to", true).unwrap();

    let mut old_contents = String::new();
    old_reader.read_to_string(&mut old_contents).unwrap();
    assert_eq!(
        old_contents, original,
        "publication must not truncate the previous inode"
    );
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        format!("{original}{{\"content\":\"added\"}}\n")
    );
}

#[test]
fn move_merge_reconciles_link_fields_and_accounts_for_extra_files() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "from");
    create_test_facet(temporary.path(), "to");
    write_facet_relationship(
        temporary.path(),
        "from",
        "subject",
        json!({"entity_id":"id","description":"source","attached_at":"2026-01","updated_at":"2026-03","last_seen":"2026-04","source_only":"yes"}),
    );
    write_facet_relationship(
        temporary.path(),
        "to",
        "subject",
        json!({"entity_id":"id","description":"destination","attached_at":"2026-02","updated_at":"2026-02","last_seen":"2026-02"}),
    );
    let extra = temporary
        .path()
        .join("facets/from/entities/subject/extra.bin");
    fs::write(&extra, b"bytes").unwrap();
    move_facet_entity(temporary.path(), "subject", "from", "to", true).unwrap();
    let relationship = relationship_value(temporary.path(), "to", "subject");
    assert_eq!(relationship["description"], "destination");
    assert_eq!(relationship["attached_at"], "2026-01");
    assert_eq!(relationship["updated_at"], "2026-03");
    assert_eq!(relationship["source_only"], "yes");
    assert_eq!(relationship["last_seen"], "2026-04");
    assert_eq!(
        fs::read(
            temporary
                .path()
                .join("facets/to/entities/subject/extra.bin")
        )
        .unwrap(),
        b"bytes"
    );
    assert!(
        !temporary
            .path()
            .join("facets/from/entities/subject")
            .exists()
    );
}

#[test]
fn move_merge_preserves_unresolved_source_entity_file() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "from");
    create_test_facet(temporary.path(), "to");
    let source = temporary
        .path()
        .join("facets/from/entities/subject/entity.json");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "null\n").unwrap();
    fs::create_dir_all(temporary.path().join("facets/to/entities/subject")).unwrap();

    move_facet_entity(temporary.path(), "subject", "from", "to", true).unwrap();

    assert_eq!(
        fs::read_to_string(
            temporary
                .path()
                .join("facets/to/entities/subject/entity.json")
        )
        .unwrap(),
        "null\n"
    );
    assert!(
        !temporary
            .path()
            .join("facets/from/entities/subject")
            .exists()
    );
}

#[test]
fn move_resolves_a_relationship_directory_that_diverges_from_the_name() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "from");
    create_test_facet(temporary.path(), "to");
    // The entity answers to a name whose derived form is `renamed_person`, but
    // its relationship directory still carries the label it was created under.
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "legacy_label",
        &json!({"id": "legacy_label", "name": "Renamed Person"}),
        None,
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "from",
        "legacy_label",
        json!({"entity_id": "legacy_label", "description": "kept"}),
    );
    let observations = temporary
        .path()
        .join("facets/from/entities/legacy_label/observations.jsonl");
    fs::write(&observations, b"{\"content\":\"noticed\"}\n").unwrap();

    move_facet_entity(temporary.path(), "Renamed Person", "from", "to", false).unwrap();

    assert_eq!(
        relationship_value(temporary.path(), "to", "legacy_label")["description"],
        "kept"
    );
    assert_eq!(
        fs::read(
            temporary
                .path()
                .join("facets/to/entities/legacy_label/observations.jsonl")
        )
        .unwrap(),
        b"{\"content\":\"noticed\"}\n"
    );
    assert!(
        !temporary
            .path()
            .join("facets/from/entities/legacy_label")
            .exists()
    );
}

#[test]
fn observation_writer_waits_for_entity_merge_and_retains_both_changes() {
    use std::sync::mpsc;
    use std::time::Duration;

    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    for id in ["source", "target"] {
        solstone_core_entity::save_entity_identity(
            temporary.path(),
            id,
            &json!({"id":id,"name":id}),
            None,
        )
        .unwrap();
        write_facet_relationship(temporary.path(), "work", id, json!({"entity_id":id}));
        crate::add_observation(
            temporary.path(),
            "work",
            id,
            &format!("{id} memory"),
            None,
            None,
        )
        .unwrap();
    }
    let merge_guard = solstone_core_entity::hold_entity_trust_lock(temporary.path()).unwrap();
    let root = temporary.path().to_owned();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result =
            crate::add_observation(&root, "work", "target", "new owner memory", None, None);
        done_tx.send(result).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        matches!(
            done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "observation writer must share the merge guard"
    );
    let encoder = solstone_core_entity::EncoderIdentity {
        id: "test".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    };
    solstone_core_entity::commit_entity_merge(
        temporary.path(),
        "source",
        "target",
        solstone_core_entity::EntityMergeOptions::default(),
        &encoder,
    )
    .unwrap();
    drop(merge_guard);
    let (observations, _) = done_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    writer.join().unwrap();
    for content in ["source memory", "target memory", "new owner memory"] {
        assert!(
            observations.iter().any(|row| row["content"] == content),
            "missing {content}"
        );
    }
}

#[test]
fn merge_undo_relinks_without_losing_later_observations() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    for id in ["source", "target"] {
        solstone_core_entity::save_entity_identity(
            temporary.path(),
            id,
            &json!({"id":id,"name":id}),
            None,
        )
        .unwrap();
    }
    write_facet_relationship(
        temporary.path(),
        "work",
        "source",
        json!({"entity_id":"source"}),
    );
    crate::add_observation(
        temporary.path(),
        "work",
        "source",
        "original memory",
        None,
        None,
    )
    .unwrap();
    let encoder = solstone_core_entity::EncoderIdentity {
        id: "test".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    };
    let merge = solstone_core_entity::commit_entity_merge(
        temporary.path(),
        "source",
        "target",
        solstone_core_entity::EntityMergeOptions::default(),
        &encoder,
    )
    .unwrap();
    crate::add_observation(
        temporary.path(),
        "work",
        "source",
        "new owner memory",
        None,
        None,
    )
    .unwrap();
    solstone_core_entity::undo_entity_merge(
        temporary.path(),
        &merge.merge_id,
        serde_json::Value::Null,
    )
    .unwrap();
    let (rows, _) =
        crate::add_observation(temporary.path(), "work", "source", "after undo", None, None)
            .unwrap();
    for content in ["original memory", "new owner memory", "after undo"] {
        assert!(
            rows.iter().any(|row| row["content"] == content),
            "missing {content}"
        );
    }
}

#[test]
fn merge_undo_refuses_later_merged_facet_observations_or_metadata() {
    for edit in ["observation", "metadata"] {
        let temporary = TempDir::new();
        create_test_facet(temporary.path(), "work");
        for id in ["source", "target"] {
            solstone_core_entity::save_entity_identity(
                temporary.path(),
                id,
                &json!({"id":id,"name":id}),
                None,
            )
            .unwrap();
            write_facet_relationship(temporary.path(), "work", id, json!({"entity_id":id}));
            crate::add_observation(
                temporary.path(),
                "work",
                id,
                &format!("{id} memory"),
                None,
                None,
            )
            .unwrap();
        }
        let encoder = solstone_core_entity::EncoderIdentity {
            id: "test".to_owned(),
            sha256: "0".repeat(64),
            width: 256,
        };
        let merge = solstone_core_entity::commit_entity_merge(
            temporary.path(),
            "source",
            "target",
            solstone_core_entity::EntityMergeOptions::default(),
            &encoder,
        )
        .unwrap();
        if edit == "observation" {
            crate::add_observation(
                temporary.path(),
                "work",
                "target",
                "new owner memory",
                None,
                None,
            )
            .unwrap();
        } else {
            crate::save_facet_entity_link(
                temporary.path(),
                "work",
                "target",
                "target",
                json!({"description":"new owner edit"}).as_object().unwrap(),
            )
            .unwrap();
        }
        let entities =
            solstone_core_journal_io::capture_snapshot(temporary.path(), "entities").unwrap();
        let facets =
            solstone_core_journal_io::capture_snapshot(temporary.path(), "facets").unwrap();
        let error = solstone_core_entity::undo_entity_merge(
            temporary.path(),
            &merge.merge_id,
            serde_json::Value::Null,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("target facet changed"),
            "{error}"
        );
        assert_eq!(
            solstone_core_journal_io::capture_snapshot(temporary.path(), "entities").unwrap(),
            entities
        );
        assert_eq!(
            solstone_core_journal_io::capture_snapshot(temporary.path(), "facets").unwrap(),
            facets
        );
    }
}
