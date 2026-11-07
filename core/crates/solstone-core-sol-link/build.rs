// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

include!("../../build-support/convey_asset_records.rs");

// What this crate serves to the dashboard. The setup page carries this
// computer's setup facts, so it is recorded but never cached or rewritten.
fn main() {
    AssetRecords::new()
        .dynamic("/init", "assets/init.html")
        .emit();
}
