// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::Command;

#[test]
fn shell_chrome_dom_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            panic!("shell chrome DOM harness requires node")
        }
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }
    let output = Command::new("node")
        .arg(format!(
            "{}/tests/shell_chrome_dom.js",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("shell chrome DOM harness starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "shell chrome DOM harness failed:\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.starts_with("DOM CASES: ") && stdout.contains(" passed"),
        "shell chrome DOM harness did not report its internal case count:\n{stdout}"
    );
    println!("{}", stdout.trim());
}
