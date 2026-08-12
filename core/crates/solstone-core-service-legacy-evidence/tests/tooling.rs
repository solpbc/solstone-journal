// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("standalone crate has repository parent")
        .to_path_buf()
}

fn python(root: &Path, arguments: &[&str]) {
    let output = Command::new("python3")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("Python tooling control runs");
    assert!(
        output.status.success(),
        "tooling control failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn integrity_controls_reject_path_and_credential_poisons() {
    let root = repository_root();
    python(
        &root,
        &[
            "scripts/service_legacy_integrity.py",
            "self-test",
            "--fixture",
            "core/fixtures/service_legacy_evidence/raw/de2b6f0479f198c76ab14a22704a5a6be7b5b55f/linux/default.json",
            "--oracle",
            "scripts/fixtures/service_legacy_path_role_oracle.json",
        ],
    );
}

#[test]
fn historical_closure_observes_only_the_synthetic_interpreter_identity() {
    let root = repository_root();
    python(
        &root,
        &["scripts/capture_service_legacy_raw.py", "--self-test"],
    );
}

#[test]
fn cached_interpreter_requires_the_complete_recorded_tree() {
    let root = repository_root();
    python(
        &root,
        &["scripts/acquire_service_legacy_cpython.py", "--self-test"],
    );
}

#[test]
fn full_tree_exchange_control_is_atomic() {
    let root = repository_root();
    python(&root, &["scripts/service_legacy_capture.py", "--self-test"]);
}

#[test]
fn git_authority_adapter_rejects_local_rewrites() {
    let root = repository_root();
    python(&root, &["scripts/service_legacy_git.py", "--self-test"]);
}

#[test]
fn wheel_environment_is_closed_and_root_independent() {
    let root = repository_root();
    python(
        &root,
        &[
            "scripts/build_service_legacy_packaging_provenance.py",
            "--self-test",
        ],
    );
}

#[test]
fn lock_split_preserves_the_exact_resolved_closure() {
    let root = repository_root();
    python(
        &root,
        &["scripts/service_legacy_integrity.py", "lock-self-test"],
    );
}

#[test]
fn non_owned_dispatch_sources_cannot_self_certify() {
    let root = repository_root();
    python(
        &root,
        &[
            "scripts/service_legacy_integrity.py",
            "source-closure-self-test",
        ],
    );
}

#[cfg(unix)]
#[test]
fn evidence_build_rejects_symlinked_fixture_entries() {
    use std::os::unix::fs::symlink;

    let root = repository_root();
    let temporary = env::temp_dir().join(format!(
        "service-legacy-build-symlink-{}",
        std::process::id()
    ));
    fs::remove_dir_all(&temporary).ok();
    let evidence = temporary.join("evidence");
    let target = temporary.join("target");
    fs::create_dir_all(&evidence).expect("controlled evidence root creates");
    fs::write(evidence.join("manifest.json"), b"{}\n").expect("manifest writes");
    fs::write(temporary.join("outside.json"), b"{}\n").expect("outside file writes");
    symlink(temporary.join("outside.json"), evidence.join("escape.json"))
        .expect("symlink poison creates");
    let output = Command::new("cargo")
        .args([
            "check",
            "--manifest-path",
            "core/crates/solstone-core-service-legacy-evidence/Cargo.toml",
            "--locked",
        ])
        .env("CARGO_TARGET_DIR", &target)
        .env("SERVICE_LEGACY_EVIDENCE_ROOT", &evidence)
        .current_dir(&root)
        .output()
        .expect("controlled evidence build runs");
    fs::remove_dir_all(&temporary).ok();
    assert!(
        !output.status.success(),
        "symlinked evidence unexpectedly built"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("evidence tree contains a symlink"),
        "build failed at the wrong guard: {stderr}"
    );
}
