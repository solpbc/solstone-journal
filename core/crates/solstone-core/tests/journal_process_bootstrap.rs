// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const RECORDING_MODULE: &str = r#"
import json
import logging
import os
from pathlib import Path
import sys

def main():
    Path(os.environ["BOOTSTRAP_RECORD"]).write_text(
        json.dumps({
            "argv": sys.argv,
            "cwd": os.getcwd(),
            "debug": logging.getLogger().getEffectiveLevel() == logging.DEBUG,
            "module": __name__,
            "verbose_env": os.environ.get("JOURNAL_CLI_VERBOSE"),
        }, ensure_ascii=False),
        encoding="utf-8",
    )
    print("bootstrap stdout")
    print("bootstrap stderr", file=sys.stderr)
    mode = os.environ["BOOTSTRAP_MODE"]
    if mode == "none":
        return None
    if mode == "integer":
        return 23
    if mode == "system-exit-integer":
        raise SystemExit(31)
    if mode == "system-exit-string":
        raise SystemExit("bootstrap exit string")
    raise RuntimeError(f"unknown mode: {mode}")
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
            "solstone-core-journal-bootstrap-{}-{stamp}",
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
    python_path: PathBuf,
    cwd: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new();
        let bin = temp.path.join("bin");
        let python_path = temp.path.join("python-path");
        let cwd = temp.path.join("inherited cwd");
        fs::create_dir_all(python_path.join("solstone/think"))
            .expect("create temporary Python package");
        fs::create_dir(&bin).expect("create binary directory");
        fs::create_dir(&cwd).expect("create inherited cwd");
        fs::write(python_path.join("solstone/__init__.py"), "").expect("write package init");
        fs::write(python_path.join("solstone/think/__init__.py"), "")
            .expect("write think package init");
        for module in ["service.py", "doctor.py", "backup_cli.py"] {
            fs::write(
                python_path.join("solstone/think").join(module),
                RECORDING_MODULE,
            )
            .expect("write recording process module");
        }

        let binary = bin.join("solstone-core");
        fs::copy(env!("CARGO_BIN_EXE_solstone-core"), &binary).expect("copy native binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
            .expect("make native binary executable");

        let python = repo_root().join(".venv/bin/python");
        assert!(
            python.is_file(),
            "make install must provide {}",
            python.display()
        );
        symlink(python, bin.join("python3")).expect("link sibling Python interpreter");

        Self {
            _temp: temp,
            binary,
            python_path,
            cwd,
        }
    }

    fn run(
        &self,
        name: &str,
        token: &str,
        mode: &str,
        verbose: bool,
        poison_verbose_env: bool,
    ) -> Run {
        let record = self
            .binary
            .parent()
            .expect("binary parent")
            .parent()
            .expect("test prefix")
            .join(format!("{name}.json"));
        let mut command = Command::new(&self.binary);
        command.arg("__solstone_identity=journal");
        if verbose {
            command.arg("--verbose");
        }
        command
            .arg(token)
            .args(OWNER_ARGV)
            .current_dir(&self.cwd)
            .env("PYTHONPATH", &self.python_path)
            .env("BOOTSTRAP_RECORD", &record)
            .env("BOOTSTRAP_MODE", mode)
            .env("HOME", &self._temp.path)
            .env("SOLSTONE_JOURNAL", self._temp.path.join("journal"));
        if poison_verbose_env {
            command.env("JOURNAL_CLI_VERBOSE", "ambient-poison");
        } else {
            command.env_remove("JOURNAL_CLI_VERBOSE");
        }
        let output = command.output().expect("run native journal process");
        let record: Value =
            serde_json::from_slice(&fs::read(&record).expect("read bootstrap record"))
                .expect("parse bootstrap record");
        Run { output, record }
    }
}

struct Run {
    output: Output,
    record: Value,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

const OWNER_ARGV: &[&str] = &[
    "--help",
    "-v",
    "-V",
    "0",
    "1",
    "solstone.think.service",
    "journal up",
    "owner.thing",
    "snow ☃",
];

fn assert_process_contract(
    run: &Run,
    harness: &Harness,
    expected_code: i32,
    debug: bool,
    token: &str,
    module: &str,
    preset: &[&str],
) {
    assert_eq!(run.output.status.code(), Some(expected_code));
    let expected_argv: Vec<_> = std::iter::once(format!("journal {token}"))
        .chain(preset.iter().map(|value| (*value).to_owned()))
        .chain(OWNER_ARGV.iter().map(|value| (*value).to_owned()))
        .collect();
    assert_eq!(run.record["argv"], serde_json::json!(expected_argv));
    assert_eq!(run.record["cwd"], harness.cwd.to_string_lossy().as_ref());
    assert_eq!(run.record["debug"], debug);
    assert_eq!(run.record["module"], module);
    assert_eq!(run.record["verbose_env"], Value::Null);
    assert_eq!(run.output.stdout, b"bootstrap stdout\n");
}

#[test]
fn real_python_bootstrap_preserves_process_contract() {
    let harness = Harness::new();

    let none = harness.run("none", "up", "none", false, false);
    assert_process_contract(
        &none,
        &harness,
        0,
        false,
        "up",
        "solstone.think.service",
        &["up"],
    );
    assert_eq!(none.output.stderr, b"bootstrap stderr\n");

    let integer = harness.run("integer", "doctor", "integer", true, false);
    assert_process_contract(
        &integer,
        &harness,
        23,
        true,
        "doctor",
        "solstone.think.doctor",
        &[],
    );
    assert_eq!(integer.output.stderr, b"bootstrap stderr\n");

    let exit_integer = harness.run(
        "system-exit-integer",
        "backup",
        "system-exit-integer",
        false,
        false,
    );
    assert_process_contract(
        &exit_integer,
        &harness,
        31,
        false,
        "backup",
        "solstone.think.backup_cli",
        &[],
    );
    assert_eq!(exit_integer.output.stderr, b"bootstrap stderr\n");

    let exit_string = harness.run(
        "system-exit-string",
        "up",
        "system-exit-string",
        false,
        false,
    );
    assert_process_contract(
        &exit_string,
        &harness,
        1,
        false,
        "up",
        "solstone.think.service",
        &["up"],
    );
    assert_eq!(
        exit_string.output.stderr,
        b"bootstrap stderr\nbootstrap exit string\n"
    );
}

#[test]
fn ambient_verbose_environment_cannot_enable_debug_logging() {
    let harness = Harness::new();
    let run = harness.run("ambient-verbose", "up", "none", false, true);
    assert_eq!(run.record["debug"], false);
    assert_eq!(run.record["verbose_env"], "ambient-poison");
}

#[test]
fn root_verbose_does_not_create_a_verbose_environment_override() {
    let harness = Harness::new();
    let run = harness.run("root-verbose-env", "up", "none", true, false);
    assert_eq!(run.record["debug"], true);
    assert_eq!(run.record["verbose_env"], Value::Null);
}
