// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Proves `journal observer` is served natively through a SIBLING
//! interpreter shim, not a `$PATH` shim: `sibling_python_for_executable`
//! resolves `python3`/`python` as a sibling of `current_exe()` and never
//! consults `$PATH`, so a `$PATH`-shimmed test would pass identically with
//! the shims absent and prove nothing. This harness copies the REAL built
//! `solstone-core` binary (not a recording stub) beside the journal binary,
//! so `observer list`/`observer prune` must actually succeed against real
//! logic -- not merely reach a dispatcher that recorded argv.

#![cfg(unix)]

#[path = "support/python_process_control.rs"]
mod python_process_control;

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s\n' "$0" > "$POISON_MARKER"
exit 97
"#;

/// `solstone-core` has no `[lib]` target, so it is invisible to Cargo's
/// artifact-dependency graph and `CARGO_BIN_EXE_solstone-core` is never set
/// for a sibling crate's tests. Ask Cargo directly for the binary it already
/// built (or builds now) instead of duplicating its build logic.
fn locate_solstone_core_binary() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("core dir")
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(&workspace_manifest)
        .args([
            "-p",
            "solstone-core",
            "--bin",
            "solstone-core",
            "--message-format=json",
        ])
        .output()
        .expect("cargo build solstone-core should execute");
    assert!(
        output.status.success(),
        "cargo build -p solstone-core failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message.get("reason").and_then(|v| v.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let target = &message["target"];
        if target["name"].as_str() != Some("solstone-core") {
            continue;
        }
        let is_bin = target["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")));
        if !is_bin {
            continue;
        }
        if let Some(executable) = message.get("executable").and_then(|v| v.as_str()) {
            return PathBuf::from(executable);
        }
    }
    panic!("cargo build did not report a solstone-core binary artifact");
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-core-journal-observer-cutover-{label}-{}-{stamp}",
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
        let temp = TempDir::new("bin");
        let bin = temp.path.join("bin");
        fs::create_dir(&bin).expect("create binary directory");

        let binary = bin.join("solstone-core-journal");
        fs::copy(env!("CARGO_BIN_EXE_solstone-core-journal"), &binary)
            .expect("copy native journal binary");
        make_executable(&binary);

        // The REAL solstone-core binary -- not a recording stub. If native
        // dispatch merely reached a stand-in, this would prove argv
        // plumbing but not that `observer` genuinely runs natively.
        let native = bin.join("solstone-core");
        fs::copy(locate_solstone_core_binary(), &native).expect("copy real solstone-core binary");
        make_executable(&native);

        for interpreter in ["python", "python3"] {
            let path = bin.join(interpreter);
            fs::write(&path, POISON_INTERPRETER).expect("write poison interpreter");
            make_executable(&path);
        }

        let journal = temp.path.join("journal");
        fs::create_dir_all(journal.join("apps/observer/observers")).expect("journal fixture");
        fs::write(
            journal.join("apps/observer/observers/abcdefgh.json"),
            r#"{"key":"abcdefgh12345678","name":"fixture-observer","created_at":1,"stats":{"segments_received":1,"bytes_received":2}}"#,
        )
        .expect("seed observer record");

        let poison_marker = temp.path.join("python-invoked.txt");
        Self {
            _temp: temp,
            binary,
            journal,
            poison_marker,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let _ = fs::remove_file(&self.poison_marker);
        Command::new(&self.binary)
            .args(args)
            .env("POISON_MARKER", &self.poison_marker)
            .env("HOME", self._temp.path.join("home"))
            .env("SOLSTONE_JOURNAL", &self.journal)
            .output()
            .expect("run native journal process")
    }
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

#[test]
fn observer_list_and_prune_run_natively_without_touching_the_poisoned_interpreter() {
    let harness = Harness::new();

    let list = harness.run(&["observer", "list", "--json"]);
    assert_eq!(
        list.status.code(),
        Some(0),
        "exit 97 means an interpreter was reached; exit 69 means the native sibling was missing; stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(
        !harness.poison_marker.exists(),
        "observer list invoked a poisoned Python interpreter"
    );
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("fixture-observer"),
        "native observer list must return real registry data, not a stub echo"
    );

    let prune = harness.run(&["observer", "prune", "--day", "20260101"]);
    assert_eq!(
        prune.status.code(),
        Some(0),
        "clean dry-run over an empty day; stderr: {}",
        String::from_utf8_lossy(&prune.stderr)
    );
    assert!(
        !harness.poison_marker.exists(),
        "observer prune invoked a poisoned Python interpreter"
    );
    assert!(
        String::from_utf8_lossy(&prune.stdout).contains("observer prune dry-run"),
        "native observer prune must return the real dry-run report"
    );
}

#[test]
fn the_poison_is_live_a_still_python_token_actually_reaches_it() {
    let harness = Harness::new();
    let token = python_process_control::token();
    let output = harness.run(&[token]);
    assert_eq!(
        output.status.code(),
        Some(97),
        "{token} is Python-routed; if this isn't 97 the poison isn't live and the cutover proof above is meaningless"
    );
    assert!(
        harness.poison_marker.exists(),
        "{token} did not invoke the poisoned interpreter"
    );
}
