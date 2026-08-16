// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    for asset in [
        "assets/workspace.html",
        "assets/background.html",
        "assets/static/support.js",
    ] {
        println!("cargo:rerun-if-changed={asset}");
    }
}
