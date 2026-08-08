// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_system_health::{
    FilesystemHealthLogSource, HealthLogSource, SENSED_TERMINAL_STATES,
};

#[test]
fn public_source_reports_missing_health_directory_as_empty() {
    let source = FilesystemHealthLogSource::new(std::env::temp_dir().join("does-not-exist-health"));
    assert!(source.health_log_paths("20990101").unwrap().is_empty());
    assert_eq!(SENSED_TERMINAL_STATES.len(), 4);
}
