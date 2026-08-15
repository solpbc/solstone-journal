// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

#[path = "support/python_process_control.rs"]
mod python_process_control;

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
            "solstone-core-journal-identity-cutover-{}-{stamp}",
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

#[test]
fn identity_help_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["identity", "--help"], false);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for token in ["usage: journal identity", "partner", "health", "briefing"] {
        assert!(stdout.contains(token), "missing {token:?} from {stdout:?}");
    }
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn identity_partner_help_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["identity", "partner", "--help"], false);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("usage: journal identity partner"));
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn identity_health_help_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["identity", "health", "--help"], false);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("usage: journal identity health"));
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn identity_briefing_help_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["identity", "briefing", "--help"], false);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("usage: journal identity briefing")
    );
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn identity_unknown_subcommand_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["identity", "--nonsense"], false);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        output.stderr,
        b"usage: journal identity [-h] {partner,health,briefing} ...\n\
journal identity: error: invalid choice: '--nonsense'\n"
    );
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn identity_hydrate_runs_natively() {
    let harness = Harness::new();
    let output = harness.run(&["identity"], true);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("# species\n"));
    assert!(stdout.contains("\n# partner\n\n(not present)\n"));
    assert!(!harness.journal.join("identity").exists());
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn identity_partner_write_is_supervisor_gated_natively() {
    let harness = Harness::new();
    let output = harness.run(
        &["identity", "partner", "--write", "--value", "after"],
        false,
    );

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        output.stderr,
        b"sol: solstone isn't running. Start it with 'journal up' and retry.\n"
    );
    assert!(!harness.journal.join("identity").exists());
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn identity_partner_help_is_available_when_supervisor_is_down_natively() {
    let harness = Harness::new();
    let output = harness.run(&["identity", "partner", "--help"], false);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("usage: journal identity partner"));
    assert!(!harness.journal.join("identity").exists());
    assert_python_was_not_invoked(&harness.poison_marker);
}

#[test]
fn poison_remains_live_for_a_python_token() {
    let harness = Harness::new();
    let token = python_process_control::token();
    let output = harness.run(&[token], false);

    assert_eq!(output.status.code(), Some(97));
    assert!(
        harness.poison_marker.exists(),
        "{token} did not invoke the poisoned interpreter"
    );
}
