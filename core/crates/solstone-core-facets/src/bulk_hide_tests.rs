// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_entity::{
    entity_is_hidden, read_entity_identity, read_identity_map, read_prepared_history,
    read_visible_history, save_entity_identity,
};

use crate::{
    BulkJournalError, FailedEntity, StoppedReport, block_journal_entities, block_journal_entity,
    create_facet, test_hooks, unblock_journal_entity, unblock_journal_operation,
};

fn temp_journal() -> tempfile::TempDir {
    tempfile::tempdir_in("/var/tmp").unwrap()
}

fn write_entity(root: &Path, entity_dir: &str, fields: Value) {
    let folder = root.join("entities").join(entity_dir);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("entity.json"),
        serde_json::to_vec_pretty(&fields).unwrap(),
    )
    .unwrap();
}

fn write_rel(root: &Path, facet: &str, entity_dir: &str, value: Value) {
    let folder = root
        .join("facets")
        .join(facet)
        .join("entities")
        .join(entity_dir);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("entity.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

fn read_rel(root: &Path, facet: &str, entity_dir: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(
            root.join("facets")
                .join(facet)
                .join("entities")
                .join(entity_dir)
                .join("entity.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

// 1. Batch of ids where the first is duplicated, one is already hidden with an attached relationship, one is absent.
#[test]
fn bulk_test_01_batch_block_dedup_and_already_hidden_and_twin_match() {
    let j1 = temp_journal();
    let j2 = temp_journal();

    create_facet(j1.path(), "work", "work", "", "blue", "💼", None).unwrap();
    create_facet(j1.path(), "home", "home", "", "green", "🏠", None).unwrap();
    create_facet(j2.path(), "work", "work", "", "blue", "💼", None).unwrap();
    create_facet(j2.path(), "home", "home", "", "green", "🏠", None).unwrap();

    let e_dup = "e-dup";
    let e_hid = "e-hid";
    let e_absent = "e-absent";

    // Setup e_dup on j1 and j2
    for root in [j1.path(), j2.path()] {
        write_entity(root, e_dup, json!({"id": e_dup, "name": "Duplicate"}));
        write_rel(
            root,
            "work",
            "rel_work_dup",
            json!({"entity_id": e_dup, "role": "engineer"}),
        );
        write_rel(
            root,
            "home",
            "rel_home_dup",
            json!({"entity_id": e_dup, "detached": true, "detached_by": "stale", "note": "home"}),
        );
    }

    // Setup e_hid on j1: already hidden under a prior operation
    write_entity(
        j1.path(),
        e_hid,
        json!({"id": e_hid, "name": "Already Hidden"}),
    );
    save_entity_identity(
        j1.path(),
        e_hid,
        &json!({"id": e_hid, "blocked": true, "name": "Already Hidden"}),
        None,
    )
    .unwrap();
    write_rel(
        j1.path(),
        "work",
        "rel_work_hid",
        json!({"entity_id": e_hid, "role": "hidden_guy"}),
    );

    let hid_identity_bytes_before =
        fs::read(j1.path().join("entities").join(e_hid).join("entity.json")).unwrap();
    let hid_history_files_before: Vec<_> = fs::read_dir(
        j1.path()
            .join("entities")
            .join(e_hid)
            .join("history/events"),
    )
    .unwrap()
    .map(|e| e.unwrap().file_name())
    .collect();

    // Single block on twin j2
    let single_report = block_journal_entity(j2.path(), e_dup).unwrap();
    assert_eq!(single_report.facets_detached, vec!["work"]);

    // Batch block on j1
    let report = block_journal_entities(
        j1.path(),
        "bulk-accept-a",
        &[
            e_dup.to_owned(),
            e_dup.to_owned(),
            e_hid.to_owned(),
            e_absent.to_owned(),
        ],
    )
    .unwrap();

    assert_eq!(report.hidden, vec![e_dup]);
    assert_eq!(report.already_hidden, vec![e_hid]);
    assert_eq!(report.missing, vec![e_absent]);
    assert_eq!(report.unreadable, vec![]);
    assert_eq!(report.stopped, None);

    // Twin verification for e_dup
    let rel_work_j1 = read_rel(j1.path(), "work", "rel_work_dup");
    let rel_work_j2 = read_rel(j2.path(), "work", "rel_work_dup");
    assert_eq!(rel_work_j1, rel_work_j2);
    assert_eq!(rel_work_j1["detached"], true);
    assert_eq!(rel_work_j1["detached_by"], "hide");
    assert_eq!(rel_work_j1["role"], "engineer");

    // Home link lost stale detached_by
    let rel_home_j1 = read_rel(j1.path(), "home", "rel_home_dup");
    assert_eq!(rel_home_j1["detached"], true);
    assert!(rel_home_j1.get("detached_by").is_none());
    assert_eq!(rel_home_j1["note"], "home");

    // New event is kind update with operation.bulk_operation
    let events = read_visible_history(j1.path(), e_dup).unwrap();
    let last_ev = events.last().unwrap();
    assert_eq!(last_ev.value()["kind"], "update");
    assert_eq!(
        last_ev.value()["operation"]["bulk_operation"],
        "bulk-accept-a"
    );

    // Already hidden entity and history bytes unchanged
    let hid_identity_bytes_after =
        fs::read(j1.path().join("entities").join(e_hid).join("entity.json")).unwrap();
    let hid_history_files_after: Vec<_> = fs::read_dir(
        j1.path()
            .join("entities")
            .join(e_hid)
            .join("history/events"),
    )
    .unwrap()
    .map(|e| e.unwrap().file_name())
    .collect();
    assert_eq!(hid_identity_bytes_before, hid_identity_bytes_after);
    assert_eq!(hid_history_files_before, hid_history_files_after);
}

// 2. Principal, and separately a stray principal flag: nothing written, principal_entity_protected.
// One unparseable relationship file anywhere: nothing written, entity_record_damaged path.
// 51 ids refused. Operation id of 7 characters refused. Operation id containing a space refused.
#[test]
fn bulk_test_02_refusals_principal_damaged_lengths() {
    let j = temp_journal();
    create_facet(j.path(), "work", "work", "", "blue", "💼", None).unwrap();

    // Principal
    write_entity(
        j.path(),
        "p1",
        json!({"id": "p1", "is_principal": true, "type": "Person"}),
    );
    let err = block_journal_entities(j.path(), "bulk-accept-a", &["p1".to_owned()]).unwrap_err();
    assert!(
        matches!(err, BulkJournalError::PrincipalEntityProtected { entity_id } if entity_id == "p1")
    );

    // Stray principal flag
    write_entity(
        j.path(),
        "p2",
        json!({"id": "p2", "is_principal": true, "type": "Thing"}),
    );
    let err2 = block_journal_entities(j.path(), "bulk-accept-a", &["p2".to_owned()]).unwrap_err();
    assert!(
        matches!(err2, BulkJournalError::PrincipalEntityProtected { entity_id } if entity_id == "p2")
    );

    // Damaged relationship file
    let damaged_rel_dir = j.path().join("facets/work/entities/rel_bad");
    fs::create_dir_all(&damaged_rel_dir).unwrap();
    fs::write(damaged_rel_dir.join("entity.json"), b"{ invalid json").unwrap();
    write_entity(j.path(), "valid_e", json!({"id": "valid_e"}));

    let err3 =
        block_journal_entities(j.path(), "bulk-accept-a", &["valid_e".to_owned()]).unwrap_err();
    assert!(
        matches!(err3, BulkJournalError::RelationshipsUnreadable { ref path } if path == "facets/work/entities/rel_bad/entity.json")
    );

    // Clean up damaged rel
    fs::remove_dir_all(&damaged_rel_dir).unwrap();

    // 51 ids refused
    let ids_51: Vec<String> = (0..51).map(|i| format!("id_{i}")).collect();
    let err51 = block_journal_entities(j.path(), "bulk-accept-a", &ids_51).unwrap_err();
    assert!(matches!(err51, BulkJournalError::InvalidIdsCount(51)));

    // 7 characters op id refused
    let err7 = block_journal_entities(j.path(), "short_7", &["valid_e".to_owned()]).unwrap_err();
    assert!(matches!(err7, BulkJournalError::InvalidOperationId(_)));

    // Space in op id refused
    let err_space =
        block_journal_entities(j.path(), "bad op id", &["valid_e".to_owned()]).unwrap_err();
    assert!(matches!(err_space, BulkJournalError::InvalidOperationId(_)));
}

// 3. Stop at entity k's relationship write after k's identity write. 1..k-1 hidden and formerly attached links marked.
// k not in hidden. stopped.id is k. k+1..n byte-unchanged. inspect on that journal contains @entity-search.
// Retry marks k's still-attached links, hides the rest, reports 1..n in hidden, and writes no new history file for 1..k.
// Second fixture: k's identity already blocked and only a prepared hide event for this operation.
// Retry publishes that event, marks relationships, does not put k in already_hidden. Undo then unhides k.
#[test]
fn bulk_test_03_stop_at_k_relationship_write_and_retry_and_prepared_event() {
    let j = temp_journal();
    create_facet(j.path(), "work", "work", "", "blue", "💼", None).unwrap();

    let e1 = "e-stop-1";
    let e2 = "e-stop-2"; // k
    let e3 = "e-stop-3";

    for id in [e1, e2, e3] {
        write_entity(j.path(), id, json!({"id": id, "name": id}));
        write_rel(
            j.path(),
            "work",
            &format!("rel_{id}"),
            json!({"entity_id": id, "role": "member"}),
        );
    }

    let e3_bytes_before = fs::read(j.path().join("entities").join(e3).join("entity.json")).unwrap();

    let guard = test_hooks::arm_fail_batch_relationship(e2);
    let report = block_journal_entities(
        j.path(),
        "bulk-accept-3",
        &[e1.to_owned(), e2.to_owned(), e3.to_owned()],
    )
    .unwrap();
    drop(guard);

    assert_eq!(report.hidden, vec![e1]);
    assert_eq!(
        report.stopped,
        Some(StoppedReport {
            reason: "relationship_write_failed".to_owned(),
            id: Some(e2.to_owned()),
        })
    );

    // e3 byte-unchanged
    let e3_bytes_after = fs::read(j.path().join("entities").join(e3).join("entity.json")).unwrap();
    assert_eq!(e3_bytes_before, e3_bytes_after);

    // inspect on that journal contains @entity-search
    let todo_status = solstone_core_indexer_todo::inspect(j.path()).unwrap();
    assert!(todo_status.keys.iter().any(|k| k == "@entity-search"));

    // Record filenames for e1 and e2
    let e1_hist_files: Vec<_> =
        fs::read_dir(j.path().join("entities").join(e1).join("history/events"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
    let e2_hist_files: Vec<_> =
        fs::read_dir(j.path().join("entities").join(e2).join("history/events"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();

    // Retry marks k's still-attached links, hides the rest, reports 1..n in hidden
    let report2 = block_journal_entities(
        j.path(),
        "bulk-accept-3",
        &[e1.to_owned(), e2.to_owned(), e3.to_owned()],
    )
    .unwrap();

    assert_eq!(report2.hidden, vec![e1, e2, e3]);
    assert_eq!(report2.stopped, None);

    let rel_e2 = read_rel(j.path(), "work", &format!("rel_{e2}"));
    assert_eq!(rel_e2["detached"], true);
    assert_eq!(rel_e2["detached_by"], "hide");

    // No new history file for 1..k (e1 and e2)
    let e1_hist_after: Vec<_> =
        fs::read_dir(j.path().join("entities").join(e1).join("history/events"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
    let e2_hist_after: Vec<_> =
        fs::read_dir(j.path().join("entities").join(e2).join("history/events"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
    assert_eq!(e1_hist_files, e1_hist_after);
    assert_eq!(e2_hist_files, e2_hist_after);

    // Second fixture: k's identity already blocked and only a prepared hide event for this operation.
    let j2 = temp_journal();
    create_facet(j2.path(), "work", "work", "", "blue", "💼", None).unwrap();
    let k_id = "k-prep-fixture";
    write_entity(
        j2.path(),
        k_id,
        json!({"id": k_id, "blocked": true, "name": "K Prep"}),
    );
    write_rel(
        j2.path(),
        "work",
        "rel_k",
        json!({"entity_id": k_id, "role": "prepared"}),
    );

    // Stage prepared hide event
    let events_dir = j2.path().join("entities").join(k_id).join("history/events");
    fs::create_dir_all(&events_dir).unwrap();
    let prep_dir = j2
        .path()
        .join("entities")
        .join(k_id)
        .join("history/prepared/staging_prep_k");
    fs::create_dir_all(&prep_dir).unwrap();
    let prep_event = json!({
        "schema_version": 1,
        "version_id": "vh_prepk",
        "seq": 1,
        "ts": "2026-10-22T00:00:00Z",
        "entity_id": k_id,
        "entity_dir": k_id,
        "kind": "update",
        "identity_before": { "id": k_id, "name": "K Prep" },
        "identity_after": { "id": k_id, "blocked": true, "name": "K Prep" },
        "operation": {
            "kind": "update",
            "caller": null,
            "actor": null,
            "bulk_operation": "bulk-accept-prep",
            "metadata": { "bulk_operation": "bulk-accept-prep" }
        }
    });
    fs::write(
        prep_dir.join("event.json"),
        serde_json::to_vec_pretty(&prep_event).unwrap(),
    )
    .unwrap();

    let retry_rep =
        block_journal_entities(j2.path(), "bulk-accept-prep", &[k_id.to_owned()]).unwrap();
    assert_eq!(retry_rep.hidden, vec![k_id]);
    assert!(retry_rep.already_hidden.is_empty());

    let rel_k = read_rel(j2.path(), "work", "rel_k");
    assert_eq!(rel_k["detached"], true);
    assert_eq!(rel_k["detached_by"], "hide");

    // Undo then unhides k
    let undo_rep = unblock_journal_operation(j2.path(), "bulk-accept-prep", 10).unwrap();
    assert_eq!(undo_rep.restored.len(), 1);
    assert_eq!(undo_rep.restored[0].id, k_id);
    let identity = read_entity_identity(j2.path(), k_id).unwrap().unwrap();
    assert!(!solstone_core_entity::entity_is_hidden(identity.value()));
}

// 4. Two batches under one operation, one response ignored. Undo restores every entity either request hid,
// reattaches only hide-marked links, and remaining reaches 0 across calls with limit in 1..50.
// Hidden, then renamed, then single-blocked again: undo unhides it.
// Hidden, unhidden by hand, then single-blocked: undo does not touch it.
// An entity hidden under a second operation is not touched.
// After undo has started, a batch under the same id writes nothing and returns operation_undone.
#[test]
fn bulk_test_04_multiple_batches_undo_renamed_unhidden_undone_set() {
    let j = temp_journal();
    create_facet(j.path(), "work", "work", "", "blue", "💼", None).unwrap();

    let op = "bulk-accept-four";
    let e1 = "e-four-1";
    let e2 = "e-four-2";
    let e3 = "e-four-3";
    let e4 = "e-four-4";
    let e_sec = "e-four-sec";

    for id in [e1, e2, e3, e4, e_sec] {
        write_entity(j.path(), id, json!({"id": id, "name": id}));
        write_rel(
            j.path(),
            "work",
            &format!("rel_{id}"),
            json!({"entity_id": id, "role": "user"}),
        );
    }

    // Batch 1: e1, e2
    block_journal_entities(j.path(), op, &[e1.to_owned(), e2.to_owned()]).unwrap();
    // Batch 2: e3, e4
    block_journal_entities(j.path(), op, &[e3.to_owned(), e4.to_owned()]).unwrap();

    // Hidden, then renamed, then single-blocked again: undo unhides it
    // Save rename on e1
    let mut e1_renamed = read_entity_identity(j.path(), e1)
        .unwrap()
        .unwrap()
        .value()
        .clone();
    e1_renamed["name"] = json!("e1 renamed");
    save_entity_identity(j.path(), e1, &e1_renamed, None).unwrap();
    // Single block again
    block_journal_entity(j.path(), e1).unwrap();

    // Hidden, unhidden by hand, then single-blocked: undo does not touch it
    unblock_journal_entity(j.path(), e2).unwrap();
    block_journal_entity(j.path(), e2).unwrap();

    // Second operation for e_sec
    block_journal_entities(j.path(), "bulk-accept-sec", &[e_sec.to_owned()]).unwrap();

    // Undo begins: limit 1
    let undo1 = unblock_journal_operation(j.path(), op, 1).unwrap();
    assert_eq!(undo1.restored.len(), 1);

    // After undo has started, a batch under the same id writes nothing and returns operation_undone
    let refuse = block_journal_entities(j.path(), op, &["new_id".to_owned()]).unwrap_err();
    assert!(matches!(refuse, BulkJournalError::OperationUndone(ref o) if o == op));

    // Next undo calls until remaining reaches 0
    let undo2 = unblock_journal_operation(j.path(), op, 50).unwrap();
    assert_eq!(undo2.remaining, 0);

    // Total restored across undo1 and undo2: e1, e3, e4 (3 entities)
    let total_restored: Vec<String> = undo1
        .restored
        .into_iter()
        .chain(undo2.restored)
        .map(|r| r.id)
        .collect();
    assert_eq!(total_restored.len(), 3);
    assert!(total_restored.contains(&e1.to_owned()));
    assert!(total_restored.contains(&e3.to_owned()));
    assert!(total_restored.contains(&e4.to_owned()));

    // e2 not touched (still blocked)
    let e2_id = read_entity_identity(j.path(), e2).unwrap().unwrap();
    assert!(solstone_core_entity::entity_is_hidden(e2_id.value()));

    // e_sec not touched (still blocked)
    let e_sec_id = read_entity_identity(j.path(), e_sec).unwrap().unwrap();
    assert!(solstone_core_entity::entity_is_hidden(e_sec_id.value()));
}

// 5. Journal-wide failure mid-undo: restored entries stay, remaining includes unfinished members, stopped has no id.
// Repeat without the hook finishes. One member forced to fail: limit 1, and name ids so the failing id sorts first.
// That request still restores one other entity. Later calls restore the rest. The bad id is in failed each time.
// remaining reaches 0. Another hidden entity, not in the operation, with an unreadable history file, does not stop undo
// and is reported with its relative path.
#[test]
fn bulk_test_05_journal_failure_mid_undo_and_failing_member_and_corrupt_other() {
    let j = temp_journal();
    create_facet(j.path(), "work", "work", "", "blue", "💼", None).unwrap();

    let op = "bulk-accept-five";
    let e_a = "e-a-01";
    let e_b = "e-b-02";
    let e_c = "e-c-03";

    for id in [e_a, e_b, e_c] {
        write_entity(j.path(), id, json!({"id": id, "name": id}));
        write_rel(
            j.path(),
            "work",
            &format!("rel_{id}"),
            json!({"entity_id": id, "role": "user"}),
        );
    }
    block_journal_entities(
        j.path(),
        op,
        &[e_a.to_owned(), e_b.to_owned(), e_c.to_owned()],
    )
    .unwrap();

    // Hook: fail after 1 restore
    let hook = test_hooks::arm_fail_undo_after_n_restores(1);
    let undo_rep = unblock_journal_operation(j.path(), op, 10).unwrap();
    drop(hook);

    assert_eq!(undo_rep.restored.len(), 1);
    assert_eq!(undo_rep.restored[0].id, e_a);
    assert_eq!(undo_rep.remaining, 2);
    assert_eq!(
        undo_rep.stopped,
        Some(StoppedReport {
            reason: "undo_interrupted".to_owned(),
            id: None
        })
    );

    // Repeat without hook finishes
    let undo_rep2 = unblock_journal_operation(j.path(), op, 10).unwrap();
    assert_eq!(undo_rep2.remaining, 0);

    // Member forced to fail test
    let op2 = "bulk-accept-five-b";
    let bad_id = "01-bad-id"; // sorts first
    let good_id1 = "02-good-id";
    let good_id2 = "03-good-id";

    for id in [bad_id, good_id1, good_id2] {
        write_entity(j.path(), id, json!({"id": id, "name": id}));
        write_rel(
            j.path(),
            "work",
            &format!("rel_{id}"),
            json!({"entity_id": id, "role": "user"}),
        );
    }
    block_journal_entities(
        j.path(),
        op2,
        &[bad_id.to_owned(), good_id1.to_owned(), good_id2.to_owned()],
    )
    .unwrap();

    // Another hidden entity not in this operation with unreadable history file
    let corrupt_id = "corrupt-other";
    write_entity(
        j.path(),
        corrupt_id,
        json!({"id": corrupt_id, "blocked": true, "name": "Corrupt"}),
    );
    let corrupt_events_dir = j
        .path()
        .join("entities")
        .join(corrupt_id)
        .join("history/events");
    fs::create_dir_all(&corrupt_events_dir).unwrap();
    fs::write(
        corrupt_events_dir.join("00000000000000000001-vh_corrupt.json"),
        b"corrupt not json",
    )
    .unwrap();

    let fail_guard = test_hooks::arm_fail_unhide_save(bad_id);

    // Call 1: limit 1
    let rep_fail1 = unblock_journal_operation(j.path(), op2, 1).unwrap();
    assert_eq!(rep_fail1.restored.len(), 1);
    assert_eq!(rep_fail1.restored[0].id, good_id1);
    assert_eq!(rep_fail1.failed.len(), 1);
    assert_eq!(rep_fail1.failed[0].id, bad_id);
    assert_eq!(rep_fail1.remaining, 1);
    // Compare as paths: the report carries the platform's separators.
    let corrupt_rel =
        format!("entities/{corrupt_id}/history/events/00000000000000000001-vh_corrupt.json");
    assert!(
        rep_fail1
            .unreadable
            .iter()
            .any(|u| u.id == corrupt_id && Path::new(&u.path) == Path::new(&corrupt_rel))
    );

    // Call 2 with hook still armed: bad_id in failed again, good_id2 restored, remaining 0
    let rep_fail2 = unblock_journal_operation(j.path(), op2, 1).unwrap();
    assert_eq!(rep_fail2.restored.len(), 1);
    assert_eq!(rep_fail2.restored[0].id, good_id2);
    assert_eq!(rep_fail2.failed.len(), 1);
    assert_eq!(rep_fail2.failed[0].id, bad_id);
    assert_eq!(rep_fail2.remaining, 0);

    drop(fail_guard);
}

// 6. One block request and one undo request of 50 ids: full entities/ listings at most 2,
// each relationship file read at most once, each entity's read_visible_history at most once.
// The cache file's resolved map is the planning map.
// All-missing request, with identities created by raw writes so nothing has enqueued yet: @entity-search stays absent.
#[test]
fn bulk_test_06_budget_counters_listings_reads_and_cache_no_scan() {
    let j = temp_journal();
    create_facet(j.path(), "work", "work", "", "blue", "💼", None).unwrap();

    let ids_50: Vec<String> = (0..50).map(|i| format!("user-50-{:02}", i)).collect();
    for id in &ids_50 {
        write_entity(j.path(), id, json!({"id": id, "name": id}));
        write_rel(
            j.path(),
            "work",
            &format!("rel_{id}"),
            json!({"entity_id": id, "role": "member"}),
        );
    }

    solstone_core_entity::test_instrumentation::reset_test_counters();

    let block_rep = block_journal_entities(j.path(), "bulk-accept-six", &ids_50).unwrap();
    assert_eq!(block_rep.hidden.len(), 50);

    let listings_after_block =
        solstone_core_entity::test_instrumentation::entities_dir_listing_count();
    assert!(
        listings_after_block <= 1,
        "listings after block was {listings_after_block}"
    );

    // Assert relationship and history read counts after block
    for id in &ids_50 {
        let rel_path = j
            .path()
            .join(format!("facets/work/entities/rel_{id}/entity.json"));
        let count =
            solstone_core_entity::test_instrumentation::relationship_read_count_for_path(&rel_path);
        assert!(count <= 1, "block rel read count for {id} was {count}");

        let hist_count = solstone_core_entity::test_instrumentation::visible_history_read_count(id);
        assert!(
            hist_count <= 1,
            "block history read count for {id} was {hist_count}"
        );
    }

    // Cache file's resolved map is the planning map
    let cache_path = j.path().join("entities/.identity-map-cache.json");
    let cache_val: Value = serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
    let cache_resolved: std::collections::HashMap<String, String> =
        serde_json::from_value(cache_val["resolved"].clone()).unwrap();
    let current_map = read_identity_map(j.path()).unwrap();
    assert_eq!(cache_resolved, current_map.resolved);

    solstone_core_entity::test_instrumentation::reset_test_counters();

    let undo_rep = unblock_journal_operation(j.path(), "bulk-accept-six", 50).unwrap();
    assert_eq!(undo_rep.restored.len(), 50);

    let listings_after_undo =
        solstone_core_entity::test_instrumentation::entities_dir_listing_count();
    assert!(
        listings_after_undo <= 1,
        "listings after undo was {listings_after_undo}"
    );
    assert!(listings_after_block + listings_after_undo <= 2);

    // Check relationship and history read counts after undo
    for id in &ids_50 {
        let rel_path = j
            .path()
            .join(format!("facets/work/entities/rel_{id}/entity.json"));
        let count =
            solstone_core_entity::test_instrumentation::relationship_read_count_for_path(&rel_path);
        assert!(count <= 1, "undo rel read count for {id} was {count}");

        let hist_count = solstone_core_entity::test_instrumentation::visible_history_read_count(id);
        assert!(
            hist_count <= 1,
            "undo history read count for {id} was {hist_count}"
        );
    }

    // All-missing request with raw writes
    let j_miss = temp_journal();
    create_facet(j_miss.path(), "work", "work", "", "blue", "💼", None).unwrap();
    write_entity(j_miss.path(), "raw1", json!({"id": "raw1"}));

    let miss_rep = block_journal_entities(
        j_miss.path(),
        "bulk-accept-miss",
        &["miss1".to_owned(), "miss2".to_owned()],
    )
    .unwrap();
    assert_eq!(miss_rep.missing, vec!["miss1", "miss2"]);

    let todo_status = solstone_core_indexer_todo::inspect(j_miss.path()).unwrap();
    assert!(!todo_status.keys.iter().any(|k| k == "@entity-search"));
}

fn make_history_event(
    seq: i64,
    version_id: &str,
    entity_id: &str,
    before: Option<Value>,
    after: Value,
) -> Value {
    json!({
        "schema_version": 1,
        "version_id": version_id,
        "seq": seq,
        "ts": "2026-08-05T00:32:02Z",
        "entity_id": entity_id,
        "kind": "update",
        "caller": null,
        "actor": null,
        "identity_before": before,
        "identity_after": after,
        "operation": {},
    })
}

#[test]
fn bulk_test_07_prepared_reconcile_failure_durability_and_cache_enqueue() {
    // Sub-scenario 1: block_journal_entities on an already-hidden entity with prepared reconcile failure
    {
        let j1 = temp_journal();
        create_facet(j1.path(), "work", "work", "", "blue", "💼", None).unwrap();
        let e_hid = "e-hid";
        let e_sec = "e-sec";
        write_entity(
            j1.path(),
            e_hid,
            json!({"id": e_hid, "name": "Already Hidden"}),
        );
        save_entity_identity(
            j1.path(),
            e_hid,
            &json!({"id": e_hid, "name": "Already Hidden", "blocked": true}),
            None,
        )
        .unwrap();
        write_entity(
            j1.path(),
            e_sec,
            json!({"id": e_sec, "name": "Second Visible Entity"}),
        );

        let current = read_entity_identity(j1.path(), e_hid).unwrap().unwrap();

        let pub_event = make_history_event(
            10,
            "vh_1_pub",
            e_hid,
            Some(json!({"id": e_hid, "name": "Before", "blocked": true})),
            current.value().clone(),
        );
        let rep_event = make_history_event(
            11,
            "vh_2_rep",
            e_hid,
            Some(json!({"id": e_hid, "name": "Mismatched"})),
            json!({"id": e_hid, "name": "Unrelated"}),
        );

        let prep_dir = j1
            .path()
            .join("entities")
            .join(e_hid)
            .join("history/prepared");
        fs::create_dir_all(prep_dir.join("vh_1_pub")).unwrap();
        fs::write(
            prep_dir.join("vh_1_pub/event.json"),
            serde_json::to_vec_pretty(&pub_event).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(prep_dir.join("vh_2_rep")).unwrap();
        fs::write(
            prep_dir.join("vh_2_rep/event.json"),
            serde_json::to_vec_pretty(&rep_event).unwrap(),
        )
        .unwrap();

        let _ = fs::remove_dir_all(solstone_core_indexer_todo::root(j1.path()));
        let cache_path = j1.path().join("entities/.identity-map-cache.json");
        let _ = fs::remove_file(&cache_path);
        assert!(!cache_path.exists());

        let report = block_journal_entities(
            j1.path(),
            "bulk-op-sub1",
            &[e_hid.to_owned(), e_sec.to_owned()],
        )
        .unwrap();

        assert_eq!(
            report.stopped,
            Some(StoppedReport {
                reason: "reconcile_failed".to_owned(),
                id: Some(e_hid.to_owned()),
            })
        );
        assert!(!report.hidden.contains(&e_hid.to_owned()));
        assert!(!report.already_hidden.contains(&e_hid.to_owned()));
        assert!(!report.hidden.contains(&e_sec.to_owned()));

        let sec_identity = read_entity_identity(j1.path(), e_sec).unwrap().unwrap();
        assert!(!entity_is_hidden(sec_identity.value()));

        // Planning map cache updated
        assert!(cache_path.exists());
        let cache_val: Value =
            serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
        let cache_resolved: std::collections::HashMap<String, String> =
            serde_json::from_value(cache_val["resolved"].clone()).unwrap();
        let current_map = read_identity_map(j1.path()).unwrap();
        assert_eq!(cache_resolved, current_map.resolved);

        // Todo @entity-search enqueued
        let todo_status = solstone_core_indexer_todo::inspect(j1.path()).unwrap();
        assert!(todo_status.keys.iter().any(|k| k == "@entity-search"));

        // Prepared prefix published and unreconciled preserved
        assert!(!prep_dir.join("vh_1_pub").exists());
        let prepared = read_prepared_history(j1.path(), e_hid).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].staging_id, "vh_2_rep");
        let visible = read_visible_history(j1.path(), e_hid).unwrap();
        assert!(
            visible
                .iter()
                .any(|e| e.version_id().unwrap() == "vh_1_pub")
        );
    }

    // Sub-scenario 2: unblock_journal_operation with non-member hidden entity having prepared reconcile failure
    {
        let j2 = temp_journal();
        create_facet(j2.path(), "work", "work", "", "blue", "💼", None).unwrap();
        let e_other = "e-other";
        write_entity(
            j2.path(),
            e_other,
            json!({"id": e_other, "name": "Other Entity"}),
        );
        block_journal_entities(j2.path(), "bulk-op-other2", &[e_other.to_owned()]).unwrap();

        let current = read_entity_identity(j2.path(), e_other).unwrap().unwrap();

        let pub_event = make_history_event(
            10,
            "vh_1_pub",
            e_other,
            Some(json!({"id": e_other, "name": "Before", "blocked": true})),
            current.value().clone(),
        );
        let rep_event = make_history_event(
            11,
            "vh_2_rep",
            e_other,
            Some(json!({"id": e_other, "name": "Mismatched"})),
            json!({"id": e_other, "name": "Unrelated"}),
        );

        let prep_dir = j2
            .path()
            .join("entities")
            .join(e_other)
            .join("history/prepared");
        fs::create_dir_all(prep_dir.join("vh_1_pub")).unwrap();
        fs::write(
            prep_dir.join("vh_1_pub/event.json"),
            serde_json::to_vec_pretty(&pub_event).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(prep_dir.join("vh_2_rep")).unwrap();
        fs::write(
            prep_dir.join("vh_2_rep/event.json"),
            serde_json::to_vec_pretty(&rep_event).unwrap(),
        )
        .unwrap();

        let _ = fs::remove_dir_all(solstone_core_indexer_todo::root(j2.path()));
        let cache_path = j2.path().join("entities/.identity-map-cache.json");
        let _ = fs::remove_file(&cache_path);
        assert!(!cache_path.exists());

        let report = unblock_journal_operation(j2.path(), "bulk-op-target2", 50).unwrap();

        // Non-member is NOT marked as failed
        assert!(!report.failed.iter().any(|f| f.id == e_other));

        // Planning map cache updated
        assert!(cache_path.exists());
        let cache_val: Value =
            serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
        let cache_resolved: std::collections::HashMap<String, String> =
            serde_json::from_value(cache_val["resolved"].clone()).unwrap();
        let current_map = read_identity_map(j2.path()).unwrap();
        assert_eq!(cache_resolved, current_map.resolved);

        // Todo @entity-search enqueued
        let todo_status = solstone_core_indexer_todo::inspect(j2.path()).unwrap();
        assert!(todo_status.keys.iter().any(|k| k == "@entity-search"));

        // Prepared prefix published and unreconciled preserved
        assert!(!prep_dir.join("vh_1_pub").exists());
        let prepared = read_prepared_history(j2.path(), e_other).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].staging_id, "vh_2_rep");
        let visible = read_visible_history(j2.path(), e_other).unwrap();
        assert!(
            visible
                .iter()
                .any(|e| e.version_id().unwrap() == "vh_1_pub")
        );
    }

    // Sub-scenario 3: unblock_journal_operation with member hidden entity having prepared reconcile failure
    {
        let j3 = temp_journal();
        create_facet(j3.path(), "work", "work", "", "blue", "💼", None).unwrap();
        let e_mem = "e-member";
        write_entity(
            j3.path(),
            e_mem,
            json!({"id": e_mem, "name": "Member Entity"}),
        );
        block_journal_entities(j3.path(), "bulk-op-target3", &[e_mem.to_owned()]).unwrap();

        let current = read_entity_identity(j3.path(), e_mem).unwrap().unwrap();

        let pub_event = make_history_event(
            10,
            "vh_1_pub",
            e_mem,
            Some(json!({"id": e_mem, "name": "Before", "blocked": true})),
            current.value().clone(),
        );
        let rep_event = make_history_event(
            11,
            "vh_2_rep",
            e_mem,
            Some(json!({"id": e_mem, "name": "Mismatched"})),
            json!({"id": e_mem, "name": "Unrelated"}),
        );

        let prep_dir = j3
            .path()
            .join("entities")
            .join(e_mem)
            .join("history/prepared");
        fs::create_dir_all(prep_dir.join("vh_1_pub")).unwrap();
        fs::write(
            prep_dir.join("vh_1_pub/event.json"),
            serde_json::to_vec_pretty(&pub_event).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(prep_dir.join("vh_2_rep")).unwrap();
        fs::write(
            prep_dir.join("vh_2_rep/event.json"),
            serde_json::to_vec_pretty(&rep_event).unwrap(),
        )
        .unwrap();

        let _ = fs::remove_dir_all(solstone_core_indexer_todo::root(j3.path()));
        let cache_path = j3.path().join("entities/.identity-map-cache.json");
        let _ = fs::remove_file(&cache_path);
        assert!(!cache_path.exists());

        let report = unblock_journal_operation(j3.path(), "bulk-op-target3", 50).unwrap();

        // Member entity is reported in failed with "reconciliation_repair_required"
        assert!(report.failed.contains(&FailedEntity {
            id: e_mem.to_owned(),
            reason: "reconciliation_repair_required".to_owned(),
        }));
        assert!(report.restored.is_empty());
        assert_eq!(report.remaining, 0);
        assert_eq!(report.stopped, None);

        let current_after = read_entity_identity(j3.path(), e_mem).unwrap().unwrap();
        assert!(entity_is_hidden(current_after.value()));

        // Planning map cache updated
        assert!(cache_path.exists());
        let cache_val: Value =
            serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
        let cache_resolved: std::collections::HashMap<String, String> =
            serde_json::from_value(cache_val["resolved"].clone()).unwrap();
        let current_map = read_identity_map(j3.path()).unwrap();
        assert_eq!(cache_resolved, current_map.resolved);

        // Todo @entity-search enqueued
        let todo_status = solstone_core_indexer_todo::inspect(j3.path()).unwrap();
        assert!(todo_status.keys.iter().any(|k| k == "@entity-search"));

        // Prepared prefix published and unreconciled preserved
        assert!(!prep_dir.join("vh_1_pub").exists());
        let prepared = read_prepared_history(j3.path(), e_mem).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].staging_id, "vh_2_rep");
        let visible = read_visible_history(j3.path(), e_mem).unwrap();
        assert!(
            visible
                .iter()
                .any(|e| e.version_id().unwrap() == "vh_1_pub")
        );
    }

    // Sub-scenario 4: block_journal_entities on a visible entity with prepared reconcile failure
    {
        let j4 = temp_journal();
        create_facet(j4.path(), "work", "work", "", "blue", "💼", None).unwrap();
        let e_vis = "e-vis";
        write_entity(
            j4.path(),
            e_vis,
            json!({"id": e_vis, "name": "Visible Entity"}),
        );

        let current = read_entity_identity(j4.path(), e_vis).unwrap().unwrap();

        let pub_event = make_history_event(
            10,
            "vh_1_pub",
            e_vis,
            Some(json!({"id": e_vis, "name": "Before"})),
            current.value().clone(),
        );
        let rep_event = make_history_event(
            11,
            "vh_2_rep",
            e_vis,
            Some(json!({"id": e_vis, "name": "Mismatched"})),
            json!({"id": e_vis, "name": "Unrelated"}),
        );

        let prep_dir = j4
            .path()
            .join("entities")
            .join(e_vis)
            .join("history/prepared");
        fs::create_dir_all(prep_dir.join("vh_1_pub")).unwrap();
        fs::write(
            prep_dir.join("vh_1_pub/event.json"),
            serde_json::to_vec_pretty(&pub_event).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(prep_dir.join("vh_2_rep")).unwrap();
        fs::write(
            prep_dir.join("vh_2_rep/event.json"),
            serde_json::to_vec_pretty(&rep_event).unwrap(),
        )
        .unwrap();

        let _ = fs::remove_dir_all(solstone_core_indexer_todo::root(j4.path()));
        let cache_path = j4.path().join("entities/.identity-map-cache.json");
        let _ = fs::remove_file(&cache_path);
        assert!(!cache_path.exists());

        let report =
            block_journal_entities(j4.path(), "bulk-op-vis-save", &[e_vis.to_owned()]).unwrap();

        assert_eq!(
            report.stopped,
            Some(StoppedReport {
                reason: "entity_save_failed".to_owned(),
                id: Some(e_vis.to_owned()),
            })
        );

        let identity_after = read_entity_identity(j4.path(), e_vis).unwrap().unwrap();
        assert!(!entity_is_hidden(identity_after.value()));

        // Planning map cache updated
        assert!(cache_path.exists());
        let cache_val: Value =
            serde_json::from_str(&fs::read_to_string(&cache_path).unwrap()).unwrap();
        let cache_resolved: std::collections::HashMap<String, String> =
            serde_json::from_value(cache_val["resolved"].clone()).unwrap();
        let current_map = read_identity_map(j4.path()).unwrap();
        assert_eq!(cache_resolved, current_map.resolved);

        // Todo @entity-search enqueued
        let todo_status = solstone_core_indexer_todo::inspect(j4.path()).unwrap();
        assert!(todo_status.keys.iter().any(|k| k == "@entity-search"));

        // Prepared prefix published and unreconciled preserved
        assert!(!prep_dir.join("vh_1_pub").exists());
        let prepared = read_prepared_history(j4.path(), e_vis).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(prepared[0].staging_id, "vh_2_rep");
        let visible = read_visible_history(j4.path(), e_vis).unwrap();
        assert!(
            visible
                .iter()
                .any(|e| e.version_id().unwrap() == "vh_1_pub")
        );
    }
}
