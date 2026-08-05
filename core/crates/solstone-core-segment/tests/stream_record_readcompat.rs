// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_segment::{SegmentDir, StreamHints, StreamRecord, advance_stream};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-segment-readcompat-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn python_stream_record_fixture_loads_and_advances_monotonically() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../fixtures/stream-record-readcompat.json"
    ))
    .unwrap();
    let records = fixture["records"].as_object().unwrap();
    let parsed: Vec<StreamRecord> = records
        .values()
        .map(|value| serde_json::from_value(value.clone()).unwrap())
        .collect();
    assert_eq!(parsed.len(), 3);
    assert!(parsed.iter().any(|record| record.name == "import.apple"
        && record.host.is_none()
        && record.platform.is_none()));
    assert!(parsed.iter().any(|record| record.name == "recovered"
        && record.host.is_none()
        && record.platform.is_none()));

    let original = records.get("workstation.json").unwrap();
    let original_record: StreamRecord = serde_json::from_value(original.clone()).unwrap();
    let temporary = TempDir::new();
    let state = temporary.path().join("streams/workstation.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, serde_json::to_vec(original).unwrap()).unwrap();
    let segment =
        SegmentDir::resolve(temporary.path(), "20260804", "120000_60", "workstation").unwrap();

    let advance = advance_stream(
        "workstation",
        "20260804",
        "120000_60",
        &segment,
        StreamHints::default(),
    )
    .unwrap();
    let updated: StreamRecord = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    assert_eq!(updated.created_at, original_record.created_at);
    assert_eq!(updated.seq, original_record.seq + 1);
    assert_eq!(advance.seq, updated.seq);
}
