// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::path::PathBuf;
use std::process::Command;

pub fn locate_workspace_binary(package: &str, binary: &str) -> PathBuf {
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
        .args(["-p", package, "--bin", binary, "--message-format=json"])
        .output()
        .expect("cargo build native binary should execute");
    assert!(
        output.status.success(),
        "cargo build -p {package} --bin {binary} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message.get("reason").and_then(|value| value.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let target = &message["target"];
        let is_binary = target["name"].as_str() == Some(binary)
            && target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")));
        if is_binary {
            if let Some(executable) = message.get("executable").and_then(|value| value.as_str()) {
                return PathBuf::from(executable);
            }
        }
    }
    panic!("cargo build did not report a {binary} binary artifact");
}
