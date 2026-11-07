// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

include!("../../build-support/convey_asset_records.rs");

// What this crate serves to the dashboard, for the shell to hash and rewrite.
fn main() {
    AssetRecords::new()
        .file(
            "/app/activities/workspace",
            "assets/activities/workspace.html",
        )
        .file("/app/curation/workspace", "assets/curation/workspace.html")
        .tree(
            "/app/curation/static",
            "assets/curation",
            &["workspace.html"],
        )
        .emit();
}
