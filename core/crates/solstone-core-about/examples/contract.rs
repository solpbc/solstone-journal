// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    let destination = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bundle");
    std::fs::create_dir_all(&destination).expect("create bundle directory");
    for (name, bytes) in solstone_core_about::bundle_artifacts() {
        std::fs::write(destination.join(name), bytes).expect("write about artifact");
    }
}
