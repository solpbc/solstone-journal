// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::hidden_names::{
    HiddenBy, HiddenName, HiddenNames, HideUndecidable, decode_hidden_names, encode_hidden_names,
    facet_hidden_keys, hidden_names_rel, link_folder_hidden, read_hidden_names,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Journal(PathBuf);

impl Journal {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-hidden-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("facets/work/entities")).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn hidden_names(&self, bytes: &[u8]) {
        fs::write(self.0.join(hidden_names_rel("work")), bytes).unwrap();
    }

    fn link(&self, folder: &str, link: Value) {
        let dir = self.0.join("facets/work/entities").join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("entity.json"), link.to_string()).unwrap();
    }

    fn link_bytes(&self, folder: &str, bytes: &[u8]) {
        let dir = self.0.join("facets/work/entities").join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("entity.json"), bytes).unwrap();
    }

    fn entity(&self, id: &str, bytes: &[u8]) {
        let dir = self.0.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("entity.json"), bytes).unwrap();
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hidden(name: &str) -> HiddenName {
    HiddenName {
        name: name.to_owned(),
        hidden_at: "2026-11-11T07:00:00Z".to_owned(),
    }
}

#[test]
fn no_file_reads_as_absent_and_hides_nothing() {
    let journal = Journal::new();
    assert_eq!(read_hidden_names(&journal.0, "work"), HiddenNames::Absent);
    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    assert!(keys.is_empty());
    assert!(!keys.is_hidden("Bob Smith"));
}

#[test]
fn written_names_read_back_in_order_and_match_by_their_key() {
    let journal = Journal::new();
    let names = vec![hidden("Bob Smith"), hidden("Straße Café")];
    journal.hidden_names(&encode_hidden_names(&names));
    assert_eq!(
        read_hidden_names(&journal.0, "work"),
        HiddenNames::Present(names.clone())
    );
    assert_eq!(names[0].key(), "bob smith");

    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    assert_eq!(keys.hidden_by("bob  SMITH"), Some(HiddenBy::Owner));
    assert!(keys.hidden_by_owner("STRASSE CAFÉ"));
    assert!(!keys.is_hidden("Bob"));
    assert!(!keys.is_hidden("Alice"));
}

#[test]
fn a_damaged_file_is_told_apart_from_an_absent_one_and_left_as_it_is() {
    for bytes in [
        &b"{not json"[..],
        br#"["Bob"]"#,
        br#"{"names": "Bob"}"#,
        br#"{"names": [{"name": "Bob", "hidden_at": "x", "key": "bob"}]}"#,
        br#"{"names": [{"name": "  ", "hidden_at": "x"}]}"#,
        br#"{"names": [{"name": "Bob"}]}"#,
        br#"{"names": [], "version": 2}"#,
    ] {
        let journal = Journal::new();
        journal.hidden_names(bytes);
        assert!(
            matches!(
                read_hidden_names(&journal.0, "work"),
                HiddenNames::Damaged { .. }
            ),
            "{}",
            String::from_utf8_lossy(bytes)
        );
        assert!(matches!(
            facet_hidden_keys(&journal.0, "work"),
            Err(HideUndecidable::HiddenNames { .. })
        ));
        assert_eq!(
            fs::read(journal.0.join(hidden_names_rel("work"))).unwrap(),
            bytes
        );
        assert!(decode_hidden_names(bytes).is_err());
    }
}

#[test]
fn an_empty_file_holds_no_names() {
    let journal = Journal::new();
    journal.hidden_names(b"");
    assert_eq!(read_hidden_names(&journal.0, "work"), HiddenNames::Absent);
    assert_eq!(decode_hidden_names(b"  \n").unwrap(), Vec::new());
}

#[test]
fn a_hide_marked_link_hides_its_entitys_keys_and_a_visible_link_does_not() {
    let journal = Journal::new();
    journal.entity(
        "robert_smith",
        json!({
            "id": "robert_smith",
            "name": "Robert Smith",
            "aka": ["Bobby"],
            "emails": ["bob@example.com"],
            "blocked": true,
        })
        .to_string()
        .as_bytes(),
    );
    journal.link(
        "robert_smith",
        json!({"entity_id": "robert_smith", "detached": true, "detached_by": "hide"}),
    );
    journal.entity(
        "alice",
        json!({"id": "alice", "name": "Alice"})
            .to_string()
            .as_bytes(),
    );
    journal.link("alice", json!({"entity_id": "alice"}));

    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    for name in [
        "Robert Smith",
        "robert smith",
        "Bobby",
        "bob@example.com",
        "robert_smith",
    ] {
        let Some(HiddenBy::Entity(entity)) = keys.hidden_by(name) else {
            panic!("{name} should be hidden by the entity");
        };
        assert_eq!(entity.id, "robert_smith");
        assert_eq!(entity.name, "Robert Smith");
        assert_eq!(entity.folder, "robert_smith");
    }
    assert!(keys.hides_folder("robert_smith"));
    assert!(!keys.hides_folder("alice"));
    assert!(!keys.is_hidden("Alice"));
    assert!(!keys.is_hidden("Robert"));
}

#[test]
fn an_attached_link_hides_only_when_its_entity_is_blocked() {
    let journal = Journal::new();
    // A block interrupted before it detached this link, or a hidden entity an
    // archive brought in attached.
    journal.entity(
        "dana",
        json!({"id": "dana", "name": "Dana", "blocked": true})
            .to_string()
            .as_bytes(),
    );
    journal.link("dana", json!({"entity_id": "dana"}));
    // Twin: an attached visible entity stays visible.
    journal.entity(
        "eve",
        json!({"id": "eve", "name": "Eve"}).to_string().as_bytes(),
    );
    journal.link("eve", json!({"entity_id": "eve"}));
    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    assert!(keys.is_hidden("Dana"));
    assert!(keys.hides_folder("dana"));
    assert!(!keys.is_hidden("Eve"));
    assert!(link_folder_hidden(&journal.0, "work", "dana").unwrap());
    assert!(!link_folder_hidden(&journal.0, "work", "eve").unwrap());
}

#[test]
fn an_attached_link_whose_entity_is_damaged_stays_visible() {
    let journal = Journal::new();
    journal.entity("mark", b"{broken");
    journal.link("mark", json!({"entity_id": "mark"}));
    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    assert!(keys.is_empty());
    assert!(!link_folder_hidden(&journal.0, "work", "mark").unwrap());
}

#[test]
fn a_detached_link_without_the_mark_hides_only_a_blocked_entity() {
    let journal = Journal::new();
    journal.entity(
        "dana",
        json!({"id": "dana", "name": "Dana Ruiz", "aka": ["DR"], "blocked": true})
            .to_string()
            .as_bytes(),
    );
    journal.link("dana", json!({"entity_id": "dana", "detached": true}));
    journal.entity(
        "eve",
        json!({"id": "eve", "name": "Eve"}).to_string().as_bytes(),
    );
    journal.link(
        "eve",
        json!({"entity_id": "eve", "detached": true, "detached_by": "owner"}),
    );
    journal.link("gone", json!({"entity_id": "gone", "detached": true}));
    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    let entity = keys.hidden_by_entity("dana ruiz").expect("hidden by Dana");
    assert_eq!(entity.id, "dana");
    assert!(keys.is_hidden("DR"));
    assert!(keys.hides_folder("dana"));
    assert!(!keys.is_hidden("Eve"));
    assert!(!keys.hides_folder("eve"));
    assert!(!keys.is_hidden("gone"));
    assert_eq!(keys.entities().len(), 1);
}

#[test]
fn a_detached_link_without_the_mark_whose_entity_is_damaged_is_undecidable() {
    let journal = Journal::new();
    journal.entity("eve", b"{broken");
    journal.link(
        "eve",
        json!({"entity_id": "eve", "detached": true, "detached_by": "owner"}),
    );
    let error = facet_hidden_keys(&journal.0, "work").unwrap_err();
    assert!(matches!(error, HideUndecidable::Entity { .. }), "{error}");
    assert!(error.path().ends_with("entities/eve/entity.json"));
}

#[test]
fn a_hide_marked_link_without_an_entity_hides_by_its_own_id_and_name() {
    let journal = Journal::new();
    journal.link(
        "ghost",
        json!({"entity_id": "ghost", "name": "Ghost Writer", "detached": true, "detached_by": "hide"}),
    );
    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    let entity = keys
        .hidden_by_entity("ghost writer")
        .expect("hidden by the link");
    assert_eq!(entity.id, "ghost");
    assert_eq!(entity.name, "Ghost Writer");
    assert!(keys.is_hidden("ghost"));
    assert!(!keys.is_hidden("Writer"));
}

#[test]
fn a_hide_marked_link_whose_entity_is_damaged_is_undecidable() {
    let journal = Journal::new();
    journal.entity("bob", b"{broken");
    journal.link(
        "bob",
        json!({"entity_id": "bob", "detached": true, "detached_by": "hide"}),
    );
    let error = facet_hidden_keys(&journal.0, "work").unwrap_err();
    assert!(matches!(error, HideUndecidable::Entity { .. }), "{error}");
    assert!(error.path().ends_with("entities/bob/entity.json"));
    assert_eq!(
        fs::read(journal.0.join("entities/bob/entity.json")).unwrap(),
        b"{broken"
    );
}

#[test]
fn a_visible_persons_damaged_entity_leaves_the_facet_decidable() {
    let journal = Journal::new();
    journal.entity("carol", b"{broken");
    journal.link("carol", json!({"entity_id": "carol"}));
    journal.hidden_names(&encode_hidden_names(&[hidden("Bob")]));
    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    assert!(keys.is_hidden("bob"));
    assert!(!keys.is_hidden("carol"));
}

#[test]
fn an_unreadable_link_file_is_undecidable() {
    let journal = Journal::new();
    journal.link_bytes("frank", b"[not a link");
    let error = facet_hidden_keys(&journal.0, "work").unwrap_err();
    assert!(matches!(error, HideUndecidable::Link { .. }), "{error}");
    assert!(
        error
            .path()
            .ends_with("facets/work/entities/frank/entity.json")
    );
}

#[test]
fn a_facet_without_an_entities_folder_hides_only_the_owners_names() {
    let journal = Journal::new();
    fs::remove_dir_all(journal.0.join("facets/work/entities")).unwrap();
    journal.hidden_names(&encode_hidden_names(&[hidden("Bob")]));
    let keys = facet_hidden_keys(&journal.0, "work").unwrap();
    assert!(keys.is_hidden("BOB"));
    assert!(keys.entities().is_empty());
}

#[test]
fn a_hidden_entitys_names_are_not_rewritten_and_a_visible_ones_are() {
    let journal = Journal::new();
    let root = &journal.0;
    for (id, blocked) in [("dana", true), ("eve", false)] {
        crate::save_entity_identity(
            root,
            id,
            &json!({"id": id, "name": id, "type": "Person", "blocked": blocked, "aka": ["one"]}),
            None,
        )
        .unwrap();
    }
    let change = |id: &str, field: &str, value: Value| {
        let mut identity = crate::read_entity_identity(root, id)
            .unwrap()
            .unwrap()
            .value()
            .clone();
        identity[field] = value;
        crate::save_entity_identity(root, id, &identity, None)
    };
    for (field, value) in [
        ("name", json!("Dana Ruiz")),
        ("aka", json!(["one", "two"])),
        ("emails", json!(["dana@example.com"])),
    ] {
        assert!(
            matches!(
                change("dana", field, value.clone()),
                Err(crate::EntityWriteError::EntityHidden { .. })
            ),
            "{field}"
        );
        change("eve", field, value).unwrap();
    }
    // Anything else, the same names in another order, and unhiding still write.
    change("dana", "description", json!("a note")).unwrap();
    change("dana", "aka", json!(["one"])).unwrap();
    change("dana", "blocked", json!(false)).unwrap();
    change("dana", "name", json!("Dana Ruiz")).unwrap();
}

#[test]
fn a_prepared_name_change_to_an_entity_hidden_since_is_refused() {
    let journal = Journal::new();
    let root = &journal.0;
    crate::save_entity_identity(
        root,
        "dana",
        &json!({"id": "dana", "name": "Dana", "type": "Person"}),
        None,
    )
    .unwrap();
    let before = crate::read_entity_identity(root, "dana")
        .unwrap()
        .unwrap()
        .value()
        .clone();
    let mut renamed = before.clone();
    renamed["aka"] = json!(["DR"]);
    let changes = crate::prepare_identity_changes(root, "dana", &[renamed]).unwrap();
    let mut hidden = before.clone();
    hidden["blocked"] = json!(true);
    crate::save_entity_identity(root, "dana", &hidden, None).unwrap();
    let mut change = changes[0].clone();
    change.before = Some(hidden.clone());
    change.after["blocked"] = json!(true);
    let error =
        crate::publish_identity_change(root, &change, true, || Ok(()), || Ok(())).unwrap_err();
    assert!(
        matches!(
            error,
            crate::ReviewOwnerError::Conflict {
                kind: crate::ReviewOwnerConflictKind::IdentityBlocked,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        crate::read_entity_identity(root, "dana")
            .unwrap()
            .unwrap()
            .value()
            .get("aka"),
        None
    );
}
