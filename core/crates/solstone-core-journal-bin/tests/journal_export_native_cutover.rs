// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const POISON_INTERPRETER: &str = "#!/bin/sh\nprintf '%s\\n' \"$0\" > \"$POISON_MARKER\"\nexit 97\n";

fn locate_solstone_core_binary() -> PathBuf {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .parent()
        .expect("core")
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(workspace)
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
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["reason"] == "compiler-artifact"
            && value["target"]["name"] == "solstone-core"
            && value["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            && let Some(path) = value["executable"].as_str()
        {
            return PathBuf::from(path);
        }
    }
    panic!("cargo did not report solstone-core")
}

struct Harness {
    root: PathBuf,
    binary: PathBuf,
    journal: PathBuf,
    poison: PathBuf,
}
impl Harness {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "solstone-export-cutover-{}-{stamp}",
            std::process::id()
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let binary = bin.join("solstone-core-journal");
        fs::copy(env!("CARGO_BIN_EXE_solstone-core-journal"), &binary).expect("journal binary");
        executable(&binary);
        let core = bin.join("solstone-core");
        fs::copy(locate_solstone_core_binary(), &core).expect("core binary");
        executable(&core);
        for name in ["python", "python3"] {
            let path = bin.join(name);
            fs::write(&path, POISON_INTERPRETER).expect("poison");
            executable(&path);
        }
        let journal = root.join("journal");
        fs::create_dir_all(&journal).expect("journal");
        Self {
            poison: root.join("poison"),
            root,
            binary,
            journal,
        }
    }
    fn run(&self, args: &[&str]) -> Output {
        let _ = fs::remove_file(&self.poison);
        Command::new(&self.binary)
            .args(args)
            .env("POISON_MARKER", &self.poison)
            .env("HOME", self.root.join("home"))
            .env("SOLSTONE_JOURNAL", &self.journal)
            .output()
            .expect("run journal")
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
}

#[test]
fn export_runs_natively_without_the_poisoned_interpreter() {
    let harness = Harness::new();
    let output = harness.run(&["export", "--to", "no-such-peer", "--dry-run"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!harness.poison.exists(), "export reached Python");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("journal export: error: no peers paired")
    );
}
