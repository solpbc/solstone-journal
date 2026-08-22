// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

// This target shells out to Cargo, which is process-global setup unsuitable for the
// identity-only harness.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("core workspace root")
        .join("Cargo.toml")
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf()
}

fn built_binary() -> PathBuf {
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(workspace_manifest())
        .args(["-p", "solstone-core-sol-bin", "--message-format=json"])
        .output()
        .expect("cargo build solstone-core-sol-bin");
    assert!(
        output.status.success(),
        "cargo build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] == "compiler-artifact"
            && message["target"]["name"] == "solstone-core-sol"
            && message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            && let Some(path) = message["executable"].as_str()
        {
            return PathBuf::from(path);
        }
    }
    panic!("cargo build did not report solstone-core-sol")
}

#[test]
fn cargo_build_stages_the_public_solstone_launcher_beside_its_native_sibling() {
    let native = built_binary();
    let profile_dir = native.parent().expect("native binary profile directory");
    let launcher = profile_dir.join("solstone");
    let source = repository_root().join("scripts/root-launchers/solstone");

    assert!(
        launcher.is_file(),
        "launcher missing: {}",
        launcher.display()
    );
    assert_ne!(
        fs::metadata(&launcher)
            .expect("launcher metadata")
            .permissions()
            .mode()
            & 0o111,
        0,
        "launcher is not executable: {}",
        launcher.display()
    );
    assert_eq!(
        fs::read(&launcher).expect("read staged launcher"),
        fs::read(&source).expect("read source launcher")
    );

    let output = Command::new(&launcher)
        .arg("--help")
        .output()
        .expect("run staged launcher");
    assert!(
        output.status.success(),
        "staged launcher failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.starts_with(b"solstone - journal access CLI"),
        "unexpected staged launcher stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
