// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

#[test]
fn service_legacy_evidence_is_standalone_and_has_one_named_gate() {
    let root = repository_root();
    let workspace =
        fs::read_to_string(root.join("core/Cargo.toml")).expect("root Cargo manifest reads");
    let members = workspace
        .split_once("members = [")
        .and_then(|(_, tail)| tail.split_once("]\n"))
        .map(|(members, _)| members)
        .expect("workspace members block is recognizable");
    assert!(
        !members.contains("solstone-core-service-legacy-evidence"),
        "heavy evidence crate must not be a root workspace member"
    );

    let root_metadata_output = Command::new("cargo")
        .args([
            "metadata",
            "--manifest-path",
            "core/Cargo.toml",
            "--locked",
            "--format-version",
            "1",
            "--no-deps",
        ])
        .current_dir(&root)
        .output()
        .expect("root cargo metadata runs");
    assert!(
        root_metadata_output.status.success(),
        "root cargo metadata must succeed: {}",
        String::from_utf8_lossy(&root_metadata_output.stderr)
    );
    let root_metadata: Value =
        serde_json::from_slice(&root_metadata_output.stdout).expect("root metadata JSON parses");
    let selected_ids = root_metadata["workspace_members"]
        .as_array()
        .expect("workspace_members array");
    let default_ids = root_metadata["workspace_default_members"]
        .as_array()
        .expect("workspace_default_members array");
    for ids in [selected_ids, default_ids] {
        assert!(
            ids.iter().all(|id| !id
                .as_str()
                .expect("package id string")
                .contains("solstone-core-service-legacy-evidence")),
            "ordinary root Cargo selection must exclude the evidence package"
        );
    }
    assert!(
        workspace.contains("exclude = [\"crates/solstone-core-service-legacy-evidence\"]"),
        "root workspace must explicitly exclude the standalone evidence crate"
    );

    let standalone = root.join("core/crates/solstone-core-service-legacy-evidence/Cargo.toml");
    let standalone_text = fs::read_to_string(&standalone).expect("standalone manifest reads");
    assert!(standalone_text.contains("[workspace]"));
    assert!(!standalone_text.contains("workspace = true"));

    let output = Command::new("cargo")
        .args([
            "metadata",
            "--manifest-path",
            standalone.to_str().expect("manifest path is UTF-8"),
            "--locked",
            "--format-version",
            "1",
            "--no-deps",
        ])
        .current_dir(&root)
        .output()
        .expect("standalone cargo metadata runs");
    assert!(
        output.status.success(),
        "standalone evidence manifest must remain directly selectable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("metadata JSON parses");
    let packages = metadata["packages"].as_array().expect("packages array");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"], "solstone-core-service-legacy-evidence");

    let makefile = fs::read_to_string(root.join("Makefile")).expect("Makefile reads");
    assert!(makefile.contains("check-service-legacy-evidence:"));
    assert!(makefile.contains(
        "cargo test --manifest-path core/crates/solstone-core-service-legacy-evidence/Cargo.toml"
    ));

    for target in [
        "check-service-legacy-evidence",
        "service-legacy-evidence-capture",
    ] {
        let dry_run = Command::new("make")
            .args(["-n", target])
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&root)
            .output()
            .expect("uv-free make dry run starts");
        assert!(
            dry_run.status.success(),
            "{target} must be selectable without uv: {}",
            String::from_utf8_lossy(&dry_run.stderr)
        );
    }

    let staged = "/tmp/service-legacy-staged-evidence-control";
    let resolved = Command::new("make")
        .args([
            "-s",
            "--no-print-directory",
            "--eval",
            r#"print-service-evidence-root: ; @printf "%s\n" "$(SERVICE_LEGACY_EVIDENCE_ROOT)""#,
            "print-service-evidence-root",
            "UV=/bin/true",
        ])
        .env("SERVICE_LEGACY_EVIDENCE_ROOT", staged)
        .current_dir(&root)
        .output()
        .expect("make resolves the staged evidence root");
    assert!(
        resolved.status.success(),
        "make failed to resolve staged evidence root: {}",
        String::from_utf8_lossy(&resolved.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&resolved.stdout).trim(), staged);
}
