// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::Command;

#[test]
fn setup_ui_dom_contract() {
    match Command::new("node").arg("--version").output() {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            panic!("setup ui DOM harness requires node")
        }
        Err(error) => panic!("node availability probe failed: {error}"),
        Ok(output) if !output.status.success() => panic!(
            "node availability probe failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ),
        Ok(_) => {}
    }
    for mode in [
        None,
        Some("--dark"),
        Some("--dark-resume"),
        Some("--other-account"),
        Some("--access-notice"),
    ] {
        let mut command = Command::new("node");
        command.arg(format!(
            "{}/tests/setup_ui_dom.mjs",
            env!("CARGO_MANIFEST_DIR")
        ));
        if let Some(mode) = mode {
            command.arg(mode);
        }
        let output = command.output().expect("setup ui DOM harness starts");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "setup ui DOM harness failed:\nstdout:\n{}\nstderr:\n{}",
            stdout,
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            stdout.starts_with("DOM CASES: ") && stdout.contains(" passed"),
            "setup ui DOM harness did not report its internal case count:\n{stdout}"
        );
        println!("{}", stdout.trim());
    }
}
