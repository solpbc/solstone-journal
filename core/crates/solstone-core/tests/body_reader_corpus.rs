// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_body_ingest::{
    AppleImportOptions, OuraImportOptions, save_apple, save_oura_source,
};

const FROZEN_CORPUS: &str = include_str!("../../../fixtures/body_reader_corpus.json");

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn approve_apple(journal: &Path) {
    let directory = journal.join("imports/_approvals");
    fs::create_dir_all(&directory).expect("create approval directory");
    fs::write(
        directory.join("health_import_preflight.json"),
        serde_json::to_vec(&json!({
            "schema": "solstone.health_import_preflight.v1",
            "checklist_version": "solstone.health_import_preflight.checklist.v3",
            "journal_root": journal.canonicalize().expect("canonical journal"),
            "approved_importers": ["apple_health"],
            "requires_per_run_confirmation": true,
            "no_real_health_data_in_artifact": true,
            "replication_destinations": {
                "time_machine": {"decision": "excluded"},
                "icloud": {"decision": "excluded"},
                "solbase": {"decision": "excluded"},
                "hosted_backup": {"decision": "excluded"},
                "other": {"decision": "excluded"}
            },
            "raw_retention": {
                "decision": "discard",
                "unparsed_sensitive_modalities_acknowledged": false
            }
        }))
        .expect("serialize approval"),
    )
    .expect("write approval");
}

fn approve_oura(journal: &Path) {
    let directory = journal.join("imports/_approvals");
    fs::create_dir_all(&directory).expect("create Oura approval directory");
    fs::write(
        directory.join("oura_sync_preflight.json"),
        serde_json::to_vec(&json!({
            "schema": "solstone.oura_sync_preflight.v1",
            "checklist_version": "solstone.oura_sync_preflight.checklist.v2",
            "journal_root": journal.canonicalize().expect("canonical source journal"),
            "requires_per_run_confirmation": true,
            "replication_destinations": {
                "time_machine": {"decision": "excluded"},
                "icloud": {"decision": "excluded"},
                "solbase": {"decision": "excluded"},
                "hosted_backup": {"decision": "excluded"},
                "other": {"decision": "excluded"}
            },
            "raw_retention": {"decision": "retain_parsed"}
        }))
        .expect("serialize Oura approval"),
    )
    .expect("write Oura approval");
}

fn native_semantic_rows(journal: &Path, bundle: &str) -> Vec<Value> {
    let root = journal.join("imports").join(bundle);
    let mut ledger = std::collections::BTreeMap::new();
    for line in fs::read_to_string(root.join("body-ledger.jsonl"))
        .expect("read native body ledger")
        .lines()
    {
        let event: Value = serde_json::from_str(line).expect("parse native ledger event");
        ledger.insert(
            event["dedupe_key"]
                .as_str()
                .expect("ledger dedupe key")
                .to_owned(),
            event,
        );
    }
    let mut files = fs::read_dir(root.join("normalized"))
        .expect("read normalized directory")
        .map(|entry| entry.expect("normalized entry").path())
        .collect::<Vec<_>>();
    files.sort();
    let mut result = Vec::new();
    for file in files {
        for line in fs::read_to_string(file)
            .expect("read normalized shard")
            .lines()
        {
            let mut row: Value = serde_json::from_str(line).expect("parse normalized row");
            let object = row.as_object_mut().expect("normalized row object");
            for field in [
                "import_id",
                "month",
                "normalized_ref",
                "raw_ref",
                "raw_inventory_sha256",
            ] {
                object.remove(field);
            }
            let dedupe = row["dedupe_key"]
                .as_str()
                .expect("row dedupe key")
                .to_owned();
            let event = &ledger[&dedupe];
            result.push(json!({
                "identity": {
                    "dedupe_key": dedupe,
                    "end_time": event["end_time"],
                    "record_type": event["record_type"],
                    "source_family": event["source_family"],
                    "source_record_id": event["source_record_id"],
                    "start_time": event["start_time"],
                    "value_hash": event["value_hash"],
                },
                "row": row,
            }));
        }
    }
    result.sort_by_key(|value| serde_json::to_string(value).expect("sort semantic rows"));
    result
}

#[test]
fn rust_body_readers_match_the_frozen_synthetic_corpora() {
    let journal = tempfile::tempdir().expect("temporary journal");
    approve_apple(journal.path());
    approve_oura(journal.path());
    let apple_source = repo_root().join("tests/fixtures/importers/health/apple_health_synthetic");
    let oura_source = repo_root().join("tests/fixtures/importers/health/oura_synthetic");
    let apple = save_apple(
        &apple_source,
        journal.path(),
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .expect("native Apple import");
    let oura = save_oura_source(
        &oura_source,
        journal.path(),
        &OuraImportOptions {
            timezone: "America/Denver".to_owned(),
            confirm_body_save: true,
            ..OuraImportOptions::default()
        },
    )
    .expect("native Oura import");

    let mut rust =
        native_semantic_rows(journal.path(), apple.bundle_id().expect("Apple bundle ID"));
    rust.extend(native_semantic_rows(
        journal.path(),
        oura.bundle_id().expect("Oura bundle ID"),
    ));
    rust.sort_by_key(|value| serde_json::to_string(value).expect("sort Rust semantic rows"));

    let mut expected: Vec<Value> =
        serde_json::from_str(FROZEN_CORPUS).expect("frozen corpus is JSON");
    expected.sort_by_key(|value| serde_json::to_string(value).expect("sort frozen corpus"));
    assert_eq!(rust, expected);
}
