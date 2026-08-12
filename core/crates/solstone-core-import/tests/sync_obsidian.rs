// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use solstone_core_import::{
    FileSyncBackend, ObsidianSyncOptions, SyncActionSeams, load_sync_state, sync_obsidian,
};

#[test]
fn vault_walk_skips_hidden_and_template_paths_and_reuses_the_stored_source() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    let vault = temporary.path().join("vault");
    fs::create_dir_all(vault.join(".obsidian")).unwrap();
    fs::create_dir_all(vault.join("Templates")).unwrap();
    fs::create_dir_all(&journal).unwrap();
    fs::write(vault.join("kept.md"), "kept").unwrap();
    fs::write(vault.join(".obsidian/hidden.md"), "hidden").unwrap();
    fs::write(vault.join("Templates/template.md"), "template").unwrap();
    let mut seams = SyncActionSeams {
        per_item_action: |_: solstone_core_import::SyncActionRequest<'_>| Ok(()),
    };

    sync_obsidian(
        &ObsidianSyncOptions {
            journal: &journal,
            save: false,
            source_path: Some(&vault),
            force: false,
        },
        &mut seams,
    )
    .unwrap();
    let state = load_sync_state(&journal, FileSyncBackend::Obsidian)
        .unwrap()
        .unwrap();
    assert!(state.files.contains_key("kept.md"));
    assert!(!state.files.contains_key(".obsidian/hidden.md"));
    assert!(!state.files.contains_key("Templates/template.md"));

    sync_obsidian(
        &ObsidianSyncOptions {
            journal: &journal,
            save: false,
            source_path: None,
            force: false,
        },
        &mut seams,
    )
    .unwrap();
}
