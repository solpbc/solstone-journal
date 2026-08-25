// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s\n' "$0" > "$POISON_MARKER"
exit 97
"#;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "journal-facet-nav-cut-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Harness {
    _temp: TempDir,
    binary: PathBuf,
    journal: PathBuf,
    poison_marker: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new();
        let bin = temp.path.join("bin");
        fs::create_dir(&bin).expect("create binary directory");

        let binary = bin.join("solstone-core-journal");
        fs::copy(env!("CARGO_BIN_EXE_solstone-core-journal"), &binary)
            .expect("copy native journal binary");
        make_executable(&binary);

        let core = bin.join("solstone-core");
        fs::copy(locate_solstone_core_binary(), &core).expect("copy native core binary");
        make_executable(&core);

        for interpreter in ["python", "python3", "pytest", "uv", "ruff"] {
            let path = bin.join(interpreter);
            fs::write(&path, POISON_INTERPRETER).expect("write poison interpreter");
            make_executable(&path);
        }

        let journal = temp.path.join("journal");
        fs::create_dir(&journal).expect("create journal");
        let poison_marker = temp.path.join("python-invoked.txt");
        Self {
            _temp: temp,
            binary,
            journal,
            poison_marker,
        }
    }

    fn run(&self, args: &[&str], skip_supervisor_check: bool) -> Output {
        let _ = fs::remove_file(&self.poison_marker);
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .env("POISON_MARKER", &self.poison_marker)
            .env("HOME", self._temp.path.join("home"))
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
            .env_remove("SOL_SUPERVISOR_SPAWNED");
        if skip_supervisor_check {
            command.env("SOL_SKIP_SUPERVISOR_CHECK", "1");
        }
        command.output().expect("run native journal process")
    }

    fn notification_listener(&self) -> UnixListener {
        let health = self.journal.join("health");
        fs::create_dir_all(&health).expect("create health directory");
        UnixListener::bind(health.join("callosum.sock")).expect("bind callosum socket")
    }
}

fn locate_solstone_core_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates directory")
        .parent()
        .expect("core directory")
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(workspace_manifest)
        .args([
            "-p",
            "solstone-core",
            "--bin",
            "solstone-core",
            "--message-format=json",
        ])
        .output()
        .expect("build solstone-core");
    assert!(
        output.status.success(),
        "cargo build -p solstone-core failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let target = &message["target"];
        if message["reason"] == "compiler-artifact"
            && target["name"] == "solstone-core"
            && target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            && let Some(path) = message["executable"].as_str()
        {
            return PathBuf::from(path);
        }
    }
    panic!("cargo did not report solstone-core")
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make executable");
}

fn assert_python_was_not_invoked(poison_marker: &Path) {
    assert!(
        !poison_marker.exists(),
        "native dispatch invoked a poisoned Python interpreter"
    );
}

fn notification(listener: &UnixListener) -> Value {
    let (mut stream, _) = listener.accept().expect("accept navigation request");
    let mut line = String::new();
    stream
        .read_to_string(&mut line)
        .expect("read navigation request");
    serde_json::from_str(line.trim()).expect("valid navigation JSON")
}

#[test]
fn facet_candidates_help_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["facet-candidates", "--help"], false);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for token in [
        "usage: journal facet-candidates",
        "-h",
        "--help",
        "-v",
        "--verbose",
        "-d",
        "--debug",
    ] {
        assert!(stdout.contains(token), "missing {token:?} from {stdout:?}");
    }
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn navigate_help_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["navigate", "--help"], false);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for token in ["usage: journal navigate", "-h", "--help", "PATH"] {
        assert!(stdout.contains(token), "missing {token:?} from {stdout:?}");
    }
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn facet_candidates_invalid_args_run_natively() {
    let harness = Harness::new();
    let output = harness.run(&["facet-candidates", "--nonsense"], false);

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage: journal facet-candidates"));
    assert!(stderr.contains("journal facet-candidates: error: invalid arguments"));
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn navigate_invalid_args_run_natively() {
    let harness = Harness::new();
    let output = harness.run(&["navigate", "--nonsense"], false);

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage: journal navigate"));
    assert!(stderr.contains("journal navigate: error: invalid arguments"));
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn facet_candidates_happy_path_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["facet-candidates"], true);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Recorded/updated "));
    assert!(stdout.contains(" facet candidate(s)."));
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn navigate_path_only_happy_path_runs_natively() {
    let harness = Harness::new();
    let listener = harness.notification_listener();
    let output = harness.run(&["navigate", "/app/work"], true);

    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Navigate: "));
    assert_python_was_not_invoked(&harness.poison_marker);
    assert_eq!(
        notification(&listener),
        json!({"tract": "navigate", "event": "request", "path": "/app/work"})
    );
}

#[test]
fn navigate_facet_options_are_rejected_before_callosum() {
    let harness = Harness::new();
    let output = harness.run(&["navigate", "/app/work", "--facet=work"], true);

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "Put facet selection in the destination URL; for example, /app/entities?facet=work."
    ));
    assert_python_was_not_invoked(&harness.poison_marker);
}
