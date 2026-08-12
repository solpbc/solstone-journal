// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn schedule_never_reaches_an_interpreter_shim() {
    let temp = tempfile::tempdir().expect("temp");
    let bin = temp.path().join("bin");
    let journal = temp.path().join("journal");
    fs::create_dir_all(&bin).expect("bin");
    fs::create_dir_all(journal.join("config")).expect("config");
    fs::create_dir_all(journal.join("health")).expect("health");
    fs::write(
        journal.join("config/journal.json"),
        br#"{"setup":{"completed_at":1}}"#,
    )
    .expect("journal config");
    fs::write(
        journal.join("config/schedules.json"),
        br#"{"x":{"cmd":["journal","heartbeat"],"every":"daily"}}"#,
    )
    .expect("schedules");
    for name in ["python", "python3", "pytest", "uv", "ruff"] {
        let shim = bin.join(name);
        fs::write(
            &shim,
            format!("#!/bin/sh\nprintf '%s' '{name}' > \"$POISON_DIR/{name}\"\nexit 97\n"),
        )
        .expect("shim");
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("shim mode");
    }
    for args in [
        ["schedule"].as_slice(),
        ["schedule", "--nonsense"].as_slice(),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(args)
            .env("PATH", &bin)
            .env("POISON_DIR", temp.path())
            .env("SOLSTONE_JOURNAL", &journal)
            .output()
            .expect("schedule runs");
        assert!(matches!(output.status.code(), Some(0 | 2)));
    }
    for name in ["python", "python3", "pytest", "uv", "ruff"] {
        assert!(!temp.path().join(name).exists(), "{name} shim was reached");
    }
}
