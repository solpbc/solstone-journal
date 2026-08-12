// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
#[cfg(unix)]
fn check_never_reaches_a_sibling_or_path_interpreter() {
    let temp = tempfile::tempdir().expect("temporary sibling directory");
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).expect("create bin");
    let core = bin.join("solstone-core");
    fs::copy(env!("CARGO_BIN_EXE_solstone-core"), &core).expect("copy core");
    fs::set_permissions(&core, fs::Permissions::from_mode(0o755)).expect("make core executable");
    let helper = build_vulkan_probe();
    fs::copy(&helper, bin.join("solstone-core-vulkan-probe")).expect("copy Vulkan helper");
    let marker = temp.path().join("python-invoked.txt");
    for name in ["python", "python3", "uv", "pytest", "ruff"] {
        let shim = bin.join(name);
        fs::write(
            &shim,
            "#!/bin/sh\nprintf '%s\\n' \"$0\" > \"$POISON_MARKER\"\nexit 97\n",
        )
        .expect("write poison shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))
            .expect("make shim executable");
    }
    let output = Command::new(&core)
        .arg("check")
        .arg("--json")
        .env("PATH", &bin)
        .env("POISON_MARKER", &marker)
        .env("SOLSTONE_JOURNAL", temp.path().join("journal"))
        .output()
        .expect("run check");
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 verdict");
    for name in ["platform", "gpu", "ram", "disk"] {
        assert!(
            stdout.contains(&format!("\"name\": \"{name}\"")),
            "missing {name}: {stdout}"
        );
    }
    assert!(
        stdout.contains("\"overall\": \"ok\"")
            || stdout.contains("\"overall\": \"warning\"")
            || stdout.contains("\"overall\": \"blocked\"")
    );
    assert!(
        !marker.exists(),
        "native check reached poison interpreter: {}",
        marker.display()
    );
}

/// Build the Vulkan probe and return the executable cargo actually produced.
///
/// The previous form joined a hardcoded `../../target/debug/` path and assumed
/// something had already built it. That passes in a warm worktree and fails in a
/// fresh one with `NotFound`, so `make ci` was red for every new lode. Building
/// here also makes the path correct under a non-default profile or
/// `CARGO_TARGET_DIR`. Mirrors `locate_workspace_binary` in
/// `journal_native_process_contract.rs`.
#[cfg(unix)]
fn build_vulkan_probe() -> std::path::PathBuf {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("core dir")
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(&workspace_manifest)
        .args([
            "-p",
            "solstone-core-vulkan-probe",
            "--bin",
            "solstone-core-vulkan-probe",
            "--message-format=json",
        ])
        .output()
        .expect("cargo build solstone-core-vulkan-probe should execute");
    assert!(
        output.status.success(),
        "cargo build -p solstone-core-vulkan-probe failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        if let Some(executable) = message["executable"].as_str()
            && std::path::Path::new(executable)
                .file_name()
                .is_some_and(|name| name == "solstone-core-vulkan-probe")
        {
            return std::path::PathBuf::from(executable);
        }
    }
    panic!("cargo did not report a solstone-core-vulkan-probe executable");
}
