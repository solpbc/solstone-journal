// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

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
        let python_path = format!(
            "{}:{}",
            self.python.parent().expect("test python parent").display(),
            root.display()
        );
        let code = format!(
            "from solstone.think import core_handshake as _handshake\n_handshake.is_source_checkout = lambda: False\n{code}"
        );
        Command::new(&self.python)
            .args(["-c", &code])
            .current_dir(&root)
            .env("PYTHONPATH", python_path)
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env("SOLSTONE_TEST_PERSIST_MARKER", &self.persist_marker)
            .env("SOLSTONE_TEST_INDEX_MARKER", &self.index_marker)
            .output()
            .expect("python client should execute")
    }

    fn helper_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.helper_log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
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
