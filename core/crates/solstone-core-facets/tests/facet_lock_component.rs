// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

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

/// Run `operation`, which calls its hook while it holds entity trust, against a
/// contender that takes entity trust itself and reports whether `settled` holds
/// at that moment.
///
/// The hook pauses the operation until the contender has taken the lock and
/// read the journal, or until `CONTENDER_GRACE` passes. An implementation that
/// holds entity trust keeps the contender out, so the hook times out, the
/// operation finishes, and the contender only then sees a settled journal. One
/// that releases it early lets the contender in while the operation is paused
/// mid-work, and the contender sees an unsettled journal. The verdict is read
/// from journal state, never from when a thread returned, so scheduling cannot
/// produce a false failure.
fn contender_sees_settled_state<T: Send + 'static>(
    root: &Path,
    operation: impl FnOnce(Box<dyn FnOnce() + Send>) -> T + Send + 'static,
    settled: impl Fn(&Path) -> bool + Send + 'static,
) -> (T, bool) {
    const CONTENDER_GRACE: Duration = Duration::from_secs(1);
    assert!(
        !settled(root),
        "the settled check must see the journal before the operation as unsettled"
    );
    let (hook_reached_sender, hook_reached) = mpsc::channel();
    let (contender_done_sender, contender_done) = mpsc::channel::<()>();
    let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
        hook_reached_sender.send(()).unwrap();
        let _ = contender_done.recv_timeout(CONTENDER_GRACE);
    });
    let worker = thread::spawn(move || operation(hook));

    let contender_root = root.to_path_buf();
    let contender = thread::spawn(move || {
        hook_reached.recv().unwrap();
        let _trust = hold_entity_trust_lock(&contender_root).unwrap();
        let settled = settled(&contender_root);
        let _ = contender_done_sender.send(());
        settled
    });

    let result = worker.join().unwrap();
    (result, contender.join().unwrap())
}

fn write_target_relationships(root: &Path) {
    for index in 0..300 {
        write_facet_relationship(
            root,
            "work",
            &format!("legacy-{index}"),
            json!({"entity_id": "target"}),
        );
    }
}

fn target_relationships(root: &Path) -> Vec<Value> {
    fs::read_dir(root.join("facets/work/entities"))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| fs::read(entry.path().join("entity.json")).ok())
                .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .filter(|link| link["entity_id"] == "target")
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn block_holds_entity_trust_through_relationship_detachment() {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target", Some("target"));
    create_test_facet(temporary.path(), "work");
    write_target_relationships(temporary.path());

    let root = temporary.path().to_path_buf();
    let (result, settled) = contender_sees_settled_state(
        temporary.path(),
        move |hook| block_journal_entity_with_hook(&root, "target", hook),
        |root| {
            let links = target_relationships(root);
            links.len() == 300 && links.iter().all(|link| link["detached"] == true)
        },
    );
    result.unwrap();
    assert!(
        settled,
        "this catches a naive implementation that releases entity trust after the identity write but before facet writes"
    );
}

#[test]
fn delete_holds_entity_trust_through_relationship_removal() {
    let temporary = TempDir::new();
    write_journal_entity(temporary.path(), "target", Some("target"));
    create_test_facet(temporary.path(), "work");
    write_target_relationships(temporary.path());

    let root = temporary.path().to_path_buf();
    let (result, settled) = contender_sees_settled_state(
        temporary.path(),
        move |hook| delete_journal_entity_with_hook(&root, "target", hook),
        |root| target_relationships(root).is_empty() && !root.join("entities/target").exists(),
    );
    result.unwrap();
    assert!(
        settled,
        "this catches a naive implementation that releases entity trust before deleting every relationship"
    );
}

#[test]
fn guarded_delete_holds_entity_trust_until_it_returns() {
    let temporary = TempDir::new();
    let (identity, history) = create_identify_entity(temporary.path(), "target", "op-1");
    open_index(temporary.path()).unwrap();

    let root = temporary.path().to_path_buf();
    let (result, settled) = contender_sees_settled_state(
        temporary.path(),
        move |hook| {
            delete_created_entity_if_unreferenced_with_hook(
                &root,
                "target",
                "op-1",
                &identity,
                &[history],
                hook,
            )
        },
        |root| !root.join("entities/target").exists(),
    );
    assert_eq!(result.unwrap(), deleted_outcome());
    assert!(
        settled,
        "guarded delete must retain entity trust through its nested owner delete"
    );
}
