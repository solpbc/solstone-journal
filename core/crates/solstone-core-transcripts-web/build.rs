// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::PathBuf;

include!("../../build-support/convey_asset_records.rs");

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace = manifest.join("assets/transcripts/workspace.html");
    let script = manifest.join("assets/transcripts/transcripts.js");
    println!("cargo:rerun-if-changed={}", workspace.display());
    println!("cargo:rerun-if-changed={}", script.display());

    let generated = format!(
        "const WORKSPACE: EmbeddedAsset = EmbeddedAsset {{ content_type: \"text/html; charset=utf-8\", bytes: include_bytes!({:?}) }};\n\
         const SCRIPT: EmbeddedAsset = EmbeddedAsset {{ content_type: \"text/javascript; charset=utf-8\", bytes: include_bytes!({:?}) }};\n",
        workspace, script,
    );
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("output dir")).join("transcripts_assets.rs"),
        generated,
    )
    .expect("generated assets write");
    // What this crate serves to the dashboard, for the shell to hash and rewrite.
    AssetRecords::new()
        .file(
            "/app/transcripts/workspace",
            "assets/transcripts/workspace.html",
        )
        .tree(
            "/app/transcripts/static",
            "assets/transcripts",
            &["workspace.html"],
        )
        .emit();
}
