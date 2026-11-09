// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::ErrorKind;
use std::process::Command;

#[test]
fn trash_page_dom_contract() {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let script = format!("{crate_dir}/tests/trash_dom.js");
    let output = match Command::new("node").arg(script).arg(crate_dir).output() {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            eprintln!("node is not installed; skipping the trash page DOM contract");
            return;
        }
        Err(error) => panic!("node did not start: {error}"),
    };
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
