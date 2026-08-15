// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let Ok(output) = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let Ok(revision) = String::from_utf8(output.stdout) else {
        return;
    };
    let revision = revision.trim();
    if !revision.is_empty() {
        println!("cargo:rustc-env=SOLSTONE_SUPPORT_PORTAL_REVISION={revision}");
    }
}
