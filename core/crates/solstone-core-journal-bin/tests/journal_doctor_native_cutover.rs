// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s\n' "$0" > "$POISON_MARKER"
exit 97
"#;

const HELP: &str = concat!(
    "usage: journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\n",
    "                      [--feature FEATURE] [--readiness]\n",
    "\n",
    "Run solstone diagnostics.\n",
    "\n",
    "options:\n",
    "  -h, --help         show this help message and exit\n",
    "  --verbose          print every check result\n",
    "  --json             emit JSON instead of text\n",
    "  --jsonl            emit one-JSON-per-line events instead of text\n",
    "  --port PORT        port to probe (default: 5015)\n",
    "  --feature FEATURE  Run only the named feature check (pdf-export, pdf-import)\n",
    "  --readiness        run the setup readiness battery\n",
    "\n",
    "If 'journal doctor' is unavailable (e.g. before 'make install' completes), run\n",
    "'python3 scripts/doctor.py' from the repo root for the same diagnostic.\n",
);

const USAGE: &str = concat!(
    "usage: journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\n",
    "                      [--feature FEATURE] [--readiness]\n",
);

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
            "solstone-core-journal-doctor-cutover-{}-{stamp}",
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
    home: PathBuf,
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
        Self {
            home: temp.path.join("home"),
            poison_marker: temp.path.join("python-invoked.txt"),
            _temp: temp,
            binary,
            journal,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let _ = fs::remove_file(&self.poison_marker);
        Command::new(&self.binary)
            .args(args)
            .env("POISON_MARKER", &self.poison_marker)
            .env("HOME", &self.home)
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env("PATH", self.binary.parent().expect("binary parent"))
            .env_remove("SOL_SKIP_SUPERVISOR_CHECK")
            .env_remove("SOL_SUPERVISOR_SPAWNED")
            .output()
            .expect("run native journal process")
    }

    fn assert_python_was_not_invoked(&self) {
        assert!(
            !self.poison_marker.exists(),
            "native dispatch invoked a poisoned Python interpreter"
        );
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

#[test]
fn doctor_batteries_run_natively_without_pinning_host_verdicts() {
    let harness = Harness::new();

    for args in [["doctor"].as_slice(), ["doctor", "--readiness"].as_slice()] {
        let output = harness.run(args);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("doctor:"),
            "missing doctor summary for {args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        harness.assert_python_was_not_invoked();
    }

    for args in [
        ["doctor", "--json"].as_slice(),
        ["doctor", "--readiness", "--json"].as_slice(),
    ] {
        let output = harness.run(args);
        let value: Value = serde_json::from_slice(&output.stdout).expect("doctor JSON output");
        assert!(value["checks"].is_array(), "missing checks for {args:?}");
        assert!(value["summary"].is_object(), "missing summary for {args:?}");
        harness.assert_python_was_not_invoked();
    }

    for args in [
        ["doctor", "--jsonl"].as_slice(),
        ["doctor", "--readiness", "--jsonl"].as_slice(),
    ] {
        let output = harness.run(args);
        let events = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("doctor JSONL event"))
            .collect::<Vec<_>>();
        assert_eq!(
            events.first().and_then(|event| event["event"].as_str()),
            Some("doctor.started")
        );
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "check.completed"),
            "missing check event for {args:?}"
        );
        assert_eq!(
            events.last().and_then(|event| event["event"].as_str()),
            Some("doctor.completed")
        );
        harness.assert_python_was_not_invoked();
    }
}

#[test]
fn doctor_help_and_usage_match_the_owner_facing_grammar_without_python() {
    let harness = Harness::new();
    for args in [["doctor", "--help"].as_slice(), ["doctor", "-h"].as_slice()] {
        let output = harness.run(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(output.stdout, HELP.as_bytes(), "{args:?}");
        assert_eq!(output.stderr, b"", "{args:?}");
        harness.assert_python_was_not_invoked();
    }

    let output = harness.run(&["doctor", "--nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        output.stderr,
        format!("{USAGE}journal doctor: error: unexpected argument\n").as_bytes()
    );
    harness.assert_python_was_not_invoked();
}

#[test]
fn poison_remains_live_for_a_python_token() {
    let harness = Harness::new();
    let output = harness.run(&["describe"]);

    assert_eq!(output.status.code(), Some(97));
    assert!(
        harness.poison_marker.exists(),
        "describe did not invoke the poisoned interpreter"
    );
}
