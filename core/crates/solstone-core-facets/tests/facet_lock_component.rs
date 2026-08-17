// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;

use serde_json::{Map, Value, json};
use solstone_core_entity::{
    EntityOperationContext, EntityOperationKind, hold_entity_trust_lock, save_entity_identity,
};
use solstone_core_facets::{
    EntityDeleteGuardOutcome, EntityHistoryReference, block_journal_entity_with_hook, create_facet,
    delete_created_entity_if_unreferenced_with_hook, delete_journal_entity_with_hook,
};
use solstone_core_indexer_store::db::open_index;

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-facets-lock-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
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

fn write_json(root: &Path, relative: &str, value: &Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn create_test_facet(root: &Path, facet: &str) {
    create_facet(root, facet, facet, "Description", "blue", "💼", None).unwrap();
}

fn write_journal_entity(root: &Path, entity_dir: &str, written_id: Option<&str>) {
    let mut entity = Map::new();
    if let Some(written_id) = written_id {
        entity.insert("id".to_owned(), Value::String(written_id.to_owned()));
    }
    write_json(
        root,
        &format!("entities/{entity_dir}/entity.json"),
        &Value::Object(entity),
    );
}

fn write_facet_relationship(root: &Path, facet: &str, entity_dir: &str, relationship: Value) {
    write_json(
        root,
        &format!("facets/{facet}/entities/{entity_dir}/entity.json"),
        &relationship,
    );
}

fn create_identify_entity(
    root: &Path,
    entity_id: &str,
    operation_id: &str,
) -> (Value, EntityHistoryReference) {
    let identity = json!({"id": entity_id, "name": "Target", "type": "Person"});
    let operation = EntityOperationContext {
        kind: EntityOperationKind::Create,
        caller: Value::Null,
        actor: Value::Null,
        metadata: json!({
            "operation_kind": "speaker_identify",
            "operation_id": operation_id,
        }),
    };
    let event = save_entity_identity(root, entity_id, &identity, Some(&operation))
        .unwrap()
        .event
        .unwrap();
    (
        identity,
        EntityHistoryReference {
            version_id: event["version_id"].as_str().unwrap().to_owned(),
            sequence: event["seq"].as_i64().unwrap().into(),
        },
    )
}

fn deleted_outcome() -> EntityDeleteGuardOutcome {
    EntityDeleteGuardOutcome {
        deleted: true,
        already_gone: false,
        identity_changed: false,
        history_changed: false,
        references: Default::default(),
    }
}

#[test]
fn block_holds_entity_trust_through_relationship_detachment() {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target", Some("target"));
    create_test_facet(temporary.path(), "work");
    for index in 0..300 {
        write_facet_relationship(
            temporary.path(),
            "work",
            &format!("legacy-{index}"),
            json!({"entity_id": "target"}),
        );
    }

    let returned = Arc::new(AtomicBool::new(false));
    let acquired_before_return = Arc::new(AtomicBool::new(false));
    let (hook_reached_sender, hook_reached) = mpsc::channel();
    let (hook_continue, hook_continue_receiver) = mpsc::channel();
    let (contender_waiting_sender, contender_waiting) = mpsc::channel();
    let block_root = temporary.path().to_path_buf();
    let block_returned = Arc::clone(&returned);
    let block = thread::spawn(move || {
        let result = block_journal_entity_with_hook(&block_root, "target", move || {
            hook_reached_sender.send(()).unwrap();
            hook_continue_receiver.recv().unwrap();
        });
        block_returned.store(true, Ordering::SeqCst);
        result
    });

    let contender_root = temporary.path().to_path_buf();
    let contender_returned = Arc::clone(&returned);
    let contender_acquired = Arc::clone(&acquired_before_return);
    let contender = thread::spawn(move || {
        hook_reached.recv().unwrap();
        contender_waiting_sender.send(()).unwrap();
        let _trust = hold_entity_trust_lock(&contender_root).unwrap();
        contender_acquired.store(!contender_returned.load(Ordering::SeqCst), Ordering::SeqCst);
    });

    contender_waiting.recv().unwrap();
    hook_continue.send(()).unwrap();
    block.join().unwrap().unwrap();
    contender.join().unwrap();
    assert!(
        !acquired_before_return.load(Ordering::SeqCst),
        "this catches a naive implementation that releases entity trust after the identity write but before facet writes"
    );
}

#[test]
fn delete_holds_entity_trust_through_relationship_removal() {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target", Some("target"));
    create_test_facet(temporary.path(), "work");
    for index in 0..300 {
        write_facet_relationship(
            temporary.path(),
            "work",
            &format!("legacy-{index}"),
            json!({"entity_id": "target"}),
        );
    }

    let returned = Arc::new(AtomicBool::new(false));
    let acquired_before_return = Arc::new(AtomicBool::new(false));
    let (hook_reached_sender, hook_reached) = mpsc::channel();
    let (hook_continue, hook_continue_receiver) = mpsc::channel();
    let (contender_waiting_sender, contender_waiting) = mpsc::channel();
    let delete_root = temporary.path().to_path_buf();
    let delete_returned = Arc::clone(&returned);
    let delete = thread::spawn(move || {
        let result = delete_journal_entity_with_hook(&delete_root, "target", move || {
            hook_reached_sender.send(()).unwrap();
            hook_continue_receiver.recv().unwrap();
        });
        delete_returned.store(true, Ordering::SeqCst);
        result
    });

    let contender_root = temporary.path().to_path_buf();
    let contender_returned = Arc::clone(&returned);
    let contender_acquired = Arc::clone(&acquired_before_return);
    let contender = thread::spawn(move || {
        hook_reached.recv().unwrap();
        contender_waiting_sender.send(()).unwrap();
        let _trust = hold_entity_trust_lock(&contender_root).unwrap();
        contender_acquired.store(!contender_returned.load(Ordering::SeqCst), Ordering::SeqCst);
    });

    contender_waiting.recv().unwrap();
    hook_continue.send(()).unwrap();
    delete.join().unwrap().unwrap();
    contender.join().unwrap();
    assert!(
        !acquired_before_return.load(Ordering::SeqCst),
        "this catches a naive implementation that releases entity trust before deleting every relationship"
    );
}

#[test]
fn guarded_delete_holds_entity_trust_until_it_returns() {
    let temporary = TempDir::new();
    let (identity, history) = create_identify_entity(temporary.path(), "target", "op-1");
    open_index(temporary.path()).unwrap();

    let returned = Arc::new(AtomicBool::new(false));
    let acquired_before_return = Arc::new(AtomicBool::new(false));
    let (hook_reached_sender, hook_reached) = mpsc::channel();
    let (hook_continue, hook_continue_receiver) = mpsc::channel();
    let (contender_waiting_sender, contender_waiting) = mpsc::channel();
    let delete_root = temporary.path().to_path_buf();
    let delete_returned = Arc::clone(&returned);
    let delete = thread::spawn(move || {
        let result = delete_created_entity_if_unreferenced_with_hook(
            &delete_root,
            "target",
            "op-1",
            &identity,
            &[history],
            move || {
                hook_reached_sender.send(()).unwrap();
                hook_continue_receiver.recv().unwrap();
            },
        );
        delete_returned.store(true, Ordering::SeqCst);
        result
    });

    let contender_root = temporary.path().to_path_buf();
    let contender_returned = Arc::clone(&returned);
    let contender_acquired = Arc::clone(&acquired_before_return);
    let contender = thread::spawn(move || {
        hook_reached.recv().unwrap();
        contender_waiting_sender.send(()).unwrap();
        let _trust = hold_entity_trust_lock(&contender_root).unwrap();
        contender_acquired.store(!contender_returned.load(Ordering::SeqCst), Ordering::SeqCst);
    });

    contender_waiting.recv().unwrap();
    hook_continue.send(()).unwrap();

    assert_eq!(delete.join().unwrap().unwrap(), deleted_outcome());
    contender.join().unwrap();
    assert!(
        !acquired_before_return.load(Ordering::SeqCst),
        "guarded delete must retain entity trust through its nested owner delete"
    );
}
