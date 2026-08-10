// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate should be nested below repo root")
        .to_path_buf()
}

fn require_uv(root: &Path) {
    let output = Command::new("uv")
        .args(["run", "--no-sync", "--frozen", "python", "--version"])
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("native sol coverage gate requires uv: {error}"));
    assert!(
        output.status.success(),
        "native sol coverage gate requires a working uv environment: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

fn run_gate(script: &str, extra_args: &[&str]) {
    let root = repo_root();
    require_uv(&root);
    let mut command = Command::new("uv");
    command
        .args(["run", "--no-sync", "--frozen", "python", script])
        .args(extra_args)
        .current_dir(root);
    let output: Output = command
        .output()
        .expect("native sol Python gate should execute");
    assert!(
        output.status.success(),
        "{script} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn native_sol_inventory_gate_passes() {
    run_gate("scripts/build_native_sol_inventory.py", &["--check"]);
}

#[test]
fn native_sol_architecture_gate_passes() {
    run_gate("scripts/check_native_sol_architecture.py", &[]);
}

#[test]
fn native_sol_coverage_gate_passes() {
    run_gate("scripts/check_native_sol_coverage.py", &[]);
}

#[test]
fn native_sol_no_python_spawn_gate_passes() {
    run_gate("scripts/check_native_sol_no_python_spawn.py", &[]);
}
