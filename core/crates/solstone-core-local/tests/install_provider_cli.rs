// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native CLI contract for `solstone-core install-provider` on staged journals.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(target_os = "linux")]
use serde_json::{Value, json};
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

fn native(binary: &Path, journal: &Path, name: &str) -> Output {
    Command::new(binary)
        .args(["install-provider", name])
        .current_dir(repository_root())
        .env("SOLSTONE_JOURNAL", journal)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run native install-provider")
}

#[cfg(target_os = "linux")]
fn stage_parakeet(journal: &Path, cpu_executable: bool) {
    let key = pins::parakeet_artifact_key(std::env::consts::OS, std::env::consts::ARCH)
        .expect("host supports Parakeet");
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

// Expected CLI text from install_provider.rs::PARAKEET_DOWNLOAD_DISCLOSURE and
// the ready / held-mismatch arms (2026-08-16).
#[cfg(target_os = "linux")]
const PARAKEET_DOWNLOAD_DISCLOSURE: &str = "parakeet-cpp fetches two artifacts into this journal's provider cache before it can run, both from updates.solstone.app: the parakeet.cpp server binary (MIT) and the speech model (CC-BY-4.0). see THIRD_PARTY_NOTICES.md.";

#[cfg(target_os = "linux")]
#[test]
fn ready_provider_prints_status_without_a_fetch() {
    let journal = tempfile::tempdir().unwrap();
    stage_parakeet(journal.path(), true);
    let output = native(&locate_solstone_core_binary(), journal.path(), "parakeet");
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(PARAKEET_DOWNLOAD_DISCLOSURE), "{stderr}");
    assert!(stderr.contains("parakeet already installed"), "{stderr}");
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    // Recorded from install_provider.rs::render_status of status::idle_status
    // on 2026-08-16. The ready arm reads on-disk status; this staged journal
    // has none, so every field is the idle default. No field is host-dependent.
    assert_eq!(
        status,
        json!({
            "schema_version": 1,
            "provider": "parakeet",
            "revision": 0,
            "install_state": "idle",
            "attempt_id": null,
            "target_fingerprint_json": null,
            "target_fingerprint_sha256": null,
            "started_at": null,
            "last_transition_at": null,
            "last_progress_at": null,
            "completed_at": null,
            "progress_bytes_received": null,
            "progress_bytes_total": null,
            "install_error": null,
            "error_code": null,
            "owner": null,
        })
    );
}

#[test]
fn held_mismatched_attempt_refuses() {
    let journal = tempfile::tempdir().unwrap();
    let mut attempt = status::idle_status("parakeet");
    attempt.target_fingerprint_sha256 = Some("other-target".to_owned());
    let attempt = status::transition(attempt, "resolving", None, None).unwrap();
    status::write_status(journal.path(), attempt).unwrap();
    let held = lease::acquire(journal.path(), "parakeet").unwrap().unwrap();
    let output = native(&locate_solstone_core_binary(), journal.path(), "parakeet");
    drop(held);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("parakeet install already running for a different target"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn host_ineligible_preflight_exits_one_with_empty_stdout() {
    let journal = tempfile::tempdir().unwrap();
    stage_parakeet(journal.path(), false);
    let output = native(&locate_solstone_core_binary(), journal.path(), "parakeet");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "{:?}", output.stdout);
}
