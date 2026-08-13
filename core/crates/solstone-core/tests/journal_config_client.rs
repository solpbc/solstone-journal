// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_journal_config::materialized_defaults;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
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
            "solstone-core-journal-config-client-{name}-{}-{stamp}",
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
    sentinel: PathBuf,
    marker: PathBuf,
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
        let sentinel = temp.path.join("sentinel.log");
        let marker = temp.path.join("shim.marker");
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
            "#!/bin/sh\nprintf '%s %s\\n' \"$0\" \"$*\" >> {}\n\
if [ \"$1\" = journal-config ] && [ \"$2\" = commit ] && [ -n \"${{SOLSTONE_TEST_COMMIT_EXIT:-}}\" ]; then\n\
  exit \"$SOLSTONE_TEST_COMMIT_EXIT\"\n\
fi\n\
if [ \"$1\" = journal-config ] && [ \"$2\" = commit ] && [ -n \"${{SOLSTONE_TEST_APPEAR_CONFIG:-}}\" ] && [ ! -e \"${{SOLSTONE_TEST_APPEAR_MARKER}}\" ]; then\n\
  : > \"$SOLSTONE_TEST_APPEAR_MARKER\"\n\
  printf '%s' \"$SOLSTONE_TEST_APPEAR_CONFIG\" | {} journal-config commit --journal \"$4\" --expect absent\n\
fi\n\
exec {} \"$@\"\n",
            quote(&sentinel),
            quote(&real_core),
            quote(&real_core),
        );
        fs::write(&shim, script).expect("write core shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("make core shim executable");

        Self {
            _temp: temp,
            python,
            journal,
            sentinel,
            marker,
        }
    }

    fn python(&self, code: &str, extra_env: &[(&str, String)]) -> Output {
        let root = repo_root();
        let python_path = format!(
            "{}:{}",
            self.python.parent().expect("test python parent").display(),
            root.display()
        );
        let code = format!(
            "from solstone.think import core_handshake as _handshake\n_handshake.is_source_checkout = lambda: False\n{code}"
        );
        let mut command = Command::new(&self.python);
        command
            .args(["-c", &code])
            .current_dir(&root)
            .env("PYTHONPATH", python_path)
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env("SOLSTONE_TEST_APPEAR_MARKER", &self.marker);
        for (name, value) in extra_env {
            command.env(name, value);
        }
        command.output().expect("python client should execute")
    }

    fn lines(&self) -> Vec<String> {
        fs::read_to_string(&self.sentinel)
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
fn journal_config_client_ac4_materializes_only_in_rust() {
    let harness = ClientHarness::new("python-os-resolution");
    let output = harness.python(
        r#"
from unittest.mock import patch
import pwd

from solstone.think import journal_config
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config

def unexpected(*args, **kwargs):
    raise AssertionError("Python must not resolve OS journal defaults")

assert not hasattr(journal_config, "_resolve_os_identity")
assert not hasattr(journal_config, "_resolve_os_timezone")

with patch.object(pwd, "getpwuid", side_effect=unexpected):
    result = mutate_journal_config(
        lambda config: JournalConfigMutation(False, config["identity"].copy())
    )

print(result.written)
"#,
        &[],
    );
    assert_success(&output);
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "True");
    assert!(harness.journal.join("config/journal.json").exists());
    assert_eq!(
        harness
            .lines()
            .iter()
            .filter(|line| line.contains("journal-config read"))
            .count(),
        1
    );
}

#[test]
fn journal_config_client_ac5_matches_in_process_materialization() {
    let harness = ClientHarness::new("parity");
    let output = harness.python(
        r#"
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config

mutate_journal_config(lambda config: JournalConfigMutation(False, None))
"#,
        &[],
    );
    assert_success(&output);

    let direct = TempDir::new("in-process");
    let direct_journal = direct.path.join("journal");
    fs::create_dir(&direct_journal).expect("create direct journal");
    let transaction = mutate_journal_config(&direct_journal, LockOptions::default(), |_config| {
        JournalConfigMutation {
            changed: false,
            value: (),
        }
    })
    .expect("direct materialization");
    assert!(transaction.written);
    assert!(!transaction.changed);

    let python_config: Value = serde_json::from_slice(
        &fs::read(harness.journal.join("config/journal.json")).expect("read Python config"),
    )
    .expect("parse Python config");
    let direct_config: Value = serde_json::from_slice(
        &fs::read(direct_journal.join("config/journal.json")).expect("read direct config"),
    )
    .expect("parse direct config");
    assert_eq!(python_config, direct_config);
}

#[test]
fn journal_config_client_ac6_through_ac10_ac15_ac16() {
    let harness = ClientHarness::new("basics");
    let output = harness.python(
        r#"
import json
from solstone.think.journal_config import JournalConfigMutation, ensure_journal_config, mutate_journal_config
materialized = mutate_journal_config(lambda config: JournalConfigMutation(False, "materialized"))
created = ensure_journal_config()
result = mutate_journal_config(lambda config: JournalConfigMutation(False, "noop"))
changed = mutate_journal_config(lambda config: (config.update({"secret": "never-in-log"}) or JournalConfigMutation(True, "changed")))
print(json.dumps({"materialized": materialized.__dict__, "created": created, "noop": result.__dict__, "changed": changed.__dict__}))
"#,
        &[],
    );
    assert_success(&output);
    let result: Value = serde_json::from_slice(&output.stdout).expect("client JSON output");
    assert_eq!(
        result["created"],
        serde_json::to_value(materialized_defaults()).unwrap()
    );
    assert_eq!(
        result["materialized"],
        json!({"value": "materialized", "changed": false, "written": true})
    );
    assert_eq!(
        result["noop"],
        json!({"value": "noop", "changed": false, "written": false})
    );
    assert_eq!(
        result["changed"],
        json!({"value": "changed", "changed": true, "written": true})
    );
    let lines = harness.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("journal-config read"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("journal-config commit"))
    );
    for line in lines.iter().filter(|line| {
        line.contains("journal-config read") || line.contains("journal-config commit")
    }) {
        assert!(line.contains("--journal"));
        assert!(line.contains(harness.journal.to_str().unwrap()));
        assert!(!line.contains("never-in-log"));
    }
}

#[test]
fn journal_config_client_ac8_preserves_unknown_order_and_utf8() {
    let harness = ClientHarness::new("preservation");
    let path = harness.journal.join("config/journal.json");
    fs::create_dir_all(path.parent().unwrap()).expect("create config directory");
    fs::write(
        &path,
        b"{\"known\": \"before\", \"unknown\": \"Ren\xc3\xa9e\", \"ordered\": true}\n",
    )
    .expect("seed config");

    let output = harness.python(
        r#"
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config
def apply(config):
    config["known"] = "after"
    config["appended"] = "new"
    return JournalConfigMutation(True, None)
mutate_journal_config(apply)
"#,
        &[],
    );
    assert_success(&output);

    let committed = fs::read(&path).expect("read committed config");
    let decoded: Value = serde_json::from_slice(&committed).expect("committed JSON");
    assert_eq!(decoded["known"], "after");
    assert_eq!(decoded["unknown"], "Renée");
    assert_eq!(decoded["ordered"], true);
    assert_eq!(decoded["appended"], "new");
    assert!(
        committed
            .windows(b"\"unknown\": \"Ren\xc3\xa9e\"".len())
            .any(|window| { window == b"\"unknown\": \"Ren\xc3\xa9e\"" }),
        "unknown UTF-8 value should remain raw"
    );
    assert!(
        !committed
            .windows(b"Ren\\u00e9e".len())
            .any(|window| window == b"Ren\\u00e9e"),
        "unknown UTF-8 value must not be escaped"
    );
    let committed = String::from_utf8(committed).expect("committed UTF-8");
    let positions: Vec<_> = ["known", "unknown", "ordered", "appended"]
        .into_iter()
        .map(|key| {
            committed
                .find(&format!("\"{key}\""))
                .expect("top-level key should be present")
        })
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn journal_config_client_ac7a_ac11_ac12() {
    let harness = ClientHarness::new("reappeared");
    let output = harness.python(
        r#"
import json
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config
def apply(config):
    if config.get("racer"):
        return JournalConfigMutation(False, "saw-racer")
    config["racer"] = True
    return JournalConfigMutation(True, "first")
result = mutate_journal_config(apply)
print(json.dumps(result.__dict__))
"#,
        &[("SOLSTONE_TEST_APPEAR_CONFIG", "{\"racer\":true}".to_owned())],
    );
    assert_success(&output);
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"value": "saw-racer", "changed": false, "written": false})
    );
    let commits = harness
        .lines()
        .into_iter()
        .filter(|line| line.contains("journal-config commit"))
        .count();
    assert_eq!(commits, 1, "a reappeared no-op must not commit again");
}

#[test]
fn journal_config_client_ac13_through_ac13d() {
    let harness = ClientHarness::new("contention");
    let started = std::time::Instant::now();
    let output = harness.python(
        r#"
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config
from solstone.think.journal_io import LockTimeout
try:
    mutate_journal_config(lambda config: (config.update({"changed": True}) or JournalConfigMutation(True, None)))
except LockTimeout as error:
    print(f"timeout={error.timeout}")
"#,
        &[("SOLSTONE_TEST_COMMIT_EXIT", "65".to_owned())],
    );
    assert_success(&output);
    assert!(started.elapsed().as_secs_f64() < 1.0);
    assert!(String::from_utf8_lossy(&output.stdout).contains("timeout=0.4"));
    // AC13a's "one child commit" wording is structural: each retry iteration
    // creates one commit child. Across those iterations, timeouts decrease.
    let commits: Vec<_> = harness
        .lines()
        .into_iter()
        .filter(|line| line.contains("journal-config commit"))
        .collect();
    assert!(commits.len() > 1);
    let values: Vec<u64> = commits
        .iter()
        .map(|line| {
            let value = line
                .split("--lock-timeout-ms ")
                .nth(1)
                .expect("commit timeout flag")
                .split_whitespace()
                .next()
                .expect("commit timeout value");
            value.parse().expect("numeric timeout")
        })
        .collect();
    assert!(values.windows(2).all(|pair| pair[0] > pair[1]));

    for exit in ["64", "73", "74", "75"] {
        let single = ClientHarness::new(&format!("exit-{exit}"));
        let output = single.python(
            r#"
import json
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config
try:
    mutate_journal_config(lambda config: JournalConfigMutation(True, None))
except Exception as error:
    print(json.dumps({"type": type(error).__name__, "path": str(getattr(error, "path", "")), "timeout": getattr(error, "timeout", None)}))
"#,
            &[("SOLSTONE_TEST_COMMIT_EXIT", exit.to_owned())],
        );
        assert_success(&output);
        let error: Value = serde_json::from_slice(&output.stdout)
            .expect("exit must raise and report an exception");
        let error_type = error["type"].as_str().expect("exception type");
        if exit == "75" {
            assert_eq!(error_type, "LockTimeout");
            assert_eq!(
                error["path"],
                single
                    .journal
                    .join("config/journal.json")
                    .to_string_lossy()
                    .as_ref()
            );
            assert!(
                error["timeout"]
                    .as_f64()
                    .is_some_and(|timeout| timeout > 0.0),
                "LockTimeout should retain the attempted timeout"
            );
        } else {
            assert_ne!(error_type, "LockTimeout");
            assert_ne!(error_type, "CorruptConfigError");
        }
        assert!(
            single
                .lines()
                .iter()
                .filter(|line| line.contains("journal-config commit"))
                .count()
                == 1
        );
    }
}

#[test]
fn journal_config_client_ac14_and_ac14a() {
    let harness = ClientHarness::new("handshake");
    let output = harness.python(
        r#"
from unittest.mock import patch
from solstone.think import core_handshake
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config
for status in ("skip", "fail"):
    with patch("solstone.think.journal_config.core_handshake.check_solstone_core_handshake", return_value=core_handshake.CoreHandshakeResult(status, status)):
        try:
            mutate_journal_config(lambda config: JournalConfigMutation(True, None))
        except RuntimeError as error:
            print(str(error))
"#,
        &[],
    );
    assert_success(&output);
    assert!(
        harness.lines().is_empty(),
        "skip/fail must not launch the helper"
    );
}

#[test]
fn journal_config_client_ac17_corrupt_message_parity() {
    let harness = ClientHarness::new("corrupt");
    let path = harness.journal.join("config/journal.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{not valid json").unwrap();

    for verb in ["read", "commit"] {
        let direct = if verb == "read" {
            Command::new(env!("CARGO_BIN_EXE_solstone-core"))
                .args(["journal-config", "read", "--journal"])
                .arg(&harness.journal)
                .output()
                .expect("direct read command")
        } else {
            let mut child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
                .args(["journal-config", "commit", "--journal"])
                .arg(&harness.journal)
                .args(["--expect", "absent"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("direct commit command");
            child
                .stdin
                .as_mut()
                .expect("commit stdin")
                .write_all(b"{}")
                .expect("write commit input");
            child.wait_with_output().expect("wait direct commit")
        };
        assert_eq!(direct.status.code(), Some(69));
        let client = harness.python(
            r#"
from solstone.think.journal_config import JournalConfigMutation, mutate_journal_config
try:
    mutate_journal_config(lambda config: JournalConfigMutation(True, None))
except Exception as error:
    print(str(error))
"#,
            &[],
        );
        assert_success(&client);
        let prefix = format!("journal-config {verb} failed: ");
        assert_eq!(
            String::from_utf8_lossy(&direct.stderr).trim(),
            format!("{prefix}{}", String::from_utf8_lossy(&client.stdout).trim())
        );
    }
}
