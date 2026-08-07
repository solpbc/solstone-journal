// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

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
            "solstone-core-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary test directory");
        Self { path }
    }

    fn journal(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::create_dir(&path).expect("create temporary journal directory");
        path
    }

    fn script(&self, name: &str, source: &str) -> PathBuf {
        let path = self.path.join(format!("{name}.py"));
        fs::write(&path, source).expect("write temporary Python script");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repo_root() -> PathBuf {
    let mut cursor = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if cursor.join(".git").exists()
            && cursor.join("pyproject.toml").is_file()
            && cursor.join("solstone").is_dir()
        {
            return cursor;
        }
        cursor = cursor
            .parent()
            .expect("CARGO_MANIFEST_DIR must be inside the repository")
            .to_path_buf();
    }
}

fn repo_binary(name: &str) -> PathBuf {
    let path = repo_root().join(".venv").join("bin").join(name);
    assert!(
        path.is_file(),
        "{} is missing; run make install first",
        path.display()
    );
    path
}

fn write_config(journal: &Path, contents: &str) {
    let config = journal.join("config");
    fs::create_dir_all(&config).expect("create temporary journal config directory");
    fs::write(config.join("journal.json"), contents).expect("write temporary journal config");
}

fn write_symlink_loop_config(journal: &Path) {
    let config = journal.join("config");
    fs::create_dir_all(&config).expect("create temporary journal config directory");
    symlink("b", config.join("a")).expect("create first config symlink");
    symlink("a", config.join("b")).expect("create second config symlink");
    symlink("a", config.join("journal.json")).expect("create looping journal config");
}

fn run_python(script: &Path, args: &[&Path], journal: Option<&Path>) -> Output {
    let root = repo_root();
    let mut command = Command::new(repo_binary("python3"));
    command
        .arg(script)
        .args(args)
        .current_dir(&root)
        .env("PYTHONPATH", root);
    if let Some(journal) = journal {
        command.env("SOLSTONE_JOURNAL", journal);
    }
    command.output().expect("run Python integration harness")
}

fn json_lines(output: &Output) -> Vec<Value> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("harness must emit JSON lines"))
        .collect()
}

fn require_success(output: &Output) {
    assert!(
        output.status.success(),
        "harness failed with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn record<'a>(records: &'a [Value], label: &str) -> &'a Value {
    records
        .iter()
        .find(|record| record["label"] == label)
        .unwrap_or_else(|| panic!("missing harness record {label}"))
}

#[test]
fn convey_root_gate_distinguishes_corrupt_and_missing_journal_config() {
    let temp = TempDir::new("journal-config-convey");
    let bootstrap = temp.journal("bootstrap");
    let corrupt = temp.journal("corrupt");
    let missing = temp.journal("missing");
    write_config(&corrupt, "{bad json");

    let script = temp.script(
        "convey",
        r#"
import json
import os
import pathlib
import sys

from solstone.convey import create_app

bootstrap, corrupt, missing = map(pathlib.Path, sys.argv[1:])
os.environ["SOLSTONE_JOURNAL"] = str(bootstrap)
app = create_app(str(bootstrap))
app.config["TESTING"] = True
client = app.test_client()

def request(label, journal, path):
    os.environ["SOLSTONE_JOURNAL"] = str(journal)
    response = client.get(path)
    print(json.dumps({
        "label": label,
        "status": response.status_code,
        "content_type": response.content_type,
        "body": response.get_data(as_text=True),
        "redirect_location": response.headers.get("Location"),
    }))

request("corrupt-root", corrupt, "/")
request("corrupt-init", corrupt, "/init")
request("corrupt-api", corrupt, "/app/stats/api/stats")
request("missing-root", missing, "/")
request("missing-init", missing, "/init")
"#,
    );
    let output = run_python(&script, &[&bootstrap, &corrupt, &missing], None);
    require_success(&output);
    let records = json_lines(&output);

    let expected = format!(
        "I couldn't read your settings file at {}. Your settings were NOT changed. Repair the file or restore config/journal.json from a backup, then try again.",
        corrupt.join("config/journal.json").display(),
    );
    for label in ["corrupt-root", "corrupt-init"] {
        let response = record(&records, label);
        assert_eq!(response["status"], 500);
        assert!(
            response["content_type"]
                .as_str()
                .unwrap()
                .starts_with("text/plain")
        );
        assert_eq!(response["body"].as_str(), Some(expected.as_str()));
    }
    let api = record(&records, "corrupt-api");
    assert_eq!(api["status"], 500);
    assert!(
        api["content_type"]
            .as_str()
            .unwrap()
            .starts_with("application/json")
    );
    let api_body: Value =
        serde_json::from_str(api["body"].as_str().unwrap()).expect("API body must be JSON");
    assert_eq!(api_body["detail"].as_str(), Some(expected.as_str()));

    let missing_root = record(&records, "missing-root");
    assert_eq!(missing_root["status"], 302);
    assert_eq!(missing_root["redirect_location"], "/init");
    assert_eq!(record(&records, "missing-init")["status"], 200);
}

#[test]
fn secure_listener_logs_corrupt_config_and_keeps_false_branch_behavior() {
    let temp = TempDir::new("journal-config-secure-listener");
    let corrupt = temp.journal("corrupt");
    let inactive = temp.journal("inactive");
    write_config(&corrupt, "{bad json");
    write_config(&inactive, r#"{"setup": {}}"#);

    let script = temp.script(
        "secure_listener",
        r#"
import io
import json
import logging
import os
import pathlib
import sys

from solstone.convey.secure_listener import runtime
from solstone.think.link.ca import load_or_generate_ca
from solstone.think.link.paths import ca_dir

journal = pathlib.Path(sys.argv[1])
os.environ["SOLSTONE_JOURNAL"] = str(journal)
if sys.argv[2] == "commit":
    load_or_generate_ca(ca_dir())
stream = io.StringIO()
handler = logging.StreamHandler(stream)
logger = logging.getLogger("convey.secure_listener.runtime")
logger.addHandler(handler)
logger.setLevel(logging.WARNING)

class App:
    config = {"SECURE_LISTENER_ENABLED": True}
    secure_listener_started = False

app = App()
raised = None
try:
    runtime.start_secure_listener(app)
except BaseException as exc:
    raised = repr(exc)
finally:
    runtime.stop_all_secure_listener()
    logger.removeHandler(handler)

print(json.dumps({
    "raised": raised,
    "warned": bool(stream.getvalue()),
    "message": stream.getvalue(),
    "started": app.secure_listener_started,
}))
"#,
    );

    let commit = temp.path.join("commit");
    let no_commit = temp.path.join("no-commit");
    let corrupt_output = run_python(&script, &[&corrupt, &commit], None);
    require_success(&corrupt_output);
    let corrupt_report = json_lines(&corrupt_output)
        .pop()
        .expect("secure-listener harness report");
    assert_eq!(corrupt_report["raised"], Value::Null);
    assert_eq!(corrupt_report["started"], false);
    assert_eq!(corrupt_report["warned"], true);
    assert!(
        corrupt_report["message"]
            .as_str()
            .unwrap()
            .contains("I couldn't read your settings file")
    );

    let inactive_output = run_python(&script, &[&inactive, &no_commit], None);
    require_success(&inactive_output);
    let inactive_report = json_lines(&inactive_output)
        .pop()
        .expect("secure-listener harness report");
    assert_eq!(inactive_report["raised"], Value::Null);
    assert_eq!(inactive_report["started"], false);
    assert_eq!(inactive_report["warned"], false);
}

#[test]
fn config_journal_reports_corrupt_current_config_but_accepts_missing_target() {
    let temp = TempDir::new("journal-config-cli");
    let corrupt = temp.journal("corrupt");
    let inactive = temp.journal("inactive");
    let corrupt_target = temp.path.join("corrupt-target");
    let missing_target = temp.path.join("missing-target");
    let home = temp.path.join("home");
    fs::create_dir(&home).expect("create isolated home");
    write_config(&corrupt, "{bad json");
    write_config(&inactive, r#"{"setup": {}}"#);

    let script = temp.script(
        "config_cli",
        r#"
import json
import os
import pathlib
import subprocess
import sys

from solstone.think.install_guard import install_wrappers

home, current, target, sol, journal = map(pathlib.Path, sys.argv[1:])
os.environ["HOME"] = str(home)
install_wrappers(str(current), {"sol": str(sol), "journal": str(journal)})
result = subprocess.run(
    [str(journal), "config", "journal", str(target), "--switch", "--yes"],
    env=os.environ.copy(),
    text=True,
    capture_output=True,
)
print(json.dumps({
    "exit_code": result.returncode,
    "stderr": result.stderr,
    "target_exists": target.exists(),
}))
"#,
    );
    let sol = repo_binary("sol");
    let journal_bin = repo_binary("journal");

    let corrupt_output = run_python(
        &script,
        &[&home, &corrupt, &corrupt_target, &sol, &journal_bin],
        None,
    );
    require_success(&corrupt_output);
    let corrupt_report = json_lines(&corrupt_output)
        .pop()
        .expect("config CLI harness report");
    assert_eq!(corrupt_report["exit_code"], 1);
    assert!(
        corrupt_report["stderr"]
            .as_str()
            .unwrap()
            .contains("I couldn't read your settings file")
    );

    let missing_output = run_python(
        &script,
        &[&home, &inactive, &missing_target, &sol, &journal_bin],
        None,
    );
    require_success(&missing_output);
    let missing_report = json_lines(&missing_output)
        .pop()
        .expect("config CLI harness report");
    assert_eq!(missing_report["exit_code"], 0);
    assert_eq!(missing_report["stderr"], "");
    assert_eq!(missing_report["target_exists"], true);
}

fn doctor_results(journal: &Path) -> Vec<Value> {
    let output = Command::new(repo_binary("journal"))
        .args(["doctor", "--jsonl"])
        .current_dir(repo_root())
        .env("SOLSTONE_JOURNAL", journal)
        .output()
        .expect("run journal doctor");
    let records = json_lines(&output);
    assert!(
        records
            .iter()
            .any(|record| record["event"] == "doctor.completed"),
        "doctor must emit completion JSONL\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    records
        .into_iter()
        .filter(|record| {
            record["event"] == "check.completed"
                && matches!(
                    record["name"].as_str(),
                    Some("default_stt_ready" | "parakeet_cpp_stt_ready")
                )
        })
        .collect()
}

#[test]
fn doctor_reports_corrupt_config_only_through_the_stt_readiness_checks() {
    let temp = TempDir::new("journal-config-doctor");
    let malformed = temp.journal("malformed");
    let looped = temp.journal("looped");
    let missing = temp.journal("missing");
    let valid = temp.journal("valid");
    write_config(&malformed, "{bad json");
    write_symlink_loop_config(&looped);
    write_config(
        &valid,
        r#"{"setup": {"completed_at": 1}, "transcribe": {"backend": "other"}}"#,
    );

    for journal in [&malformed, &looped] {
        let checks = doctor_results(journal);
        assert_eq!(checks.len(), 2, "must isolate the two STT readiness checks");
        for check in checks {
            assert_eq!(check["status"], "failed");
            assert!(
                check["detail"]
                    .as_str()
                    .unwrap()
                    .starts_with("I couldn't read your settings file at ")
            );
            assert_eq!(
                check["fix"],
                "repair or restore config/journal.json from a backup"
            );
            assert_eq!(check["execution_error"], Value::Null);
        }
    }

    for journal in [&missing, &valid] {
        let checks = doctor_results(journal);
        assert_eq!(checks.len(), 2, "must isolate the two STT readiness checks");
        for check in checks {
            assert!(
                !check["detail"]
                    .as_str()
                    .unwrap()
                    .contains("I couldn't read your settings file")
            );
            assert_ne!(
                check["fix"],
                "repair or restore config/journal.json from a backup"
            );
            assert_eq!(check["execution_error"], Value::Null);
        }
    }
}
