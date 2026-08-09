// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::{Value, json};
use solstone_core_body_ingest::{
    OuraImportOptions, normalize_oura_documents, parse_oura_source, save_oura_source,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("solstone-oura-ingest-{stamp}"));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn fixture() -> PathBuf {
    root().join("tests/fixtures/importers/health/oura_synthetic")
}

fn approve(journal: &Path) {
    approve_with_retention(journal, "retain_parsed");
}

fn approve_with_retention(journal: &Path, retention: &str) {
    let path = journal.join("imports/_approvals");
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("oura_sync_preflight.json"),
        serde_json::to_vec(&json!({
            "schema": "solstone.oura_sync_preflight.v1",
            "checklist_version": "solstone.oura_sync_preflight.checklist.v2",
            "journal_root": journal.canonicalize().unwrap(),
            "requires_per_run_confirmation": true,
            "replication_destinations": {
                "time_machine": {"decision": "excluded"},
                "icloud": {"decision": "excluded"},
                "solbase": {"decision": "excluded"},
                "hosted_backup": {"decision": "excluded"},
                "other": {"decision": "excluded"}
            },
            "raw_retention": {"decision": retention}
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn retention_participates_in_quiet_run_identity() {
    let temporary = TempDir::new();
    let journal = temporary.0.join("journal");
    fs::create_dir(&journal).unwrap();
    approve_with_retention(&journal, "discard");
    let options = OuraImportOptions {
        timezone: "America/Denver".to_owned(),
        confirm_body_save: true,
        ..OuraImportOptions::default()
    };
    let discarded = save_oura_source(&fixture(), &journal, &options).unwrap();

    approve_with_retention(&journal, "retain_parsed");
    let retained = save_oura_source(&fixture(), &journal, &options).unwrap();
    assert!(!retained.skipped());
    assert_ne!(retained.bundle_id(), discarded.bundle_id());
    assert!(
        journal
            .join("imports")
            .join(retained.bundle_id().unwrap())
            .join("body-raw-inventory.jsonl")
            .is_file()
    );

    let identical = save_oura_source(&fixture(), &journal, &options).unwrap();
    assert!(identical.skipped());
    assert_eq!(identical.bundle_id(), retained.bundle_id());
}

#[test]
fn complete_synthetic_endpoint_corpus_normalizes_and_publishes_native_history() {
    let documents = parse_oura_source(&fixture()).unwrap();
    let rows = normalize_oura_documents(&documents, "America/Denver").unwrap();
    assert_eq!(rows.len(), 31);
    let mut record_types = BTreeMap::<String, usize>::new();
    for row in &rows {
        *record_types
            .entry(row.row()["record_type"].as_str().unwrap().to_owned())
            .or_default() += 1;
    }
    assert_eq!(record_types["oura.blood_glucose"], 4);
    assert_eq!(record_types["oura.heartrate"], 4);
    assert_eq!(record_types["oura.temperature_deviation"], 2);
    assert_eq!(record_types["oura.workout"], 2);
    let first_heart = rows
        .iter()
        .find(|row| row.row()["record_type"] == "oura.heartrate")
        .unwrap();
    assert_eq!(first_heart.row()["day"], "20260102");
    assert_eq!(
        first_heart.row()["metadata"],
        json!({
            "source": "sleep",
            "raw_timestamp": "2026-01-02T03:15:00-07:00",
            "timezone": "America/Denver"
        })
    );

    let temporary = TempDir::new();
    let journal = temporary.0.join("journal");
    fs::create_dir(&journal).unwrap();
    approve(&journal);
    let report = save_oura_source(
        &fixture(),
        &journal,
        &OuraImportOptions {
            timezone: "America/Denver".to_owned(),
            confirm_body_save: true,
            ..OuraImportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(report.rows(), 31);
    assert_eq!(report.days(), ["20260102", "20260103"]);
    let bundle = journal.join("imports").join(report.bundle_id().unwrap());
    for relative in [
        "body-bundle.json",
        "body-ledger.jsonl",
        "body-raw-inventory.jsonl",
        "manifest.json",
        "raw/oura/daily_readiness.jsonl",
        "normalized/2026-01.jsonl",
    ] {
        assert!(bundle.join(relative).is_file(), "missing {relative}");
    }
    let normalized: Vec<Value> = fs::read_to_string(bundle.join("normalized/2026-01.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(normalized.len(), 31);
    assert!(normalized.iter().all(|row| {
        row["dedupe_key"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:"))
    }));
    let first_heart = normalized
        .iter()
        .find(|row| row["record_type"] == "oura.heartrate")
        .expect("heartrate row");
    assert_eq!(
        first_heart["dedupe_key"],
        "sha256:597a4cd729ff3e2d393416886e2a7d916ac8d23259478866eac828a68d79b785"
    );
    let first_temperature = normalized
        .iter()
        .find(|row| row["record_type"] == "oura.temperature_deviation")
        .expect("temperature-deviation row");
    assert_eq!(
        first_temperature["dedupe_key"],
        "sha256:210786310f55f5e5774323339b34866dd60dec0b2ae43f11f24d787c30b1d646"
    );
    let connection = Connection::open(journal.join("imports/health-dedupe.sqlite")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_dedupe", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 31);
}

#[test]
fn malformed_known_endpoint_fails_loudly_and_unknown_files_are_ignored() {
    let temporary = TempDir::new();
    fs::write(temporary.0.join("README.json"), b"not json").unwrap();
    fs::write(
        temporary.0.join("daily_sleep.json"),
        br#"{"data":[{"id":"only-id"}]}"#,
    )
    .unwrap();
    let error = parse_oura_source(&temporary.0).unwrap_err();
    assert_eq!(
        error.kind(),
        solstone_core_body_ingest::BodyIngestErrorKind::Source
    );
    assert_eq!(error.stage(), "document_fields");
}

#[test]
fn normalization_timezone_participates_in_quiet_run_identity() {
    let temporary = TempDir::new();
    let journal = temporary.0.join("journal");
    fs::create_dir(&journal).unwrap();
    approve(&journal);
    let first = save_oura_source(
        &fixture(),
        &journal,
        &OuraImportOptions {
            timezone: "UTC".to_owned(),
            confirm_body_save: true,
            ..OuraImportOptions::default()
        },
    )
    .expect("first timezone publishes");
    let second = save_oura_source(
        &fixture(),
        &journal,
        &OuraImportOptions {
            timezone: "America/Denver".to_owned(),
            confirm_body_save: true,
            ..OuraImportOptions::default()
        },
    )
    .expect("changed timezone republishes");

    assert!(!first.skipped());
    assert!(!second.skipped());
    assert_ne!(first.bundle_id(), second.bundle_id());
    assert_eq!(
        fs::read_dir(journal.join("imports"))
            .expect("imports directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("body-"))
            .count(),
        2
    );
}
