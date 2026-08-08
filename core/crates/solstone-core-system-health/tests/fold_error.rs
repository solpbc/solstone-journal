// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_system_health::{
    FilesystemHealthLogSource, HealthError, HealthLogSource, read_completed_since,
};

#[test]
fn completed_since_rejects_invalid_day() {
    let source = FilesystemHealthLogSource::new(std::env::temp_dir());
    assert!(read_completed_since(&source, "not-a-day", 0).is_err());
}

#[test]
fn filesystem_source_rejects_path_traversal_day() {
    let source = FilesystemHealthLogSource::new(std::env::temp_dir());
    assert!(matches!(
        source.health_log_paths("../../escape"),
        Err(HealthError::InvalidDay(day)) if day == "../../escape"
    ));
}
