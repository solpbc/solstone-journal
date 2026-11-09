// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::Command;

#[test]
fn read_resilience_dom_contract() {
    let output = Command::new("node")
        .arg(format!(
            "{}/tests/read_resilience_dom.js",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("read resilience DOM harness starts (requires node)");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "read resilience DOM harness failed:\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        stdout.starts_with("DOM CASES: ") && stdout.contains(" passed"),
        "read resilience DOM harness did not report its case count:\n{stdout}"
    );
    println!("{}", stdout.trim());
}
