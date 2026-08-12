// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_import::{
    AudioAuto, ImportPreview, ImportResult, PreviewRequest, SaveRequest, SyncBackendRequest,
    SyncGuidance, should_write_manifest,
};
use std::path::PathBuf;

const GRAMMAR: &str = include_str!("../../../fixtures/import_reference_grammar.json");

#[test]
fn contract_field_names_match_the_frozen_grammar() {
    let fixture: Value = serde_json::from_str(GRAMMAR).unwrap();
    let result_fields = string_array(&fixture, "import_result_fields");
    let preview_fields = string_array(&fixture, "import_preview_fields");

    assert_eq!(ImportResult::FIELD_NAMES.as_slice(), result_fields);
    assert_eq!(ImportPreview::FIELD_NAMES.as_slice(), preview_fields);
}

#[test]
fn manifest_write_predicate_only_suppresses_empty_failed_imports() {
    assert!(!should_write_manifest(&result(0, vec!["failure"])));
    assert!(should_write_manifest(&result(0, Vec::new())));
    assert!(should_write_manifest(&result(1, vec!["failure"])));
}

#[test]
fn preview_and_save_requests_are_distinct_types() {
    accepts_preview(PreviewRequest);
    accepts_save(SaveRequest);
}

fn string_array<'a>(fixture: &'a Value, key: &str) -> Vec<&'a str> {
    fixture[key]
        .as_array()
        .unwrap()
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .unwrap()
}

fn result(entries_written: u64, hard_failures: Vec<&str>) -> ImportResult {
    ImportResult {
        entries_written,
        entities_seeded: 0,
        files_created: Vec::new(),
        errors: Vec::new(),
        summary: String::new(),
        hard_failures: hard_failures.into_iter().map(str::to_owned).collect(),
        segments: None,
        date_range: None,
        merge_summary: None,
        principal_collision: None,
        merge_log_path: None,
        merge_staging_path: None,
        raw_retention: None,
    }
}

fn accepts_preview(_: PreviewRequest) {}

fn accepts_save(_: SaveRequest) {}

#[test]
fn sync_backend_request_confines_window_days_to_the_native_variant() {
    let path = PathBuf::from("journal");
    let request = SyncBackendRequest::Oura {
        journal_root: path.clone(),
        save: true,
        window_days: 14,
        confirmed: true,
        scheduled: false,
    };
    match request {
        SyncBackendRequest::Oura { window_days, .. } => assert_eq!(window_days, 14),
        _ => panic!("Oura request must retain window days"),
    }
    let request = SyncBackendRequest::Audio {
        journal_root: path,
        save: false,
        source_path: Some(PathBuf::from("audio")),
        force: false,
        auto: AudioAuto::Enabled,
    };
    match request {
        SyncBackendRequest::Audio { source_path, .. } => {
            assert_eq!(source_path, Some(PathBuf::from("audio")))
        }
        _ => panic!("audio request must retain its source override"),
    }
}

#[test]
fn scheduling_guidance_is_pure_text_without_a_schedule_writer_surface() {
    let guidance = SyncGuidance::new("Run this on a cadence you choose.".to_owned());
    assert_eq!(guidance.format_text(), "Run this on a cadence you choose.");
}
