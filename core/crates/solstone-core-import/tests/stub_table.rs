// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_import::MODULE_STUBS;

#[test]
fn importer_has_no_reserved_module_seams() {
    assert!(MODULE_STUBS.is_empty());
}

#[test]
fn implemented_import_modules_have_no_unimplemented_seam() {
    for implemented in [
        "audio",
        "contract",
        "detect",
        "timestamp",
        "staging",
        "metadata",
        "dedupe",
        "publish",
        "events",
        "cli_journal_source",
        "text",
        "sync_state",
        "sync_plaud",
        "sync_obsidian",
        "sync_audio",
        "connect",
        "consent_gate",
        "cli_argv",
        "cli_render",
    ] {
        assert!(
            MODULE_STUBS
                .iter()
                .all(|(module, _)| *module != implemented)
        );
    }
}
