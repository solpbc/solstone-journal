// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-level reachability coverage for native speaker-resolve verbs.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "solstone-core-speaker-resolve-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("chronicle")).expect("create temporary journal");
    root
}

fn encoder() -> Value {
    json!({"id":"test","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","width":256})
}

fn run(verb: &str, request: Value) -> (Output, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["speaker-resolve", verb])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start speaker-resolve command");
    serde_json::to_writer(child.stdin.as_mut().expect("stdin"), &request).expect("write request");
    child.stdin.take();
    let output = child.wait_with_output().expect("wait for speaker-resolve");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    (output, response)
}

#[test]
fn ac1_identify_cli_reaches_native_orchestrator() {
    let journal = root("identify");
    let (_, output) = run(
        "identify",
        json!({
            "schema":"solstone-speaker-resolve-identify-request-v1", "journal_root":journal,
            "cluster_id":1, "name":"Target", "entity_id":null, "resolve_only":false,
            "create_new":false, "entity_type":"Person", "request_id":"reach-identify",
            "reviewed_near_match_entity_ids":[], "caller":"test", "actor":null, "encoder":encoder(),
        }),
    );
    assert!(output.get("error").is_some());
}

#[test]
fn identify_cli_returns_python_compatible_duplicate_reviewed_id_error() {
    let journal = root("identify-duplicate-reviewed-id");
    let (_, output) = run(
        "identify",
        json!({
            "schema":"solstone-speaker-resolve-identify-request-v1", "journal_root":journal,
            "cluster_id":1, "name":"Target", "entity_id":null, "resolve_only":false,
            "create_new":false, "entity_type":"Person", "request_id":"duplicate-reviewed-id",
            "reviewed_near_match_entity_ids":["ent-bob", " ent-bob "],
            "caller":"test", "actor":null, "encoder":encoder(),
        }),
    );
    assert_eq!(output["status"], "invalid_request");
    assert_eq!(
        output["invalid_reviewed_near_match_entity_ids"],
        json!([{"entity_id":"ent-bob","reason":"duplicate"}])
    );
}

#[test]
fn ac1_undo_identify_cli_reaches_native_orchestrator() {
    let journal = root("undo-identify");
    let (_, output) = run(
        "undo-identify",
        json!({
            "schema":"solstone-speaker-resolve-undo-identify-request-v1",
            "journal_root":journal, "operation_id":"idop_missing", "encoder":encoder(),
        }),
    );
    assert_eq!(output["status"], "not_found");
}

#[test]
fn ac1_bootstrap_voiceprints_cli_reaches_native_orchestrator() {
    let journal = root("bootstrap");
    let (_, output) = run(
        "bootstrap-voiceprints",
        json!({
            "schema":"solstone-speaker-resolve-bootstrap-voiceprints-request-v1",
            "journal_root":journal, "encoder":encoder(), "added_at":1, "dry_run":true,
        }),
    );
    assert_eq!(output["status"], "no_owner_centroid");
}

#[test]
fn ac1_seed_from_imports_cli_reaches_native_orchestrator() {
    let journal = root("seed");
    let (_, output) = run(
        "seed-from-imports",
        json!({
            "schema":"solstone-speaker-resolve-seed-from-imports-request-v1",
            "journal_root":journal, "encoder":encoder(), "added_at":1, "dry_run":true,
        }),
    );
    assert_eq!(output["status"], "no_owner_centroid");
}

#[test]
fn ac1_merge_names_cli_reaches_native_orchestrator() {
    let journal = root("merge");
    let (_, output) = run(
        "merge-names",
        json!({
            "schema":"solstone-speaker-resolve-merge-names-request-v1",
            "journal_root":journal, "alias_name":"Alias", "canonical_name":"Canonical",
        }),
    );
    assert_eq!(output["status"], "alias_not_found");
}

#[test]
fn ac1_backfill_cli_reaches_ledger_backed_execution() {
    let journal = root("backfill");
    let (_, output) = run(
        "backfill",
        json!({
            "schema":"solstone-speaker-resolve-backfill-request-v1", "journal_root":journal,
            "operation_id":"backfill-reach", "reattribute":false, "now_ms":1,
        }),
    );
    assert_eq!(output["done"], true);
    assert_eq!(output["total_count"], 0);
}

#[test]
fn ac1_backfill_status_cli_reaches_read_only_ledger_fold() {
    let journal = root("backfill-status");
    let (_, output) = run(
        "backfill-status",
        json!({
            "schema":"solstone-speaker-resolve-backfill-status-request-v1",
            "journal_root":journal, "operation_id":"backfill-missing",
        }),
    );
    assert_eq!(output["status"], "not_found");
}
