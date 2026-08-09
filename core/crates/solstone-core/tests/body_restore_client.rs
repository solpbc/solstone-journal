// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::{Value, json};
use solstone_core_body_ingest::{
    AppleImportOptions, OuraImportOptions, save_apple, save_oura_source,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-core-body-restore-client-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary directory");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct ClientHarness {
    _temp: TempDir,
    python: PathBuf,
    journal: PathBuf,
    helper_log: PathBuf,
    persist_marker: PathBuf,
    index_marker: PathBuf,
}

fn quote(value: &Path) -> String {
    format!("'{}'", value.display().to_string().replace('\'', "'\\''"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn real_python() -> PathBuf {
    let project_python = repo_root().join(".venv/bin/python");
    if project_python.is_file() {
        return project_python;
    }
    let output = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .expect("python3 should execute");
    assert!(output.status.success(), "python3 discovery failed");
    PathBuf::from(String::from_utf8(output.stdout).expect("utf-8").trim())
}

impl ClientHarness {
    fn new(name: &str) -> Self {
        let temp = TempDir::new(name);
        let python = temp.path.join("python3");
        let journal = temp.path.join("journal");
        let helper_log = temp.path.join("helper.log");
        let persist_marker = temp.path.join("persist.marker");
        let index_marker = temp.path.join("index.marker");
        symlink(real_python(), &python).expect("link test python");
        fs::create_dir(&journal).expect("create journal");

        let dist_info = temp.path.join(format!(
            "solstone_core-{}.dist-info",
            env!("CARGO_PKG_VERSION")
        ));
        fs::create_dir(&dist_info).expect("create core distribution metadata");
        fs::write(
            dist_info.join("METADATA"),
            format!(
                "Metadata-Version: 2.1\nName: solstone-core\nVersion: {}\n",
                env!("CARGO_PKG_VERSION")
            ),
        )
        .expect("write core distribution metadata");

        let shim = temp.path.join("solstone-core");
        let real_core = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core"));
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexec {} \"$@\"\n",
            quote(&helper_log),
            quote(&real_core),
        );
        fs::write(&shim, script).expect("write core shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("make core shim executable");

        Self {
            _temp: temp,
            python,
            journal,
            helper_log,
            persist_marker,
            index_marker,
        }
    }

    fn python(&self, code: &str) -> Output {
        let root = repo_root();
        let code = format!(
            "from solstone.think import core_handshake as _handshake\n_handshake.is_source_checkout = lambda: False\n{code}"
        );
        Command::new(&self.python)
            .args(["-c", &code])
            .current_dir(&root)
            .env("PYTHONPATH", self.python_path())
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env("SOLSTONE_TEST_PERSIST_MARKER", &self.persist_marker)
            .env("SOLSTONE_TEST_INDEX_MARKER", &self.index_marker)
            .output()
            .expect("python client should execute")
    }

    fn python_path(&self) -> String {
        let root = repo_root();
        let output = Command::new(real_python())
            .args([
                "-c",
                "import site; print(next(iter(site.getsitepackages()), ''))",
            ])
            .output()
            .expect("discover Python site packages");
        assert!(output.status.success(), "site-packages discovery failed");
        let site_packages = String::from_utf8(output.stdout).expect("site path is UTF-8");
        format!(
            "{}:{}:{}",
            self.python.parent().expect("test python parent").display(),
            root.display(),
            site_packages.trim()
        )
    }

    fn helper_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.helper_log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn approve_apple(&self) {
        approve_apple(&self.journal);
    }
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

fn dedupe_rows(journal: &Path) -> Vec<Vec<Option<String>>> {
    let connection =
        Connection::open(journal.join("imports/health-dedupe.sqlite")).expect("open body store");
    let mut statement = connection
        .prepare(
            "SELECT dedupe_key, source_family, source_record_id, record_type, start_time, \
             end_time, value_hash, first_import_id, last_seen_import_id, normalized_ref, raw_ref \
             FROM health_dedupe ORDER BY dedupe_key",
        )
        .expect("prepare body-store snapshot");
    statement
        .query_map([], |row| {
            (0..11)
                .map(|index| row.get::<_, Option<String>>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .expect("query body-store snapshot")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("read body-store snapshot")
}

fn run_restic(repository: &str, password: &str, args: &[String]) -> Output {
    let output = Command::new("restic")
        .args(["--repo", repository])
        .args(args)
        .env("RESTIC_PASSWORD", password)
        .output()
        .expect("installed restic should execute");
    assert!(
        output.status.success(),
        "restic failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn shipping_backup_excludes() -> Vec<String> {
    let output = Command::new(real_python())
        .args([
            "-c",
            "import json; from solstone.think.backup.engine import BACKUP_EXCLUDES; print(json.dumps(BACKUP_EXCLUDES))",
        ])
        .current_dir(repo_root())
        .env("PYTHONPATH", repo_root())
        .output()
        .expect("Python should expose the shipping backup exclusions");
    assert_success(&output);
    serde_json::from_slice(&output.stdout).expect("backup exclusions are JSON")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "python failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn body_native_adapter_selects_and_runs_the_real_version_matched_helper() {
    let harness = ClientHarness::new("adapter");
    let output = harness.python(
        r#"
import json
import os
from solstone.think.body_native import rebuild_body_store

print(json.dumps(rebuild_body_store(os.environ["SOLSTONE_JOURNAL"]), sort_keys=True))
"#,
    );
    assert_success(&output);
    let result: Value = serde_json::from_slice(&output.stdout).expect("adapter JSON output");
    assert_eq!(
        result,
        json!({
            "schema": "solstone.body.rebuild.result.v1",
            "native_bundles": 0,
            "legacy_bundles": 0,
            "rows": 0,
        })
    );
    assert_eq!(
        harness.helper_lines(),
        [
            "--version".to_owned(),
            format!(
                "body rebuild --journal {} --json",
                harness.journal.display()
            ),
        ]
    );
    assert!(
        harness
            .journal
            .join("imports/health-dedupe.sqlite")
            .is_file()
    );
}

#[test]
fn shipping_importer_dispatches_apple_save_to_the_real_native_helper() {
    let harness = ClientHarness::new("apple-adapter");
    harness.approve_apple();
    let source = harness._temp.path.join("apple-export-with-decoys");
    fs::create_dir_all(source.join("apple_health_export")).expect("create Apple export directory");
    fs::copy(
        repo_root().join(
            "tests/fixtures/importers/health/apple_health_synthetic/apple_health_export/export.xml",
        ),
        source.join("apple_health_export/export.xml"),
    )
    .expect("copy synthetic Apple export");
    for name in ["one.md", "two.md", "three.md"] {
        fs::write(source.join(name), b"# synthetic decoy\n").expect("write Markdown decoy");
    }
    let output = Command::new(&harness.python)
        .args([
            "-c",
            r#"
from solstone.think import core_handshake as _handshake
_handshake.is_source_checkout = lambda: False
import os
import sys
from solstone.think.importers import cli

sys.argv = [
    "journal importer",
    os.environ["SOLSTONE_APPLE_SOURCE"],
    "--confirm-body-save",
    "--json",
]
cli.main()
"#,
        ])
        .current_dir(repo_root())
        .env("PYTHONPATH", harness.python_path())
        .env("SOLSTONE_JOURNAL", &harness.journal)
        .env("SOLSTONE_APPLE_SOURCE", &source)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("python importer should execute");
    assert_success(&output);
    let result: Value = serde_json::from_slice(&output.stdout).expect("adapter JSON output");
    assert_eq!(result["schema"], "solstone.body.ingest.result.v1");
    assert_eq!(result["source"], "apple_health");
    assert_eq!(result["mode"], "save");
    assert_eq!(result["skipped"], false);
    assert!(result["rows"].as_u64().is_some_and(|rows| rows > 0));
    let bundle = result["bundle_id"].as_str().expect("saved bundle id");
    assert!(
        harness
            .journal
            .join("imports")
            .join(bundle)
            .join("body-ledger.jsonl")
            .is_file()
    );
    assert!(
        harness
            .journal
            .join("imports/health-dedupe.sqlite")
            .is_file()
    );
    let preview = Command::new(&harness.python)
        .args([
            "-c",
            r#"
from solstone.think import core_handshake as _handshake
_handshake.is_source_checkout = lambda: False
import os
import sys
from solstone.think.importers import cli

sys.argv = [
    "journal importer",
    os.environ["SOLSTONE_APPLE_SOURCE"],
    "--dry-run",
]
cli.main()
"#,
        ])
        .current_dir(repo_root())
        .env("PYTHONPATH", harness.python_path())
        .env("SOLSTONE_JOURNAL", &harness.journal)
        .env("SOLSTONE_APPLE_SOURCE", &source)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("human Apple preview should execute");
    assert_success(&preview);
    let preview = String::from_utf8(preview.stdout).expect("preview output is UTF-8");
    assert!(preview.contains("Apple Health preview complete."));
    assert!(preview.contains("Rows:"));
    assert!(preview.contains("Days:"));
    assert_eq!(
        harness.helper_lines(),
        [
            "--version".to_owned(),
            format!("body apple --source {} --detect --json", source.display()),
            "--version".to_owned(),
            format!(
                "body apple --source {} --journal {} --json --save --confirm-body-save",
                source.display(),
                harness.journal.display()
            ),
            "--version".to_owned(),
            format!("body apple --source {} --detect --json", source.display()),
            "--version".to_owned(),
            format!(
                "body apple --source {} --journal {} --json",
                source.display(),
                harness.journal.display()
            ),
        ]
    );
}

#[test]
fn shipping_importer_routes_zip_detection_through_the_bounded_native_reader() {
    let harness = ClientHarness::new("apple-zip-detect");
    let source = harness._temp.path.join("synthetic-entry-bomb.zip");
    let entries = 10_001_u16;
    let mut eocd = b"PK\x05\x06".to_vec();
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    eocd.extend_from_slice(&entries.to_le_bytes());
    eocd.extend_from_slice(&entries.to_le_bytes());
    eocd.extend_from_slice(&0_u32.to_le_bytes());
    eocd.extend_from_slice(&0_u32.to_le_bytes());
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    fs::write(&source, eocd).expect("write bounded-detection fixture");

    let output = Command::new(&harness.python)
        .args([
            "-c",
            r#"
from solstone.think import core_handshake as _handshake
_handshake.is_source_checkout = lambda: False
import os
import sys
from solstone.think.importers import cli

sys.argv = ["journal importer", os.environ["SOLSTONE_APPLE_SOURCE"], "--dry-run"]
cli.main()
"#,
        ])
        .current_dir(repo_root())
        .env("PYTHONPATH", harness.python_path())
        .env("SOLSTONE_JOURNAL", &harness.journal)
        .env("SOLSTONE_APPLE_SOURCE", &source)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("Python importer should execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("archive_entry_limit"), "{stderr}");
    assert_eq!(
        harness.helper_lines(),
        [
            "--version".to_owned(),
            format!("body apple --source {} --detect --json", source.display()),
        ]
    );
    assert!(!harness.journal.join("imports").exists());
}

#[test]
fn shipping_importer_refuses_zip_symlinks_before_python_detection() {
    let harness = ClientHarness::new("apple-zip-symlink");
    let target = harness._temp.path.join("synthetic-entry-bomb-target.zip");
    let source = harness._temp.path.join("synthetic-entry-bomb-link.zip");
    let entries = 10_001_u16;
    let mut eocd = b"PK\x05\x06".to_vec();
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    eocd.extend_from_slice(&entries.to_le_bytes());
    eocd.extend_from_slice(&entries.to_le_bytes());
    eocd.extend_from_slice(&0_u32.to_le_bytes());
    eocd.extend_from_slice(&0_u32.to_le_bytes());
    eocd.extend_from_slice(&0_u16.to_le_bytes());
    fs::write(&target, eocd).expect("write symlink target");
    symlink(&target, &source).expect("link ZIP source");

    let output = Command::new(&harness.python)
        .args([
            "-c",
            r#"
from solstone.think import core_handshake as _handshake
_handshake.is_source_checkout = lambda: False
import os
import sys
from solstone.think.importers import cli

sys.argv = ["journal importer", os.environ["SOLSTONE_APPLE_SOURCE"], "--dry-run"]
cli.main()
"#,
        ])
        .current_dir(repo_root())
        .env("PYTHONPATH", harness.python_path())
        .env("SOLSTONE_JOURNAL", &harness.journal)
        .env("SOLSTONE_APPLE_SOURCE", &source)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("Python importer should execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("native Apple body source detection failed"));
    assert_eq!(
        harness.helper_lines(),
        [
            "--version".to_owned(),
            format!("body apple --source {} --detect --json", source.display()),
        ]
    );
    assert!(!harness.journal.join("imports").exists());
}

#[test]
fn shipping_importer_refuses_directory_symlinks_before_python_detection() {
    let harness = ClientHarness::new("apple-directory-symlink");
    let target = harness._temp.path.join("synthetic-apple-target");
    let source = harness._temp.path.join("synthetic-apple-link");
    fs::create_dir(&target).expect("create Apple directory target");
    fs::write(
        target.join("export.xml"),
        br#"<?xml version="1.0"?><HealthData/>"#,
    )
    .expect("write Apple export");
    for index in 0..3 {
        fs::write(target.join(format!("decoy-{index}.md")), b"# decoy")
            .expect("write generic importer decoy");
    }
    symlink(&target, &source).expect("link Apple directory source");

    let output = Command::new(&harness.python)
        .args([
            "-c",
            r#"
from solstone.think import core_handshake as _handshake
_handshake.is_source_checkout = lambda: False
import os
import sys
from solstone.think.importers import cli

sys.argv = ["journal importer", os.environ["SOLSTONE_APPLE_SOURCE"], "--dry-run"]
cli.main()
"#,
        ])
        .current_dir(repo_root())
        .env("PYTHONPATH", harness.python_path())
        .env("SOLSTONE_JOURNAL", &harness.journal)
        .env("SOLSTONE_APPLE_SOURCE", &source)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("Python importer should execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("native Apple body source detection failed"));
    assert_eq!(
        harness.helper_lines(),
        [
            "--version".to_owned(),
            format!("body apple --source {} --detect --json", source.display()),
        ]
    );
    assert!(!harness.journal.join("imports").exists());
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
fn rust_and_python_body_readers_match_the_complete_synthetic_corpora() {
    let harness = ClientHarness::new("body-differential");
    approve_apple(&harness.journal);
    approve_oura(&harness.journal);
    let apple_source = repo_root().join("tests/fixtures/importers/health/apple_health_synthetic");
    let oura_source = repo_root().join("tests/fixtures/importers/health/oura_synthetic");
    let apple = save_apple(
        &apple_source,
        &harness.journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .expect("native Apple import");
    let oura = save_oura_source(
        &oura_source,
        &harness.journal,
        &OuraImportOptions {
            timezone: "America/Denver".to_owned(),
            confirm_body_save: true,
            ..OuraImportOptions::default()
        },
    )
    .expect("native Oura import");
    let code = format!(
        r#"
import json
from dataclasses import asdict
from pathlib import Path
from zoneinfo import ZoneInfo
from solstone.think.importers.apple_health import _DateWindow, _parse_normalized_items
from solstone.think.importers.oura import normalize_bundle, parse_oura_bundle

def semantic(item):
    row = dict(item.row)
    row.pop("raw_ref", None)
    record = item.dedupe_record
    return {{
        "identity": {{
            "dedupe_key": record.dedupe_key,
            "end_time": record.end_time,
            "record_type": record.record_type,
            "source_family": record.source_family,
            "source_record_id": record.source_record_id,
            "start_time": record.start_time,
            "value_hash": record.value_hash,
        }},
        "row": row,
    }}

apple = _parse_normalized_items(
    Path({apple_source}),
    import_id="synthetic-differential",
    date_window=_DateWindow(start_day=None, end_day=None),
    raw_ref=None,
    progress_callback=None,
)
oura = normalize_bundle(
    parse_oura_bundle(Path({oura_source})),
    import_id="synthetic-differential",
    raw_ref_root=None,
    owner_timezone=ZoneInfo("America/Denver"),
)
rows = [semantic(item) for item in [*apple, *oura]]
rows.sort(key=lambda value: json.dumps(value, sort_keys=True, separators=(",", ":")))
print(json.dumps(rows, sort_keys=True, separators=(",", ":")))
"#,
        apple_source = serde_json::to_string(&apple_source.display().to_string())
            .expect("encode Apple source path"),
        oura_source = serde_json::to_string(&oura_source.display().to_string())
            .expect("encode Oura source path"),
    );
    let output = harness.python(&code);
    assert_success(&output);
    let python: Vec<Value> = serde_json::from_slice(&output.stdout).expect("Python oracle JSON");
    let mut rust = native_semantic_rows(
        &harness.journal,
        apple.bundle_id().expect("Apple bundle ID"),
    );
    rust.extend(native_semantic_rows(
        &harness.journal,
        oura.bundle_id().expect("Oura bundle ID"),
    ));
    rust.sort_by_key(|value| serde_json::to_string(value).expect("sort Rust semantic rows"));
    assert_eq!(rust, python);
}

#[test]
fn shipping_oura_adapter_dispatches_to_the_real_native_helper() {
    let harness = ClientHarness::new("oura-adapter");
    fs::create_dir_all(harness.journal.join("config")).expect("config directory");
    fs::write(
        harness.journal.join("config/journal.json"),
        br#"{"identity":{"timezone":"UTC"},"oura":{"client_id":"synthetic-client"}}"#,
    )
    .expect("journal config");
    let output = harness.python(
        r#"
import json
import os
from solstone.think.body_native import BodyNativeError, oura_sync

try:
    oura_sync(os.environ["SOLSTONE_JOURNAL"], save=False, window_days=7)
except BodyNativeError as error:
    print(json.dumps({"error": str(error)}))
else:
    raise AssertionError("missing Oura tokens must fail before network access")
"#,
    );
    assert_success(&output);
    let result: Value = serde_json::from_slice(&output.stdout).expect("adapter JSON output");
    let error = result["error"].as_str().expect("bounded adapter error");
    assert!(error.contains("native Oura body sync failed"));
    assert!(!error.contains("synthetic-client"));
    assert_eq!(
        harness.helper_lines(),
        [
            "--version".to_owned(),
            format!(
                "body oura sync --journal {} --json --window-days 7",
                harness.journal.display()
            ),
        ]
    );
    assert!(!harness.journal.join("imports/oura.json").exists());
}

#[test]
fn python_body_surface_has_no_writer_network_token_cursor_or_dedupe_authority() {
    let root = repo_root();
    let checks = [
        (
            "solstone/think/importers/apple_health.py",
            &[
                "def _save_export",
                "write_jsonl_records",
                "upsert_health_dedupe",
                "write_manifest(",
                "sqlite3",
            ][..],
        ),
        (
            "solstone/think/importers/oura.py",
            &[
                "OuraApiClient",
                "OuraSyncBackend",
                "urllib.request",
                "httpx",
                "requests.",
                "save_sync_state",
                "mutate_journal_config",
                "upsert_health_dedupe",
            ][..],
        ),
        (
            "solstone/think/importers/health_dedupe.py",
            &["sqlite3", "def upsert", "def connect", "def rebuild"][..],
        ),
    ];
    for (relative, forbidden) in checks {
        let source = fs::read_to_string(root.join(relative)).expect("read Python body surface");
        for needle in forbidden {
            assert!(
                !source.contains(needle),
                "{relative} retained forbidden production authority: {needle}"
            );
        }
    }
    assert!(!root.join("solstone/think/importers/oura_auth.py").exists());
    assert!(
        !root
            .join("scripts/backfill_health_workout_statistics.py")
            .exists()
    );
    assert!(
        !root
            .join("scripts/regenerate_health_day_summaries.py")
            .exists()
    );
    let registry = fs::read_to_string(root.join("solstone/think/importers/sync.py"))
        .expect("read Python sync registry");
    assert!(!registry.contains("OuraSyncBackend"));
    assert!(!registry.contains("\"oura\""));
}

#[test]
fn restore_refuses_success_when_the_real_native_rebuild_fails() {
    let harness = ClientHarness::new("restore-failure");
    fs::create_dir_all(
        harness
            .journal
            .join("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X32"),
    )
    .expect("create torn native bundle");

    let output = harness.python(
        r#"
from dataclasses import asdict
import json
import os
from pathlib import Path
from unittest.mock import patch

from solstone.think.backup import restore
from solstone.think.backup.destination import Destination
from solstone.think.backup.runner import ResticResult

responses = iter([
    ResticResult(0, "", "", [{"paths": ["/old/journal"]}], ("restic",)),
    ResticResult(0, "", "", {"message_type": "summary", "bytes_restored": 7}, ("restic",)),
    ResticResult(0, "", "", None, ("restic",)),
])

def forbidden_persist(*args, **kwargs):
    Path(os.environ["SOLSTONE_TEST_PERSIST_MARKER"]).write_text("called")

def forbidden_index(*args, **kwargs):
    Path(os.environ["SOLSTONE_TEST_INDEX_MARKER"]).write_text("called")

destination = Destination(
    repository="s3:synthetic-bucket/path",
    backend="s3",
    credentials={"access_key_id": "synthetic", "secret_access_key": "synthetic"},
)
with (
    patch.object(restore, "ensure_restic", return_value=Path("/restic")),
    patch.object(restore, "run_restic", side_effect=lambda *args, **kwargs: next(responses)),
    patch.object(restore, "set_destination", side_effect=forbidden_persist),
    patch.object(restore, "set_recovery_key", side_effect=forbidden_persist),
    patch.object(restore, "set_recovery_key_confirmed", side_effect=forbidden_persist),
    patch.object(restore, "scan_journal", side_effect=forbidden_index),
):
    result = restore.restore_journal(destination, "A" * 64)

print(json.dumps({
    "result": asdict(result),
    "persisted": Path(os.environ["SOLSTONE_TEST_PERSIST_MARKER"]).exists(),
    "indexed": Path(os.environ["SOLSTONE_TEST_INDEX_MARKER"]).exists(),
}, sort_keys=True))
"#,
    );
    assert_success(&output);
    let result: Value = serde_json::from_slice(&output.stdout).expect("restore JSON output");
    assert_eq!(
        result,
        json!({
            "result": {
                "status": "error",
                "reason_code": "body_rebuild_failed",
                "integrity_ok": false,
                "resumable": false,
                "bytes_restored": null,
            },
            "persisted": false,
            "indexed": false,
        })
    );
    assert_eq!(
        harness.helper_lines(),
        [
            "--version".to_owned(),
            format!(
                "body rebuild --journal {} --json",
                harness.journal.display()
            ),
        ]
    );
    let stderr = String::from_utf8(output.stderr).expect("restore stderr is UTF-8");
    assert!(!stderr.contains(&harness.journal.display().to_string()));
    assert!(!harness.persist_marker.exists());
    assert!(!harness.index_marker.exists());
}

#[test]
#[ignore = "requires installed restic; VPE runs this real backup/restore gate directly"]
fn apple_and_oura_body_history_survives_real_backup_restore_and_native_rebuild() {
    let harness = ClientHarness::new("real-round-trip");
    let source_journal = harness._temp.path.join("source-journal");
    fs::create_dir(&source_journal).expect("create source journal");

    approve_apple(&source_journal);
    approve_oura(&source_journal);

    let apple_source = repo_root().join("tests/fixtures/importers/health/apple_health_synthetic");
    let apple = save_apple(
        &apple_source,
        &source_journal,
        &AppleImportOptions {
            confirm_body_save: true,
            ..AppleImportOptions::default()
        },
    )
    .expect("save synthetic Apple body history");
    let oura_source = repo_root().join("tests/fixtures/importers/health/oura_synthetic");
    let oura = save_oura_source(
        &oura_source,
        &source_journal,
        &OuraImportOptions {
            timezone: "America/Denver".to_owned(),
            confirm_body_save: true,
            ..OuraImportOptions::default()
        },
    )
    .expect("save synthetic Oura body history");
    let before = dedupe_rows(&source_journal);
    assert_eq!(before.len(), (apple.rows() + oura.rows()) as usize);
    assert!(
        before
            .iter()
            .any(|row| row[1].as_deref() == Some("apple_health"))
    );
    assert!(
        before
            .iter()
            .any(|row| row[1].as_deref() == Some("oura_api"))
    );

    let repository_path = harness._temp.path.join("restic-repository");
    let repository = format!("local:{}", repository_path.display());
    let recovery_key = "A".repeat(64);
    run_restic(&repository, &recovery_key, &["init".to_owned()]);
    let mut backup_args = vec!["backup".to_owned(), source_journal.display().to_string()];
    for pattern in shipping_backup_excludes() {
        backup_args.push("--exclude".to_owned());
        backup_args.push(pattern);
    }
    run_restic(&repository, &recovery_key, &backup_args);
    let listing = run_restic(
        &repository,
        &recovery_key,
        &["ls".to_owned(), "latest".to_owned()],
    );
    let listing = String::from_utf8(listing.stdout).expect("restic listing is UTF-8");
    assert!(listing.contains("body-bundle.json"));
    assert!(listing.contains("body-ledger.jsonl"));
    assert!(listing.contains("body-raw-inventory.jsonl"));
    assert!(listing.contains("raw/oura/"));
    assert!(!listing.contains("health-dedupe.sqlite"));

    let restore_code = r#"
from dataclasses import asdict
import json
import os
from pathlib import Path
from unittest.mock import patch

from solstone.think.backup import restore
from solstone.think.backup.destination import Destination

destination = Destination(
    repository=os.environ["SOLSTONE_TEST_REPOSITORY"],
    backend="s3",
    credentials={"access_key_id": "synthetic", "secret_access_key": "synthetic"},
)
with (
    patch.object(restore, "ensure_restic", return_value=Path(os.environ["SOLSTONE_TEST_RESTIC"])),
    patch.object(restore, "scan_journal", return_value=None),
):
    result = restore._run_restore(
        destination,
        os.environ["SOLSTONE_TEST_RECOVERY_KEY"],
        lambda _canonical: None,
    )
print(json.dumps(asdict(result), sort_keys=True))
"#;
    let code = format!(
        "from solstone.think import core_handshake as _handshake\n_handshake.is_source_checkout = lambda: False\n{restore_code}"
    );
    let output = Command::new(&harness.python)
        .args(["-c", &code])
        .current_dir(repo_root())
        .env("PYTHONPATH", harness.python_path())
        .env("SOLSTONE_JOURNAL", &harness.journal)
        .env("SOLSTONE_TEST_REPOSITORY", &repository)
        .env("SOLSTONE_TEST_RECOVERY_KEY", &recovery_key)
        .env("SOLSTONE_TEST_RESTIC", "restic")
        .output()
        .expect("shipping restore should execute");
    assert_success(&output);
    let restored: Value = serde_json::from_slice(&output.stdout).expect("restore result JSON");
    assert_eq!(restored["status"], "ok");
    assert_eq!(restored["integrity_ok"], true);
    assert!(
        restored["bytes_restored"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );

    let after = dedupe_rows(&harness.journal);
    assert_eq!(after, before);
    for bundle in [apple.bundle_id(), oura.bundle_id()] {
        let bundle = bundle.expect("saved bundle ID");
        assert!(
            harness
                .journal
                .join("imports")
                .join(bundle)
                .join("body-ledger.jsonl")
                .is_file()
        );
    }
    let oura_bundle = oura.bundle_id().expect("saved Oura bundle ID");
    assert!(
        harness
            .journal
            .join("imports")
            .join(oura_bundle)
            .join("body-raw-inventory.jsonl")
            .is_file()
    );
    assert!(
        harness
            .journal
            .join("imports")
            .join(oura_bundle)
            .join("raw/oura/daily_readiness.jsonl")
            .is_file()
    );
    assert_eq!(
        harness.helper_lines(),
        [
            "--version".to_owned(),
            format!(
                "body rebuild --journal {} --json",
                harness.journal.display()
            ),
        ]
    );
}
