// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{FacetEntityCreateOutcome, create_facet, create_facet_entity_if_absent};

fn create_test_facet(root: &Path, facet: &str) {
    create_facet(root, facet, facet, "Description", "blue", "💼", None).unwrap();
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn write_identity(root: &Path, entity_dir: &str, identity: Value) {
    write_json(
        root,
        &format!("entities/{entity_dir}/entity.json"),
        &identity,
    );
}

fn write_link(root: &Path, facet: &str, folder: &str, link: Value) {
    write_json(
        root,
        &format!("facets/{facet}/entities/{folder}/entity.json"),
        &link,
    );
}

fn read_json(root: &Path, relative: &str) -> Value {
    serde_json::from_slice(&fs::read(root.join(relative)).unwrap()).unwrap()
}

/// Every directory and file under the journal with its bytes, leaving out the
/// lock files a call takes.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if relative.starts_with("health/locks") {
                continue;
            }
            if path.is_dir() {
                into.insert(relative, None);
                walk(root, &path, into);
            } else {
                into.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

/// Call the create for `name` in `work`, assert it reports `Exists`, and that
/// nothing in the journal changed.
fn assert_exists_and_nothing_written(root: &Path, entity_type: &str, name: &str) {
    let before = snapshot(root);
    let outcome =
        create_facet_entity_if_absent(root, "work", entity_type, name, "named in 3 workouts");
    assert!(
        matches!(outcome, Ok(FacetEntityCreateOutcome::Exists)),
        "{name:?} should already exist, got {outcome:?}"
    );
    assert_eq!(
        snapshot(root),
        before,
        "nothing may be written for {name:?}"
    );
}

#[test]
fn a_name_no_one_has_becomes_a_new_entity_linked_into_the_facet() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_identity(
        root,
        "alex_kim",
        json!({"id":"alex_kim","name":"Alex Kim","type":"Person"}),
    );

    let outcome =
        create_facet_entity_if_absent(root, "work", "Person", "Sam Chen", "named in 3 workouts")
            .unwrap();

    let FacetEntityCreateOutcome::Created {
        entity_id,
        relationship,
    } = outcome
    else {
        panic!("expected a new entity, got {outcome:?}");
    };
    assert_eq!(entity_id, "sam_chen");
    assert_eq!(relationship["entity_id"], "sam_chen");
    let identity = read_json(root, "entities/sam_chen/entity.json");
    assert_eq!(identity["id"], "sam_chen");
    assert_eq!(identity["name"], "Sam Chen");
    assert_eq!(identity["type"], "Person");
    assert!(identity.get("is_principal").is_none());
    let link = read_json(root, "facets/work/entities/sam_chen/entity.json");
    assert_eq!(link["entity_id"], "sam_chen");
    assert_eq!(link["description"], "named in 3 workouts");
    assert!(link.get("detached").is_none());
}

#[test]
fn a_name_attached_in_this_facet_exists() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_identity(
        root,
        "s_chen",
        json!({"id":"s_chen","name":"Sam Chen","type":"Person"}),
    );
    write_link(
        root,
        "work",
        "s_chen",
        json!({"entity_id":"s_chen","description":"a colleague"}),
    );
    assert_exists_and_nothing_written(root, "Person", "Sam Chen");
    assert_exists_and_nothing_written(root, "Person", "sam chen");
}

#[test]
fn a_name_linked_only_in_another_facet_exists() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    create_test_facet(root, "home");
    write_identity(
        root,
        "s_chen",
        json!({"id":"s_chen","name":"Sam Chen","type":"Person"}),
    );
    write_link(
        root,
        "home",
        "s_chen",
        json!({"entity_id":"s_chen","description":"a neighbour"}),
    );
    assert_exists_and_nothing_written(root, "Person", "Sam Chen");
}

#[test]
fn a_name_detached_from_this_facet_exists() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_identity(
        root,
        "s_chen",
        json!({"id":"s_chen","name":"Sam Chen","type":"Person"}),
    );
    write_link(
        root,
        "work",
        "s_chen",
        json!({"entity_id":"s_chen","description":"a colleague","detached":true,"detached_by":"owner"}),
    );
    assert_exists_and_nothing_written(root, "Person", "Sam Chen");
}

#[test]
fn a_detached_entity_keeps_its_type_and_description() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_identity(
        root,
        "domane",
        json!({"id":"domane","name":"Trek Domane","type":"Project"}),
    );
    write_link(
        root,
        "work",
        "domane",
        json!({"entity_id":"domane","description":"the owner's words","detached":true}),
    );

    let outcome =
        create_facet_entity_if_absent(root, "work", "Tool", "Trek Domane", "named in 9 workouts")
            .unwrap();

    assert_eq!(outcome, FacetEntityCreateOutcome::Exists);
    assert_eq!(
        read_json(root, "entities/domane/entity.json")["type"],
        "Project"
    );
    let link = read_json(root, "facets/work/entities/domane/entity.json");
    assert_eq!(link["description"], "the owner's words");
    assert_eq!(link["detached"], true);
}

#[test]
fn a_hidden_name_exists() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_identity(
        root,
        "s_chen",
        json!({"id":"s_chen","name":"Sam Chen","type":"Person","blocked":true}),
    );
    write_link(root, "work", "s_chen", json!({"entity_id":"s_chen"}));
    assert_exists_and_nothing_written(root, "Person", "Sam Chen");
}

#[test]
fn a_name_an_entity_answers_to_by_alias_exists() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_identity(
        root,
        "s_chen",
        json!({"id":"s_chen","name":"Sam Chen","type":"Person","aka":["Sam"]}),
    );
    write_link(root, "work", "s_chen", json!({"entity_id":"s_chen"}));
    assert_exists_and_nothing_written(root, "Person", "Sam");
}

#[test]
fn a_name_tying_two_live_entities_exists() {
    // Two entities with the same name, and one whose alias is another's name.
    for identities in [
        [
            (
                "sam_one",
                json!({"id":"sam_one","name":"Sam","type":"Person"}),
            ),
            (
                "sam_two",
                json!({"id":"sam_two","name":"Sam","type":"Person"}),
            ),
        ],
        [
            (
                "sam_one",
                json!({"id":"sam_one","name":"Sam","type":"Person"}),
            ),
            (
                "s_chen",
                json!({"id":"s_chen","name":"Sam Chen","type":"Person","aka":["Sam"]}),
            ),
        ],
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        create_test_facet(root, "work");
        for (entity_dir, identity) in identities {
            write_identity(root, entity_dir, identity);
        }
        assert_exists_and_nothing_written(root, "Person", "Sam");
    }
}

#[test]
fn an_identity_with_no_name_whose_id_is_the_slug_exists() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_identity(
        root,
        "sam_chen",
        json!({"id":"sam_chen","type":"Person","emails":["sam@example.com"]}),
    );
    assert_exists_and_nothing_written(root, "Person", "Sam Chen");
}

#[test]
fn a_deleted_name_is_not_created_again() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_json(
        root,
        "entities/retired.json",
        &json!({"ids":{"sam_chen":{"state":"deleted","dir":"sam_chen"}}}),
    );
    assert_exists_and_nothing_written(root, "Person", "Sam Chen");
}

#[test]
fn a_merged_name_is_not_created_again() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_identity(
        root,
        "solstone",
        json!({"id":"solstone","name":"Solstone","type":"Project"}),
    );
    write_json(
        root,
        "entities/retired.json",
        &json!({"ids":{"sunstone":{"state":"merged","dir":"sunstone","successor":"solstone"}}}),
    );
    assert_exists_and_nothing_written(root, "Project", "Sunstone");
}

#[test]
fn the_owner_is_never_created_as_a_person() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    create_test_facet(root, "work");
    write_json(
        root,
        "config/journal.json",
        &json!({"identity":{"name":"Jordan Rivera","preferred":"Jo"}}),
    );
    assert_exists_and_nothing_written(root, "Person", "Jordan Rivera");
    assert_exists_and_nothing_written(root, "Person", "jo");
}
