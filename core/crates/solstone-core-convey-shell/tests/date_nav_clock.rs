// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::Command;

fn parse_passed_cases(stdout: &str) -> usize {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("NAV CASES: ") {
            if let Some(count_str) = rest.strip_suffix(" passed") {
                if let Ok(n) = count_str.trim().parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}

#[test]
fn date_nav_clock_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            panic!("date nav clock harness requires node")
        }
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let script_path = format!("{manifest_dir}/tests/date_nav_clock.js");

    let output = Command::new("node")
        .arg(&script_path)
        .arg(manifest_dir)
        .env("TZ", "America/Denver")
        .output()
        .expect("date nav clock harness starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "date nav clock harness failed:\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    let count = parse_passed_cases(&stdout);
    assert!(
        count > 0,
        "date nav clock harness reported 0 passed cases:\n{stdout}"
    );
    for line in stdout.lines() {
        if line.starts_with("NAV CASES:") {
            println!("{line}");
        }
    }
}
