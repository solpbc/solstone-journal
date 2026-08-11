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
    let helper = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/solstone-core-vulkan-probe");
    fs::copy(helper, bin.join("solstone-core-vulkan-probe")).expect("copy Vulkan helper");
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
