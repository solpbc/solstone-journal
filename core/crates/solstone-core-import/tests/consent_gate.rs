// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use solstone_core_import::{
    AutoTimestamp, ImportError, SyncActionSeams, SyncRequest, dispatch_sync,
    read_oura_scheduled_sync_guidance,
};

#[test]
fn oura_save_refusal_preserves_gate_exit_and_scheduled_consent_requirement() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().canonicalize().unwrap();
    let mut seams = SyncActionSeams {
        per_item_action: |_: solstone_core_import::SyncActionRequest<'_>| Ok(()),
    };

    let error = dispatch_sync(&request(&journal, false, false), &mut seams).unwrap_err();
    assert!(matches!(
        error,
        ImportError::Refusal {
            kind: "per_run_confirmation_missing",
            exit_code: 2,
            ..
        }
    ));
    assert!(error.to_string().contains("--confirm-body-save"));

    write_approval(&journal, None);
    let error = dispatch_sync(&request(&journal, true, true), &mut seams).unwrap_err();
    assert!(matches!(
        error,
        ImportError::Refusal {
            kind: "scheduled_sync_consent_missing",
            exit_code: 2,
            ..
        }
    ));
    assert!(
        error
            .to_string()
            .contains("standing scheduled_sync consent")
    );
}

#[test]
fn dispatch_fails_closed_for_missing_unreadable_and_invalid_oura_approval() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().canonicalize().unwrap();
    let mut seams = SyncActionSeams {
        per_item_action: |_: solstone_core_import::SyncActionRequest<'_>| Ok(()),
    };

    for setup in ["missing", "unreadable", "invalid"] {
        let approval = journal.join("imports/_approvals/oura_sync_preflight.json");
        if approval.exists() {
            if approval.is_dir() {
                fs::remove_dir_all(&approval).unwrap();
            } else {
                fs::remove_file(&approval).unwrap();
            }
        }
        match setup {
            "missing" => {}
            "unreadable" => {
                fs::create_dir_all(&approval).unwrap();
            }
            "invalid" => {
                fs::create_dir_all(approval.parent().unwrap()).unwrap();
                fs::write(&approval, br#"{"schema":"wrong"}"#).unwrap();
            }
            _ => unreachable!(),
        }
        let error = dispatch_sync(&request(&journal, true, false), &mut seams).unwrap_err();
        assert_eq!(error.exit_code(), Some(2), "{setup}: {error}");
    }
}

#[test]
fn scheduled_guidance_is_read_only_data() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().canonicalize().unwrap();
    write_approval(
        &journal,
        Some(json!({
            "approved": true,
            "cadence": "daily",
            "valid_until": "2099-01-01T00:00:00Z"
        })),
    );
    let before = journal_tree(&journal);

    let guidance = read_oura_scheduled_sync_guidance(&journal)
        .unwrap()
        .unwrap();

    assert_eq!(guidance.cadence, "daily");
    assert_eq!(guidance.valid_until, "2099-01-01T00:00:00Z");
    assert_eq!(journal_tree(&journal), before);
    assert!(!journal.join("crontab").exists());
    assert!(!journal.join("schedule-config").exists());
}

fn request<'a>(journal: &'a Path, confirmed: bool, scheduled: bool) -> SyncRequest<'a> {
    SyncRequest {
        journal,
        backend: "oura",
        save: true,
        source_path: None,
        window_days: Some(7),
        confirm_body_save: confirmed,
        scheduled,
        force: false,
        auto: AutoTimestamp::Absent,
        plaud_access_token: None,
    }
}

fn write_approval(journal: &Path, scheduled_sync: Option<serde_json::Value>) {
    let approval = journal.join("imports/_approvals/oura_sync_preflight.json");
    fs::create_dir_all(approval.parent().unwrap()).unwrap();
    let mut value = json!({
        "schema": "solstone.oura_sync_preflight.v1",
        "checklist_version": "solstone.oura_sync_preflight.checklist.v2",
        "journal_root": journal,
        "requires_per_run_confirmation": true,
        "replication_destinations": {
            "time_machine": {"decision": "excluded"},
            "icloud": {"decision": "excluded"},
            "solbase": {"decision": "excluded"},
            "hosted_backup": {"decision": "excluded"},
            "other": {"decision": "excluded"}
        },
        "raw_retention": {"decision": "discard"}
    });
    if let Some(scheduled_sync) = scheduled_sync {
        value["scheduled_sync"] = scheduled_sync;
    }
    fs::write(approval, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn journal_tree(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    walk(root, root, &mut paths);
    paths.sort();
    paths
}

fn walk(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        paths.push(path.strip_prefix(root).unwrap().to_path_buf());
        if entry.file_type().unwrap().is_dir() {
            walk(root, &path, paths);
        }
    }
}
