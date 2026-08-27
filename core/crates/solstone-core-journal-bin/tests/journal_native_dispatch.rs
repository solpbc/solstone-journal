// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const NATIVE_STUB: &str = r#"#!/bin/sh
printf '%s\n' "${0##*/}" > "$NATIVE_DISPATCH_RECORD"
for arg in "$@"; do
    printf '%s\n' "$arg" >> "$NATIVE_DISPATCH_RECORD"
done
"#;

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
            .expect("time should be available")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-core-journal-native-dispatch-{}-{stamp}",
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

struct Harness {
    _temp: TempDir,
    binary: PathBuf,
    record: PathBuf,
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
        for helper in ["solstone-core", "solstone-core-depict"] {
            let path = bin.join(helper);
            fs::write(&path, NATIVE_STUB).expect("write native helper stub");
            make_executable(&path);
        }
        for interpreter in ["python", "python3", "pytest", "uv", "ruff"] {
            let path = bin.join(interpreter);
            fs::write(&path, POISON_INTERPRETER).expect("write poison interpreter");
            make_executable(&path);
        }
        let record = temp.path.join("native-dispatch.txt");
        let poison_marker = temp.path.join("python-invoked.txt");
        Self {
            _temp: temp,
            binary,
            record,
            poison_marker,
        }
    }

    fn run(&self, token: &str) -> String {
        self.run_args(token, &["--opaque", "has space"])
    }

    fn run_args(&self, token: &str, args: &[&str]) -> String {
        let output = self.run_process_args(token, args);
        assert!(
            output.status.success(),
            "{token} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !self.poison_marker.exists(),
            "{token} invoked a poisoned Python interpreter"
        );
        fs::read_to_string(&self.record).expect("read native dispatch record")
    }

    fn run_process_args(&self, token: &str, args: &[&str]) -> Output {
        let _ = fs::remove_file(&self.record);
        let _ = fs::remove_file(&self.poison_marker);
        Command::new(&self.binary)
            .arg(token)
            .args(args)
            .env("NATIVE_DISPATCH_RECORD", &self.record)
            .env("POISON_MARKER", &self.poison_marker)
            .env("HOME", &self._temp.path)
            .env("SOLSTONE_JOURNAL", self._temp.path.join("journal"))
            .output()
            .expect("run native journal process")
    }
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

#[test]
fn native_process_verbs_exec_their_sibling_without_python() {
    let harness = Harness::new();
    assert_eq!(
        harness.run("spl"),
        "solstone-core\nspl\nservice\n--opaque\nhas space\n"
    );
    assert_eq!(
        harness.run("grab"),
        "solstone-core\ngrab\n--opaque\nhas space\n"
    );
    assert_eq!(
        harness.run_args(
            "transfer",
            &["export", "--day", "20260203", "--output", "out.tgz"]
        ),
        "solstone-core\ntransfer\nexport\n--day\n20260203\n--output\nout.tgz\n"
    );
    assert_eq!(
        harness.run_args("transfer", &["import", "--archive", "in.tgz"]),
        "solstone-core\ntransfer\nimport\n--archive\nin.tgz\n"
    );
    assert_eq!(
        harness.run_args("transfer", &["send", "--to", "office", "--dry-run"]),
        "solstone-core\ntransfer\nsend\n--to\noffice\n--dry-run\n"
    );
    assert_eq!(
        harness.run("depict"),
        "solstone-core-depict\n--opaque\nhas space\n"
    );
    assert_eq!(
        harness.run("facet-candidates"),
        "solstone-core\nfacet-candidates\n--opaque\nhas space\n"
    );
    assert_eq!(
        harness.run("navigate"),
        "solstone-core\nnavigate\n--opaque\nhas space\n"
    );
    assert_eq!(
        harness.run("identity"),
        "solstone-core\nidentity\n--opaque\nhas space\n"
    );
}

#[test]
fn convey_and_maintenance_bypass_python() {
    let harness = Harness::new();
    assert_eq!(
        harness.run("convey"),
        "solstone-core\nconvey\n--opaque\nhas space\n"
    );
    assert_eq!(
        harness.run("maintenance"),
        "solstone-core\nmaintenance\n--opaque\nhas space\n"
    );
}
