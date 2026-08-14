// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace = manifest.join("assets/transcripts/workspace.html");
    println!("cargo:rerun-if-changed={}", workspace.display());

    let generated = format!(
        "const WORKSPACE: EmbeddedAsset = EmbeddedAsset {{ content_type: \"text/html; charset=utf-8\", bytes: include_bytes!({:?}) }};\n",
        workspace,
    );
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("output dir")).join("transcripts_assets.rs"),
        generated,
    )
    .expect("generated assets write");
}
