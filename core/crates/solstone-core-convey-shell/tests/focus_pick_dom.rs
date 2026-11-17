// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::Command;

fn parse_passed_cases(stdout: &str) -> usize {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("PICK CASES: ")
            && let Some(count_str) = rest.strip_suffix(" passed")
            && let Ok(n) = count_str.trim().parse::<usize>()
        {
            return n;
        }
    }
    0
}

#[test]
fn focus_pick_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            panic!("focus pick harness requires node")
        }
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let script_path = format!("{manifest_dir}/tests/focus_pick_dom.js");

    let output = Command::new("node")
        .arg(&script_path)
        .arg(manifest_dir)
        .output()
        .expect("focus pick harness starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "focus pick harness failed:\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    let count = parse_passed_cases(&stdout);
    assert!(
        count > 0,
        "focus pick harness reported 0 passed cases:\n{stdout}"
    );
    for line in stdout.lines() {
        if line.starts_with("PICK CASES:") {
            println!("{line}");
        }
    }
}
