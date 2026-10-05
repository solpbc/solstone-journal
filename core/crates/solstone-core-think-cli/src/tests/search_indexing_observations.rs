// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use serde_json::Value;
use solstone_core_indexer_store::scan::attempt_saved_publication;
use solstone_core_journal_io::health_marker::{
    bump_stream_marker, day_marker_pair_status, publish_daily_marker_if_current,
};
use solstone_core_system_health::{
    LIMIT_TOKEN_UNOBSERVED_OUTSIDE_DAYS, read_indexing_observations,
};
use tempfile::tempdir;

use crate::run_log::RunLogWriter;

struct PermGuard(PathBuf);
impl Drop for PermGuard {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o644));
    }
}

fn setup_utc_journal(journal: &Path) {
    let config_dir = journal.join("config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("journal.json"),
        r#"{"identity":{"timezone":"UTC"}}"#,
    )
    .unwrap();
}

#[test]
fn test_search_indexing_observations_with_real_publication_attempts() {
    let dir = tempdir().unwrap();
    let journal = dir.path();
    setup_utc_journal(journal);
    let now = Utc::now();

    // Create chronicle days 20260101 through 20260131 (31 directories)
    for day_num in 1..=31 {
        let day_str = format!("202601{:02}", day_num);
        let health_dir = journal.join("chronicle").join(&day_str).join("health");
        fs::create_dir_all(&health_dir).unwrap();
    }

    // On 20260131, make day marker pair status is_complete() true
    bump_stream_marker(journal, "20260131").unwrap();
    publish_daily_marker_if_current(
        journal,
        "20260131",
        1,
        "raw-fp",
        || Ok("raw-fp".to_string()),
    )
    .unwrap();
    assert!(
        day_marker_pair_status(journal, "20260131")
            .unwrap()
            .is_complete()
    );

    // Call attempt_saved_publication on a missing file under 20260131
    let missing_path_31 = journal.join("chronicle/20260131/missing.md");
    let attempt_31 = attempt_saved_publication(journal, &missing_path_31, |_, _| {
        unreachable!("rescan closure must not be called on missing file")
    });

    let mut writer_31 = RunLogWriter::open(journal, "20260131", "test_oplog");
    let mut fields_31 = serde_json::Map::new();
    fields_31.insert("path".to_owned(), Value::String(attempt_31.path.clone()));
    fields_31.insert(
        "outcome".to_owned(),
        Value::String(attempt_31.outcome.as_str().to_owned()),
    );
    fields_31.insert(
        "warnings".to_owned(),
        Value::Array(attempt_31.warnings.into_iter().map(Value::String).collect()),
    );
    if let Some(cause) = attempt_31.cause.clone() {
        fields_31.insert("cause".to_owned(), Value::String(cause));
    }
    writer_31.log(
        "index.attempt",
        (now - Duration::seconds(1)).timestamp_millis(),
        fields_31,
    );
    writer_31.finish().unwrap();

    // Sibling oplog with mode 0o000 beside the log
    let health_dir_31 = journal.join("chronicle/20260131/health");
    let sibling_file = health_dir_31.join("sibling.jsonl");
    fs::write(&sibling_file, b"").unwrap();
    fs::set_permissions(&sibling_file, fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = PermGuard(sibling_file);

    // On 20260101, write another real attempt_saved_publication failure
    let missing_path_01 = journal.join("chronicle/20260101/missing.md");
    let attempt_01 = attempt_saved_publication(journal, &missing_path_01, |_, _| {
        unreachable!("rescan closure must not be called on missing file")
    });

    let mut writer_01 = RunLogWriter::open(journal, "20260101", "test_oplog");
    let mut fields_01 = serde_json::Map::new();
    fields_01.insert("path".to_owned(), Value::String(attempt_01.path.clone()));
    fields_01.insert(
        "outcome".to_owned(),
        Value::String(attempt_01.outcome.as_str().to_owned()),
    );
    fields_01.insert(
        "warnings".to_owned(),
        Value::Array(attempt_01.warnings.into_iter().map(Value::String).collect()),
    );
    if let Some(cause) = attempt_01.cause {
        fields_01.insert("cause".to_owned(), Value::String(cause));
    }
    writer_01.log(
        "index.attempt",
        (now - Duration::seconds(1)).timestamp_millis(),
        fields_01,
    );
    writer_01.finish().unwrap();

    let obs = read_indexing_observations(journal, now);

    // The 20260131 failure must be present with recorded display path and cause
    let failure_31 = obs.outcomes.iter().find(|o| o.path == attempt_31.path);
    assert!(failure_31.is_some());
    let f31 = failure_31.unwrap();
    assert_eq!(f31.outcome, "failed");
    assert_eq!(f31.cause, attempt_31.cause);

    // 20260101 is outside the selected 30 directories, so its failure is absent
    let failure_01 = obs.outcomes.iter().find(|o| o.path == attempt_01.path);
    assert!(failure_01.is_none());

    // Partial must be true due to unreadable sibling
    assert!(obs.diagnostics.partial);

    // Limit token unobserved_history_outside_selected_think_days must be present in lines
    assert!(
        obs.diagnostics
            .lines
            .contains(&LIMIT_TOKEN_UNOBSERVED_OUTSIDE_DAYS.to_string())
    );
}
