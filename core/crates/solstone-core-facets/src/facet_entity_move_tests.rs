// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

use std::fs;

use serde_json::json;

use crate::move_facet_entity;
use crate::store_tests::{
    TempDir, create_test_facet, relationship_value, write_facet_relationship,
};

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
