// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use chrono::{Duration, TimeZone, Utc};
use serde_json::{Map, Value, json};
use tempfile::TempDir;

use super::*;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
        .single()
        .expect("valid time")
}

fn fields() -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("ticket_id".into(), json!("T-42"));
    fields.insert("content".into(), json!("authored-marker-never-persisted"));
    fields
}

fn ledger() -> (TempDir, Ledger) {
    let temp = TempDir::new().expect("tempdir");
    let ledger = Ledger::new(temp.path());
    (temp, ledger)
}

fn begin(ledger: &Ledger, parent: &str, at: chrono::DateTime<Utc>) -> OperationRecord {
    try_begin(ledger, parent, at).expect("begin operation")
}

fn try_begin(
    ledger: &Ledger,
    parent: &str,
    at: chrono::DateTime<Utc>,
) -> Result<OperationRecord, OperationError> {
    ledger.begin_operation(parent, "reply", &fields(), "jkt:test-thumbprint", 0, at)
}

fn record_path(ledger: &Ledger, record: &OperationRecord) -> std::path::PathBuf {
    ledger
        .record_path(&record.child_action_id)
        .expect("record path")
}

fn stored_json(path: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("record bytes")).expect("record json")
}

fn assert_return_matches_stored_except_key(returned: &OperationRecord, path: &std::path::Path) {
    let stored = stored_json(path);
    assert_eq!(stored["parent_action_id"], returned.parent_action_id);
    assert_eq!(stored["child_action_id"], returned.child_action_id);
    assert_eq!(stored["generation"], returned.generation);
    assert_eq!(stored["state"], returned.state);
    assert_eq!(stored["ack_state"], returned.ack_state);
    assert!(returned.operation_key.is_some());
    assert!(stored.get("operation_key").is_none());
}

#[test]
fn terminal_replays_rewrite_identical_payload_bytes_and_return_key_only() {
    let (_temp, ledger) = ledger();
    let started = now();
    let pending = begin(&ledger, "terminal-replay", started);
    let in_progress = ledger
        .mark_in_progress(&pending, started)
        .expect("in progress");
    let completed = ledger
        .mark_completed(&in_progress, Some("remote-1"), started)
        .expect("completed");
    let path = record_path(&ledger, &completed);
    assert!(
        path.with_file_name(format!(
            "{}.lock",
            path.file_name().expect("name").to_string_lossy()
        ))
        .is_file()
    );

    // `_update_current` writes unconditionally, so inode and mtime change and are not oracles.
    // Payload bytes and parsed content must remain identical for all terminal early returns.
    let before_complete = fs::read(&path).expect("before complete replay");
    let replayed = ledger
        .mark_completed(&completed, Some("different-remote-is-ignored"), started)
        .expect("completed replay");
    assert_eq!(
        before_complete,
        fs::read(&path).expect("after complete replay")
    );
    assert_return_matches_stored_except_key(&replayed, &path);

    let before_release = fs::read(&path).expect("before release replay");
    let released = ledger
        .release_retryable_lease(&completed, started)
        .expect("completed release replay");
    assert_eq!(
        before_release,
        fs::read(&path).expect("after release replay")
    );
    assert_return_matches_stored_except_key(&released, &path);

    let acknowledged = ledger.mark_acknowledged(&completed).expect("acknowledged");
    let before_ack = fs::read(&path).expect("before acknowledgement replay");
    let replayed_ack = ledger.mark_acknowledged(&acknowledged).expect("ack replay");
    assert_eq!(
        before_ack,
        fs::read(&path).expect("after acknowledgement replay")
    );
    assert_return_matches_stored_except_key(&replayed_ack, &path);
}

#[test]
fn dead_lease_recovery_changes_generation_lease_and_expiry_only() {
    let (_temp, ledger) = ledger();
    let started = now();
    let first = begin(&ledger, "dead-lease", started);
    let recovered = begin(&ledger, "dead-lease", started + LEASE_DURATION);
    assert_eq!(recovered.generation, first.generation + 1);
    assert_ne!(recovered.lease_id, first.lease_id);
    assert_eq!(
        recovered.lease_expires_at,
        Some(iso(started + LEASE_DURATION * 2))
    );
    assert_eq!(recovered.canonical_fingerprint, first.canonical_fingerprint);
    assert_eq!(recovered.operation_key, first.operation_key);
}

#[test]
fn existing_fingerprint_key_keeps_bytes_and_inode() {
    let (_temp, ledger) = ledger();
    let first = begin(&ledger, "key-existing-one", now());
    let key = ledger.storage_dir().join("operation-fingerprint.key");
    let bytes = fs::read(&key).expect("key bytes");
    #[cfg(unix)]
    let inode = key.metadata().expect("key metadata").ino();
    let _second = begin(&ledger, "key-existing-two", now());
    assert_eq!(fs::read(&key).expect("key bytes after"), bytes);
    #[cfg(unix)]
    assert_eq!(key.metadata().expect("key metadata after").ino(), inode);
    assert!(first.operation_key.is_some());
}

#[test]
fn record_persistence_excludes_payload_and_operation_key_and_scan_positive_control() {
    let (_temp, ledger) = ledger();
    let record = begin(&ledger, "scan-record", now());
    let marker = b"authored-marker-never-persisted";
    let mut found = Vec::new();
    collect_file_hits(ledger.storage_dir(), marker, &mut found);
    collect_file_hits(ledger.storage_dir(), b"spk1_", &mut found);
    assert!(found.is_empty(), "ledger leaked secret material: {found:?}");

    let sibling = ledger.storage_dir().join("positive-control.txt");
    fs::write(&sibling, marker).expect("plant marker");
    let mut positive = Vec::new();
    collect_file_hits(ledger.storage_dir(), marker, &mut positive);
    assert_eq!(positive, vec![sibling]);
    assert!(record.operation_key.is_some());
}

#[test]
fn lease_boundaries_and_in_progress_conflict_are_exact() {
    let (_temp, ledger) = ledger();
    let started = now();
    let record = begin(&ledger, "lease-boundary", started);
    let in_progress = ledger
        .mark_in_progress(&record, started)
        .expect("in progress");
    assert_eq!(
        try_begin(&ledger, "lease-boundary", started + Duration::seconds(59)).unwrap_err(),
        OperationError::OperationInProgress
    );
    let at_boundary = begin(&ledger, "lease-boundary", started + LEASE_DURATION);
    assert_eq!(at_boundary.generation, 2);
    assert_eq!(at_boundary.state, "in_progress");
    assert_eq!(in_progress.state, "in_progress");
    assert_eq!(record.generation, 1);
}

#[test]
fn terminal_retention_and_retirement_edges_are_exact() {
    let (_temp, ledger) = ledger();
    let started = now();
    let pending = begin(&ledger, "retention-edge", started);
    let in_progress = ledger
        .mark_in_progress(&pending, started)
        .expect("in progress");
    let completed = ledger
        .mark_completed(&in_progress, None, started)
        .expect("completed");
    let path = record_path(&ledger, &completed);
    let mut value = stored_json(&path);
    value["completed_at"] = Value::String(iso(started - RETENTION));
    fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("edit fixture");
    ledger
        .compact_expired_terminal_records(started)
        .expect("exact edge kept");
    assert_eq!(stored_json(&path).as_object().expect("record").len(), 15);

    value["completed_at"] = Value::String(iso(started - RETENTION - Duration::seconds(1)));
    fs::write(&path, serde_json::to_vec(&value).expect("json")).expect("edit fixture");
    ledger
        .compact_expired_terminal_records(started)
        .expect("expired retired");
    let marker = stored_json(&path);
    assert_eq!(marker.as_object().expect("marker").len(), 3);
    assert_eq!(
        try_begin(&ledger, "retention-edge", started).unwrap_err(),
        OperationError::OperationRetired
    );
}

#[test]
fn corrupt_record_bricks_begin_and_pending_list() {
    let (_temp, ledger) = ledger();
    let operations = ledger.storage_dir().join("operations");
    fs::create_dir_all(&operations).expect("operations dir");
    fs::write(
        operations.join("sact1_corrupt.json"),
        b"{\"schema_version\":1,\"wrong\":true}\n",
    )
    .expect("corrupt fixture");
    assert_eq!(
        try_begin(&ledger, "unrelated-action", now()).unwrap_err(),
        unavailable(RECORD_INVALID)
    );
    assert_eq!(
        ledger.list_pending_acknowledgements().unwrap_err(),
        unavailable(RECORD_INVALID)
    );
}

#[test]
fn acknowledgement_state_machine_and_pending_order() {
    let (_temp, ledger) = ledger();
    let started = now();
    for parent in ["ack-z", "ack-a"] {
        let pending = begin(&ledger, parent, started);
        let in_progress = ledger
            .mark_in_progress(&pending, started)
            .expect("in progress");
        ledger
            .mark_completed(&in_progress, Some(parent), started)
            .expect("completed");
    }
    let pending = ledger
        .list_pending_acknowledgements()
        .expect("pending acknowledgements");
    assert_eq!(pending.len(), 2);
    assert!(pending[0].child_action_id <= pending[1].child_action_id);
    ledger.mark_acknowledged(&pending[0]).expect("acknowledge");
    assert_eq!(
        ledger
            .list_pending_acknowledgements()
            .expect("remaining")
            .len(),
        1
    );
}

#[test]
fn stale_generation_is_rejected_after_current_read() {
    let (_temp, ledger) = ledger();
    let started = now();
    let stale = begin(&ledger, "stale-generation", started);
    let current = begin(&ledger, "stale-generation", started + LEASE_DURATION);
    assert_eq!(current.generation, 2);
    assert_eq!(
        ledger
            .mark_in_progress(&stale, started + LEASE_DURATION)
            .unwrap_err(),
        OperationError::OperationSuperseded
    );
}

#[test]
fn record_and_retired_tombstone_key_sets_are_exact() {
    let (_temp, ledger) = ledger();
    let started = now();
    let pending = begin(&ledger, "key-set", started);
    let in_progress = ledger
        .mark_in_progress(&pending, started)
        .expect("in progress");
    let completed = ledger
        .mark_completed(&in_progress, None, started)
        .expect("completed");
    let path = record_path(&ledger, &completed);
    assert_eq!(stored_json(&path).as_object().expect("record").len(), 15);
    ledger
        .compact_expired_terminal_records(started + RETENTION + Duration::seconds(1))
        .expect("retire");
    assert_eq!(stored_json(&path).as_object().expect("marker").len(), 3);
}

#[test]
fn fingerprint_key_permissions_length_and_artifact_failures_are_mapped() {
    let (_temp, ledger) = ledger();
    fs::create_dir_all(ledger.storage_dir()).expect("storage");
    let key = ledger.storage_dir().join("operation-fingerprint.key");
    fs::write(&key, [7_u8; KEY_BYTES]).expect("key");
    #[cfg(unix)]
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).expect("permissions");
    assert_eq!(
        try_begin(&ledger, "unsafe-key", now()).unwrap_err(),
        unavailable(KEY_UNSAFE)
    );

    #[cfg(unix)]
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).expect("permissions");
    fs::write(&key, [7_u8; KEY_BYTES - 1]).expect("short key");
    assert_eq!(
        try_begin(&ledger, "short-key", now()).unwrap_err(),
        unavailable(KEY_INVALID)
    );

    fs::remove_file(&key).expect("remove key");
    let sidecar = ledger.storage_dir().join("operations/lone.json.lock");
    fs::create_dir_all(sidecar.parent().expect("parent")).expect("operations");
    fs::write(sidecar, b"").expect("lone sidecar");
    assert_eq!(
        try_begin(&ledger, "sidecar-artifact", now()).unwrap_err(),
        unavailable(KEY_UNAVAILABLE)
    );
}

#[test]
fn malformed_action_id_is_state_unavailable() {
    let (_temp, ledger) = ledger();
    assert_eq!(
        ledger.record_path("bad/action").unwrap_err(),
        unavailable(ACTION_ID_INVALID)
    );
}

#[test]
fn failure_reason_maps_to_reference_terminal_error() {
    let (_temp, ledger) = ledger();
    let started = now();
    let pending = begin(&ledger, "terminal-failure", started);
    let failed = ledger
        .mark_failed(&pending, "tos_changed", started)
        .expect("failed");
    assert_eq!(failed.state, "failed");
    assert_eq!(
        try_begin(&ledger, "terminal-failure", started).unwrap_err(),
        OperationError::OperationTosChanged
    );
}

fn collect_file_hits(root: &std::path::Path, needle: &[u8], hits: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("read root").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_file_hits(&path, needle, hits);
        } else if path.is_file()
            && fs::read(&path)
                .expect("read file")
                .windows(needle.len())
                .any(|window| window == needle)
        {
            hits.push(path);
        }
    }
}
