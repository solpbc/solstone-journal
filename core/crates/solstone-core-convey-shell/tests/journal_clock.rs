// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::Command;

fn parse_passed_cases(stdout: &str) -> usize {
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("CLOCK CASES: ") {
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
fn journal_clock_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            panic!("journal clock harness requires node")
        }
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let script_path = format!("{manifest_dir}/tests/journal_clock.js");

    // Pass 1: Denver TZ
    let output_denver = Command::new("node")
        .arg(&script_path)
        .arg(manifest_dir)
        .env("TZ", "America/Denver")
        .output()
        .expect("journal clock harness starts for Denver");
    let stdout_denver = String::from_utf8_lossy(&output_denver.stdout);
    let stderr_denver = String::from_utf8_lossy(&output_denver.stderr);
    assert!(
        output_denver.status.success(),
        "journal clock Denver pass failed:\nstdout:\n{stdout_denver}\nstderr:\n{stderr_denver}",
    );
    let count_denver = parse_passed_cases(&stdout_denver);
    assert!(
        count_denver > 0,
        "journal clock Denver pass reported 0 passed cases:\n{stdout_denver}"
    );
    for line in stdout_denver.lines() {
        if line.starts_with("CLOCK CASES:") {
            println!("{line}");
        }
    }

    // Pass 2: Tokyo TZ (reverse)
    let output_tokyo = Command::new("node")
        .arg(&script_path)
        .arg(manifest_dir)
        .arg("reverse")
        .env("TZ", "Asia/Tokyo")
        .output()
        .expect("journal clock harness starts for Tokyo reverse");
    let stdout_tokyo = String::from_utf8_lossy(&output_tokyo.stdout);
    let stderr_tokyo = String::from_utf8_lossy(&output_tokyo.stderr);
    assert!(
        output_tokyo.status.success(),
        "journal clock Tokyo pass failed:\nstdout:\n{stdout_tokyo}\nstderr:\n{stderr_tokyo}",
    );
    let count_tokyo = parse_passed_cases(&stdout_tokyo);
    assert!(
        count_tokyo > 0,
        "journal clock Tokyo pass reported 0 passed cases:\n{stdout_tokyo}"
    );
    for line in stdout_tokyo.lines() {
        if line.starts_with("CLOCK CASES:") {
            println!("{line}");
        }
    }
}
