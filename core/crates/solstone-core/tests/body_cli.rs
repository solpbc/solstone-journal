// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-core-body-cli-{name}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary directory creates");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn journal(&self) -> PathBuf {
        let journal = self.path.join("journal");
        fs::create_dir(&journal).expect("temporary journal creates");
        journal
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_solstone-core"))
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

fn write_entry_bomb(path: &Path) {
    let entries = 10_001_u16;
    let mut eocd = b"PK\x05\x06".to_vec();
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    eocd.extend_from_slice(&entries.to_le_bytes());
    eocd.extend_from_slice(&entries.to_le_bytes());
    eocd.extend_from_slice(&0_u32.to_le_bytes());
    eocd.extend_from_slice(&0_u32.to_le_bytes());
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    fs::write(path, eocd).expect("write bounded-detection fixture");
}

fn detect(source: &Path) -> Output {
    bin()
        .args(["body", "apple", "--source"])
        .arg(source)
        .args(["--detect", "--json"])
        .output()
        .expect("body apple --detect runs")
}

#[test]
fn body_apple_save_and_preview_use_the_native_ingest_contract() {
    let temporary = TempDir::new("apple-save");
    let journal = temporary.journal();
    approve_apple(&journal);
    let source = repo_root().join("tests/fixtures/importers/health/apple_health_synthetic");

    let preview = bin()
        .args(["body", "apple", "--source"])
        .arg(&source)
        .arg("--journal")
        .arg(&journal)
        .output()
        .expect("body apple preview runs");
    assert!(preview.status.success(), "{preview:?}");
    assert_eq!(
        String::from_utf8(preview.stdout).expect("preview stdout is UTF-8"),
        "Apple Health body import preview: rows=6 days=2 (nothing written)\n"
    );

    let save = bin()
        .args(["body", "apple", "--source"])
        .arg(&source)
        .arg("--journal")
        .arg(&journal)
        .args(["--json", "--save", "--confirm-body-save"])
        .output()
        .expect("body apple save runs");
    assert!(save.status.success(), "{save:?}");
    let result: serde_json::Value = serde_json::from_slice(&save.stdout).expect("save JSON parses");
    assert_eq!(result["schema"], "solstone.body.ingest.result.v1");
    assert_eq!(result["source"], "apple_health");
    assert_eq!(result["mode"], "save");
    assert_eq!(result["skipped"], false);
    assert_eq!(result["rows"], 6);
    assert_eq!(result["days"], json!(["20260101", "20260102"]));
    let bundle = result["bundle_id"].as_str().expect("saved bundle id");
    assert!(bundle.starts_with("body-"));
    assert!(
        journal
            .join("imports")
            .join(bundle)
            .join("body-ledger.jsonl")
            .is_file()
    );
    assert!(journal.join("imports/health-dedupe.sqlite").is_file());
}

#[test]
fn body_apple_detect_refuses_an_entry_bomb_zip() {
    let temporary = TempDir::new("apple-zip-detect");
    let journal = temporary.journal();
    let source = temporary.path().join("synthetic-entry-bomb.zip");
    write_entry_bomb(&source);

    let output = detect(&source);
    assert_eq!(output.status.code(), Some(65));
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        "Apple Health source detection failed: body-ingest source: archive_entry_limit\n"
    );
    assert!(!journal.join("imports").exists());
}

#[test]
fn body_apple_detect_refuses_a_zip_symlink() {
    let temporary = TempDir::new("apple-zip-symlink");
    let journal = temporary.journal();
    let target = temporary.path().join("synthetic-entry-bomb-target.zip");
    let source = temporary.path().join("synthetic-entry-bomb-link.zip");
    write_entry_bomb(&target);
    symlink(&target, &source).expect("link ZIP source");

    let output = detect(&source);
    assert_eq!(output.status.code(), Some(65));
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        "Apple Health source detection failed: body-ingest source: source_symlink\n"
    );
    assert!(!journal.join("imports").exists());
}

#[test]
fn body_apple_detect_refuses_a_directory_symlink() {
    let temporary = TempDir::new("apple-directory-symlink");
    let journal = temporary.journal();
    let target = temporary.path().join("synthetic-apple-target");
    let source = temporary.path().join("synthetic-apple-link");
    fs::create_dir(&target).expect("create Apple directory target");
    fs::write(
        target.join("export.xml"),
        br#"<?xml version="1.0"?><HealthData/>"#,
    )
    .expect("write Apple export");
    symlink(&target, &source).expect("link Apple directory source");

    let output = detect(&source);
    assert_eq!(output.status.code(), Some(65));
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        "Apple Health source detection failed: body-ingest source: source_symlink\n"
    );
    assert!(!journal.join("imports").exists());
}

#[test]
fn body_oura_sync_refuses_missing_tokens_without_leaking_client_id() {
    let temporary = TempDir::new("oura-tokens");
    let journal = temporary.journal();
    fs::create_dir(journal.join("config")).expect("config directory");
    fs::write(
        journal.join("config/journal.json"),
        br#"{"identity":{"timezone":"UTC"},"oura":{"client_id":"synthetic-client"}}"#,
    )
    .expect("journal config");

    let output = bin()
        .args(["body", "oura", "sync", "--journal"])
        .arg(&journal)
        .args(["--json", "--window-days", "7"])
        .output()
        .expect("body oura sync runs");
    assert_eq!(output.status.code(), Some(65));
    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        "Oura body sync failed: body-ingest source: authorization_needed\n"
    );
    assert!(!stdout.contains("synthetic-client"));
    assert!(!stderr.contains("synthetic-client"));
    assert!(!journal.join("imports/oura.json").exists());
}
