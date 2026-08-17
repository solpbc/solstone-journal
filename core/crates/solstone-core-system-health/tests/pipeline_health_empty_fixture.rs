// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_system_health::{
    FilesystemHealthLogSource, HealthLogSource, read_terminal_states,
};

use super::corpus;

#[test]
fn fixture_journal_has_empty_health_logs() {
    let source_journal = corpus::repository_root().join("tests/fixtures/journal");
    let root = tempfile::tempdir().unwrap();
    corpus::copy_tree(&source_journal, root.path());
    let source = FilesystemHealthLogSource::new(root.path());
    assert!(
        source.health_log_paths("20250101").unwrap().is_empty(),
        "fixture health_log_paths"
    );
    assert!(
        read_terminal_states(&source, "20250101", false)
            .unwrap()
            .value
            .is_empty(),
        "fixture terminal_states"
    );
}
