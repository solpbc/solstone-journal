// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_system_health::{
    FilesystemHealthLogSource, HealthLogSource, read_terminal_states,
};

fn fixture_journal() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .join("tests/fixtures/journal")
}

#[test]
fn fixture_journal_has_empty_health_logs() {
    let root = fixture_journal();
    let source = FilesystemHealthLogSource::new(&root);
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
