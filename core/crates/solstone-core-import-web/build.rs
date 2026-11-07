// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

include!("../../build-support/convey_asset_records.rs");

// What this crate serves to the dashboard, for the shell to hash and rewrite.
fn main() {
    let mut records = AssetRecords::new();
    records
        .file("/app/import/workspace", "assets/workspace.html")
        .tree(
            "/app/import/static",
            "assets",
            &["workspace.html", "guides/"],
        );
    // The guides are read by name through the import API, one file each.
    for entry in std::fs::read_dir("assets/guides").expect("guides directory") {
        let name = entry.expect("guide entry").file_name();
        let name = name.to_string_lossy();
        let stem = name.strip_suffix(".md").expect("a guide is markdown");
        records.file(
            &format!("/app/import/api/guide/{stem}"),
            &format!("assets/guides/{name}"),
        );
    }
    println!("cargo:rerun-if-changed=assets/guides");
    records.emit();
}
