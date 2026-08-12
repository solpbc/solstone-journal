// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use serde_json::json;
use solstone_core_body_ingest::{OURA_CHECKLIST, OURA_PATH};
use solstone_core_import::{
    CONSENT_GATE_EXIT_CODE, ConsentGateOutcome, ConsentGateRequest, check_oura_sync_save,
};
use tempfile::TempDir;

#[test]
fn gate_fails_closed_for_missing_confirmation_and_unreadable_approval() {
    let tree = TempDir::new().unwrap();
    let request = ConsentGateRequest {
        journal_root: tree.path().to_path_buf(),
        confirmed: false,
        scheduled: false,
    };
    let blocked = check_oura_sync_save(&request);
    assert!(matches!(blocked, ConsentGateOutcome::Blocked(_)));
    assert_eq!(CONSENT_GATE_EXIT_CODE, 2);
    let ConsentGateOutcome::Blocked(failure) = blocked else {
        unreachable!()
    };
    assert_eq!(failure.reason(), "per_run_confirmation_missing");
    let rendered = failure.format_text();
    assert!(rendered.contains("Target journal:"));
    assert!(rendered.contains(&tree.path().join(OURA_PATH).display().to_string()));
    assert!(rendered.contains("Retry with confirmation"));

    let unreadable = ConsentGateRequest {
        journal_root: tree.path().to_path_buf(),
        confirmed: true,
        scheduled: false,
    };
    assert!(matches!(
        check_oura_sync_save(&unreadable),
        ConsentGateOutcome::Blocked(_)
    ));

    let approval = tree.path().join(OURA_PATH);
    fs::create_dir_all(approval.parent().unwrap()).unwrap();
    fs::write(&approval, b"not json").unwrap();
    let malformed = check_oura_sync_save(&unreadable);
    let ConsentGateOutcome::Blocked(failure) = malformed else {
        panic!("malformed approval must refuse")
    };
    assert_eq!(failure.reason(), "malformed_approval_artifact");

    fs::write(
        approval,
        serde_json::to_vec(&json!({"schema": "unknown"})).unwrap(),
    )
    .unwrap();
    let unknown = check_oura_sync_save(&unreadable);
    let ConsentGateOutcome::Blocked(failure) = unknown else {
        panic!("unknown approval must refuse")
    };
    assert_eq!(failure.reason(), "unsupported_approval_schema");
}

#[test]
fn scheduled_confirmation_cannot_replace_standing_consent() {
    let tree = TempDir::new().unwrap();
    let approval = tree.path().join(OURA_PATH);
    fs::create_dir_all(approval.parent().unwrap()).unwrap();
    fs::write(
        approval,
        serde_json::to_vec(&valid_approval(tree.path().display().to_string())).unwrap(),
    )
    .unwrap();
    let request = ConsentGateRequest {
        journal_root: tree.path().to_path_buf(),
        confirmed: true,
        scheduled: true,
    };
    let outcome = check_oura_sync_save(&request);
    let ConsentGateOutcome::Blocked(failure) = outcome else {
        panic!("scheduled approval must refuse")
    };
    assert_eq!(failure.reason(), "scheduled_sync_consent_missing");
    let payload = failure.to_python_payload();
    assert_eq!(payload["missing_fields"], json!([]));
    assert_eq!(payload["invalid_fields"], json!([]));
    assert_eq!(payload["checklist_version"], OURA_CHECKLIST);
    assert_eq!(
        payload["approval_path"],
        tree.path().join(OURA_PATH).display().to_string()
    );
    assert_eq!(payload.as_object().unwrap().len(), 10);
}

fn valid_approval(journal_root: String) -> serde_json::Value {
    json!({
        "schema": "solstone.oura_sync_preflight.v1",
        "checklist_version": "solstone.oura_sync_preflight.checklist.v2",
        "requires_per_run_confirmation": true,
        "journal_root": journal_root,
        "replication_destinations": {
            "time_machine": {"decision": "excluded"},
            "icloud": {"decision": "excluded"},
            "solbase": {"decision": "excluded"},
            "hosted_backup": {"decision": "excluded"},
            "other": {"decision": "excluded"}
        },
        "raw_retention": {"decision": "discard"}
    })
}
