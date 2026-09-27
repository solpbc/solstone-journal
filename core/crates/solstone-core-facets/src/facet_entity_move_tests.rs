// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;
use std::io::Read;

use serde_json::json;

use crate::store_tests::{
    TempDir, create_test_facet, relationship_value, write_facet_relationship,
};
use crate::{move_facet_entity, read_live_observations};

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
    let original = "{\"id\":1,\"content\":\"retained\",\"observed_at\":1000}\n";
    fs::write(&destination, original).unwrap();
    fs::write(
        &source,
        "{\"id\":1,\"content\":\"added\",\"observed_at\":2000}\n",
    )
    .unwrap();
    let mut old_reader = fs::File::open(&destination).unwrap();

    move_facet_entity(temporary.path(), "subject", "from", "to", true).unwrap();

    let mut old_contents = String::new();
    old_reader.read_to_string(&mut old_contents).unwrap();
    assert_eq!(
        old_contents, original,
        "publication must not truncate the previous inode"
    );
    let page = read_live_observations(
        temporary.path(),
        "to",
        "subject",
        crate::ObservationReadQuery {
            order: crate::ObservationReadOrder::Oldest,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].content, "retained");
    assert_eq!(page.items[1].content, "added");
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
    let (observations, ..) = done_rx
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
fn a_move_refused_at_the_destination_leaves_the_source_as_it_was() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "from");
    create_test_facet(temporary.path(), "to");
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "ada",
        &json!({"id": "ada", "name": "Ada"}),
        None,
    )
    .unwrap();
    // Two folders link ada in the source; the destination's `ada` folder
    // holds someone else.
    write_facet_relationship(
        temporary.path(),
        "from",
        "ada_one",
        json!({"entity_id": "ada"}),
    );
    write_facet_relationship(
        temporary.path(),
        "from",
        "ada_two",
        json!({"entity_id": "ada"}),
    );
    write_facet_relationship(
        temporary.path(),
        "to",
        "ada",
        json!({"entity_id": "babbage"}),
    );
    let before = |facet: &str| {
        let mut names: Vec<String> =
            fs::read_dir(temporary.path().join(format!("facets/{facet}/entities")))
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
        names.sort();
        names
    };
    let from_before = before("from");
    let error = move_facet_entity(temporary.path(), "Ada", "from", "to", true).unwrap_err();
    assert!(error.to_string().contains("the 'to' facet"), "{error}");
    assert_eq!(before("from"), from_before);
}

#[test]
fn a_move_into_its_own_facet_is_refused_and_keeps_the_folder() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "ada",
        &json!({"id": "ada", "name": "Ada"}),
        None,
    )
    .unwrap();
    write_facet_relationship(temporary.path(), "work", "ada", json!({"entity_id": "ada"}));
    assert!(move_facet_entity(temporary.path(), "Ada", "work", "work", true).is_err());
    assert!(
        temporary
            .path()
            .join("facets/work/entities/ada/entity.json")
            .exists()
    );
}

#[test]
fn a_move_brings_notes_under_the_entitys_id_that_no_link_claimed() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "from");
    create_test_facet(temporary.path(), "to");
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "ada",
        &json!({"id": "ada", "name": "Ada"}),
        None,
    )
    .unwrap();
    write_facet_relationship(
        temporary.path(),
        "from",
        "ada_l",
        json!({"entity_id": "ada"}),
    );
    let orphan = temporary.path().join("facets/from/entities/ada");
    fs::create_dir_all(&orphan).unwrap();
    fs::write(
        orphan.join("observations.jsonl"),
        "{\"id\":1,\"content\":\"unlinked\",\"observed_at\":1}\n",
    )
    .unwrap();
    move_facet_entity(temporary.path(), "Ada", "from", "to", false).unwrap();
    assert!(!temporary.path().join("facets/from/entities/ada").exists());
    assert!(!temporary.path().join("facets/from/entities/ada_l").exists());
    assert!(
        fs::read_to_string(
            temporary
                .path()
                .join("facets/to/entities/ada/observations.jsonl")
        )
        .unwrap()
        .contains("unlinked")
    );
}

#[cfg(unix)]
#[test]
fn a_move_into_its_own_facet_is_refused_however_the_facet_is_spelled() {
    let temporary = TempDir::new();
    create_test_facet(temporary.path(), "work");
    solstone_core_entity::save_entity_identity(
        temporary.path(),
        "ada",
        &json!({"id": "ada", "name": "Ada"}),
        None,
    )
    .unwrap();
    write_facet_relationship(temporary.path(), "work", "ada", json!({"entity_id": "ada"}));
    std::os::unix::fs::symlink(
        temporary.path().join("facets/work"),
        temporary.path().join("facets/alias"),
    )
    .unwrap();
    for to in ["work/", "./work", "alias"] {
        assert!(
            move_facet_entity(temporary.path(), "Ada", "work", to, true).is_err(),
            "{to}"
        );
        assert!(
            temporary
                .path()
                .join("facets/work/entities/ada/entity.json")
                .exists(),
            "{to}"
        );
    }
}
