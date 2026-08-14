// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    println!("cargo:rerun-if-changed=assets/workspace.html");
    println!("cargo:rerun-if-changed=assets/health.js");
}
