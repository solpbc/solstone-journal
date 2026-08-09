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

fn segment_request(root: &std::path::Path) -> Value {
    std::fs::create_dir_all(root.join("chronicle/20260809/main/120000_1/talents"))
        .expect("create segment");
    json!({"day":"20260809","stream":"main","segment_key":"120000_1"})
}

fn create_entity(root: &std::path::Path, entity_id: &str) {
    std::fs::create_dir_all(root.join("entities")).expect("create entities directory");
    solstone_core_entity::create_journal_entity(
        root,
        entity_id,
        "Test Person",
        "Person",
        None,
        None,
        &[],
        true,
        None,
    )
    .expect("create test entity");
}

#[test]
fn speaker_label_and_correction_write_verbs_reach_their_native_owners() {
    let journal = root("label-writes");
    let first_segment = segment_request(&journal);
    let base = json!({
        "schema":"solstone-speaker-resolve-write-stub-labels-request-v1",
        "journal_root":journal,
        "segment":first_segment,
        "reason":"no_audio",
    });
    assert_eq!(run("write-stub-labels", base).1["status"], "written");

    let segment = segment_request(&journal);
    assert_eq!(
        run(
            "write-full-labels",
            json!({
                "schema":"solstone-speaker-resolve-write-full-labels-request-v1",
                "journal_root":journal, "segment":segment,
                "labels":[{"sentence_id":1,"speaker":"person","confidence":"high","method":"acoustic"}],
                "metadata":{},
            }),
        )
        .1["status"],
        "written"
    );
    let segment = segment_request(&journal);
    assert_eq!(
        run(
            "patch-labels",
            json!({
                "schema":"solstone-speaker-resolve-patch-labels-request-v1",
                "journal_root":journal, "segment":segment,
                "patches":{"1":{"speaker":"other"}}, "allow_insert":false,
            }),
        )
        .1["status"],
        "written"
    );
    let segment = segment_request(&journal);
    let restore = run(
        "restore-label-rows",
        json!({
            "schema":"solstone-speaker-resolve-restore-label-rows-request-v1",
            "journal_root":journal, "segment":segment,
            "restorations":[{
                "sentence_id":1,
                "expected_current_label":{"sentence_id":1,"speaker":"other","confidence":"high","method":"acoustic"},
                "prior_state":"present",
                "prior_label":{"sentence_id":1,"speaker":"person","confidence":"high","method":"acoustic"},
            }],
        }),
    )
    .1;
    assert_eq!(restore["restored_count"], 1);
    let segment = segment_request(&journal);
    assert_eq!(
        run(
            "append-correction",
            json!({
                "schema":"solstone-speaker-resolve-append-correction-request-v1",
                "journal_root":journal, "segment":segment,
                "correction":{"sentence_id":1,"corrected_speaker":"person"},
            }),
        )
        .1["status"],
        "appended"
    );
}

#[test]
fn direct_voiceprint_and_backfill_verbs_reach_entity_store() {
    let journal = root("voiceprint-writes");
    create_entity(&journal, "person");
    let metadata = json!({
        "day":"20260809", "segment_key":"120000_1", "source":"audio",
        "sentence_id":1, "added_at":1, "last_seen_ts":1,
    });
    assert_eq!(
        run(
            "write-voiceprint",
            json!({
                "schema":"solstone-speaker-resolve-write-voiceprint-request-v1",
                "journal_root":journal, "entity_id":"person", "embedding":vec![1.0; 256],
                "metadata":metadata.clone(), "encoder":encoder(),
            }),
        )
        .1["status"],
        "written"
    );
    assert_eq!(
        run(
            "backfill-voiceprint-last-seen",
            json!({
                "schema":"solstone-speaker-resolve-backfill-voiceprint-last-seen-request-v1",
                "journal_root":journal, "entity_id":"person", "last_seen_ts":2, "encoder":encoder(),
            }),
        )
        .1["rows_written"],
        1
    );
    assert_eq!(
        run(
            "remove-voiceprint",
            json!({
                "schema":"solstone-speaker-resolve-remove-voiceprint-request-v1",
                "journal_root":journal, "entity_id":"person", "key":metadata, "encoder":encoder(),
            }),
        )
        .1["outcome"],
        "unlinked"
    );
}

#[test]
fn clear_and_wipe_speaker_artifact_verbs_are_mechanical_and_strict() {
    let journal = root("artifact-wipes");
    create_entity(&journal, "person");
    let entity_dir = journal.join("entities/person");
    std::fs::write(entity_dir.join("voiceprints.npz"), b"voiceprints").expect("write voiceprints");
    std::fs::write(entity_dir.join("owner_centroid.npz"), b"centroid").expect("write centroid");
    std::fs::create_dir_all(journal.join("awareness")).expect("create awareness");
    std::fs::write(journal.join("awareness/owner_candidate.npz"), b"candidate")
        .expect("write candidate");
    assert_eq!(
        run(
            "clear-owner-candidate",
            json!({
                "schema":"solstone-speaker-resolve-clear-owner-candidate-request-v1",
                "journal_root":journal,
            }),
        )
        .1["removed"],
        true
    );
    std::fs::write(journal.join("awareness/owner_candidate.npz"), b"candidate")
        .expect("restore candidate");
    let dry_run = run(
        "wipe-speaker-artifacts",
        json!({
            "schema":"solstone-speaker-resolve-wipe-speaker-artifacts-request-v1",
            "journal_root":journal, "dry_run":true,
        }),
    )
    .1;
    assert_eq!(dry_run["total_files"], 3);
    assert_eq!(
        run(
            "wipe-speaker-artifacts",
            json!({
                "schema":"solstone-speaker-resolve-wipe-speaker-artifacts-request-v1",
                "journal_root":journal, "dry_run":false,
            }),
        )
        .1["total_files"],
        3
    );
    assert!(!entity_dir.join("voiceprints.npz").exists());

    let mut child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["speaker-resolve", "write-stub-labels"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start malformed request");
    serde_json::to_writer(
        child.stdin.as_mut().expect("stdin"),
        &json!({
            "schema":"solstone-speaker-resolve-write-stub-labels-request-v1",
            "journal_root":journal,
            "segment":{"day":"20260809","stream":"main","segment_key":"120000_1"},
            "reason":"no_audio",
            "unexpected":true,
        }),
    )
    .expect("write malformed request");
    child.stdin.take();
    let output = child
        .wait_with_output()
        .expect("wait for malformed request");
    assert_eq!(output.status.code(), Some(64));
    let error: Value = serde_json::from_slice(&output.stderr).expect("structured error JSON");
    assert_eq!(error["error"], "speaker_resolve_failed");
    assert_eq!(error["exit_code"], 64);
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
