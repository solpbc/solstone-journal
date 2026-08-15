// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cross-process parity checks for the owner-facing Parakeet install verb.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
#[cfg(target_os = "linux")]
use serde_json::json;
use solstone_core_local::install::{lease, status};
#[cfg(target_os = "linux")]
use solstone_core_local::install::{manifest, pins};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let repository = repository_root();
    let venv = repository.join(".venv/bin/python");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

/// `solstone-core` is a sibling binary, not a library dependency of this
/// package. Ask Cargo for the artifact path rather than guessing target dirs.
fn locate_solstone_core_binary() -> PathBuf {
    let root = repository_root();
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(root.join("core/Cargo.toml"))
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
        if message["reason"] != "compiler-artifact"
            || message["target"]["name"] != "solstone-core"
            || !message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        {
            continue;
        }
        if let Some(executable) = message["executable"].as_str() {
            return PathBuf::from(executable);
        }
    }
    panic!("cargo build did not report a solstone-core binary artifact");
}

/// Both sides gate on the journal service before doing anything else, and a
/// temp journal has no recorded Convey port -- so without this every case below
/// would compare two supervisor refusals and pass while asserting nothing about
/// the install grammar it names. The harness establishes the condition rather
/// than the expectations being relaxed to accommodate it; `supervisor_gate_*`
/// covers the gate itself.
fn native(binary: &Path, journal: &Path, name: &str) -> Output {
    Command::new(binary)
        .args(["install-provider", name])
        .current_dir(repository_root())
        .env("SOLSTONE_JOURNAL", journal)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run native install-provider")
}

fn reference(journal: &Path, name: &str) -> Output {
    Command::new(python())
        .args(["-m", "solstone.think.install_provider", name])
        .current_dir(repository_root())
        .env("SOLSTONE_JOURNAL", journal)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run Python install-provider")
}

fn native_gated(binary: &Path, journal: &Path, name: &str, spawned: bool) -> Output {
    let mut command = Command::new(binary);
    command
        .args(["install-provider", name])
        .current_dir(repository_root())
        .env("SOLSTONE_JOURNAL", journal)
        .env_remove("SOL_SKIP_SUPERVISOR_CHECK");
    if spawned {
        command.env("SOL_SUPERVISOR_SPAWNED", "1");
    } else {
        command.env_remove("SOL_SUPERVISOR_SPAWNED");
    }
    command.output().expect("run native install-provider")
}

fn reference_gated(journal: &Path, name: &str, spawned: bool) -> Output {
    let mut command = Command::new(python());
    command
        .args(["-m", "solstone.think.install_provider", name])
        .current_dir(repository_root())
        .env("SOLSTONE_JOURNAL", journal)
        .env_remove("SOL_SKIP_SUPERVISOR_CHECK");
    if spawned {
        command.env("SOL_SUPERVISOR_SPAWNED", "1");
    } else {
        command.env_remove("SOL_SUPERVISOR_SPAWNED");
    }
    command.output().expect("run Python install-provider")
}

#[cfg(target_os = "linux")]
fn stage_parakeet(journal: &Path, cpu_executable: bool) {
    let key = pins::parakeet_artifact_key(std::env::consts::OS, std::env::consts::ARCH)
        .expect("differential host supports Parakeet");
    let paths = pins::parakeet_paths(journal, &key);
    let cpu_path = PathBuf::from(paths["binary_path_cpu"].as_str().unwrap());
    let vulkan_path = PathBuf::from(paths["binary_path_vulkan"].as_str().unwrap());
    let model_path = PathBuf::from(paths["model_path"].as_str().unwrap());
    for path in [&cpu_path, &vulkan_path, &model_path] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    for path in [&cpu_path, &vulkan_path] {
        std::fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
    }
    std::fs::write(&model_path, b"parakeet model").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(
            &cpu_path,
            std::fs::Permissions::from_mode(if cpu_executable { 0o755 } else { 0o644 }),
        )
        .unwrap();
        std::fs::set_permissions(&vulkan_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (root, unit, identity, inventory) in [
        (
            cpu_path.parent().unwrap(),
            "parakeet-server",
            pins::parakeet_backend_identity(&key, "cpu").unwrap(),
            manifest::runtime_inventory(cpu_path.parent().unwrap(), &[]).unwrap(),
        ),
        (
            vulkan_path.parent().unwrap(),
            "parakeet-server",
            pins::parakeet_backend_identity(&key, "vulkan").unwrap(),
            manifest::runtime_inventory(vulkan_path.parent().unwrap(), &[]).unwrap(),
        ),
        (
            model_path.parent().unwrap(),
            "parakeet-model",
            pins::parakeet_model_identity(),
            manifest::inventory_for_tree(model_path.parent().unwrap(), "model").unwrap(),
        ),
    ] {
        let manifest = manifest::build_manifest(
            "parakeet",
            unit,
            "target",
            json!({"pin_identity": identity}),
            inventory,
            None,
            None,
        )
        .unwrap();
        manifest::write_manifest(&manifest::artifact_manifest_path(root), &manifest).unwrap();
    }
}

#[test]
fn supervisor_gate_refuses_before_the_provider_name_matches_python() {
    // The gate runs after argument parsing and before the name check, so an
    // unsupported name still exits at the gate rather than at the name.
    let journal = tempfile::tempdir().unwrap();
    let binary = locate_solstone_core_binary();
    let native = native_gated(&binary, journal.path(), "bogus", false);
    let python = reference_gated(journal.path(), "bogus", false);
    assert_eq!(native.status.code(), Some(1));
    assert_eq!(native.status.code(), python.status.code());
    assert_eq!(native.stdout, python.stdout);
    assert_eq!(native.stderr, python.stderr);
}

#[test]
fn supervisor_gate_stays_silent_for_a_spawned_child_matching_python() {
    // A spawned child gets exit 75 and ZERO bytes of stderr; a spurious line
    // here lands in the supervisor's own spawn path.
    let journal = tempfile::tempdir().unwrap();
    let binary = locate_solstone_core_binary();
    let native = native_gated(&binary, journal.path(), "parakeet", true);
    let python = reference_gated(journal.path(), "parakeet", true);
    assert_eq!(native.status.code(), Some(75));
    assert_eq!(native.status.code(), python.status.code());
    assert!(native.stderr.is_empty(), "{:?}", native.stderr);
    assert_eq!(native.stderr, python.stderr);
    assert_eq!(native.stdout, python.stdout);
}

#[test]
fn unsupported_provider_matches_python() {
    let journal = tempfile::tempdir().unwrap();
    let native = native(&locate_solstone_core_binary(), journal.path(), "bogus");
    let python = reference(journal.path(), "bogus");
    assert_eq!(native.status.code(), python.status.code());
    assert_eq!(native.stdout, python.stdout);
    assert_eq!(native.stderr, python.stderr);
}

#[cfg(target_os = "linux")]
#[test]
fn ready_provider_matches_python_without_a_fetch() {
    let journal = tempfile::tempdir().unwrap();
    stage_parakeet(journal.path(), true);
    let native = native(&locate_solstone_core_binary(), journal.path(), "parakeet");
    let python = reference(journal.path(), "parakeet");
    assert_eq!(native.status.code(), python.status.code());
    assert_eq!(native.stderr, python.stderr);
    let native_status: Value = serde_json::from_slice(&native.stdout).unwrap();
    let python_status: Value = serde_json::from_slice(&python.stdout).unwrap();
    assert_eq!(native_status, python_status);
}

#[test]
fn held_mismatched_attempt_matches_python() {
    let journal = tempfile::tempdir().unwrap();
    let mut attempt = status::idle_status("parakeet");
    attempt.target_fingerprint_sha256 = Some("other-target".to_owned());
    let attempt = status::transition(attempt, "resolving", None, None).unwrap();
    status::write_status(journal.path(), attempt).unwrap();
    let held = lease::acquire(journal.path(), "parakeet").unwrap().unwrap();
    let native = native(&locate_solstone_core_binary(), journal.path(), "parakeet");
    let python = reference(journal.path(), "parakeet");
    drop(held);
    assert_eq!(native.status.code(), python.status.code());
    assert_eq!(native.stdout, python.stdout);
    assert_eq!(native.stderr, python.stderr);
}

#[cfg(target_os = "linux")]
#[test]
fn host_ineligible_preflight_compares_exit_only() {
    let journal = tempfile::tempdir().unwrap();
    stage_parakeet(journal.path(), false);
    let native = native(&locate_solstone_core_binary(), journal.path(), "parakeet");
    let python = reference(journal.path(), "parakeet");
    // Native refuses before creating an attempt; Python persists a failed
    // attempt when its installer preflight runs, so stdout intentionally differs.
    assert_eq!(native.status.code(), python.status.code());
    assert!(native.stdout.is_empty());
    assert!(!python.stdout.is_empty());
}
