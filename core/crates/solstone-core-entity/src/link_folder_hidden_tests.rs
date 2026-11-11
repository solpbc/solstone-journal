// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One link folder's hide, decided on its own.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::hidden_names::{HideUndecidable, hidden_names_rel, link_folder_hidden};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Journal(PathBuf);

impl Journal {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-link-hidden-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("facets/work/entities")).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn link(&self, folder: &str, link: Value) {
        self.link_bytes(folder, link.to_string().as_bytes());
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

    fn hidden(&self, folder: &str) -> Result<bool, HideUndecidable> {
        link_folder_hidden(&self.0, "work", folder)
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn an_attached_link_hides_only_while_its_entity_is_blocked() {
    let journal = Journal::new();
    journal.link("bob", json!({"entity_id": "bob"}));
    journal.entity("bob", br#"{"id":"bob","name":"Bob","blocked":true}"#);
    assert!(journal.hidden("bob").unwrap());
    journal.entity("bob", br#"{"id":"bob","name":"Bob"}"#);
    assert!(!journal.hidden("bob").unwrap());
    journal.entity("bob", b"{ damaged");
    assert!(!journal.hidden("bob").unwrap());
}

#[test]
fn a_link_the_hide_marked_is_hidden_even_with_a_damaged_entity() {
    let journal = Journal::new();
    journal.link(
        "bob",
        json!({"entity_id": "bob", "detached": true, "detached_by": "hide"}),
    );
    assert!(journal.hidden("bob").unwrap());
    journal.entity("bob", b"{ damaged");
    assert!(journal.hidden("bob").unwrap());
}

#[test]
fn an_unmarked_detached_link_follows_its_entity() {
    let journal = Journal::new();
    journal.link("bob", json!({"entity_id": "bob", "detached": true}));
    assert!(!journal.hidden("bob").unwrap(), "no entity file");
    journal.entity("bob", br#"{"id":"bob","name":"Bob"}"#);
    assert!(!journal.hidden("bob").unwrap(), "visible entity");
    journal.entity("bob", br#"{"id":"bob","name":"Bob","blocked":true}"#);
    assert!(journal.hidden("bob").unwrap(), "blocked entity");
    journal.entity("bob", b"{ damaged");
    assert!(matches!(
        journal.hidden("bob"),
        Err(HideUndecidable::Entity { .. })
    ));
}

#[test]
fn only_this_folders_link_can_make_it_undecidable() {
    let journal = Journal::new();
    journal.link("ann", json!({"entity_id": "ann"}));
    journal.link_bytes("broken", b"{ damaged");
    fs::write(journal.0.join(hidden_names_rel("work")), b"{ damaged").unwrap();
    assert!(!journal.hidden("ann").unwrap());
    assert!(!journal.hidden("nobody").unwrap(), "no link file");
    assert!(matches!(
        journal.hidden("broken"),
        Err(HideUndecidable::Link { .. })
    ));
}
