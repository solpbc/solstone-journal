// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_speaker_resolve::evidence::{
    extract_meeting_participants_with_gaps, extract_screen_participants_with_gaps,
    load_segment_speakers_with_gaps, load_setting_field_with_gaps, parse_setting_names,
};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-id-evidence-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary journal");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn segment(temporary: &TempDir) -> PathBuf {
    let path = temporary.path().join("chronicle/20260808/mic/120000_300");
    fs::create_dir_all(path.join("talents")).expect("create segment");
    path
}

#[test]
fn ac1_setting_and_meeting_distinguish_missing_empty_and_invalid_sources() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    assert_eq!(load_setting_field_with_gaps(&segment), (None, vec![]));
    assert_eq!(
        extract_meeting_participants_with_gaps(temporary.path(), "20260808"),
        (Vec::new(), vec![])
    );

    fs::write(segment.join("imported_audio.jsonl"), b"\n").expect("write empty setting");
    assert_eq!(load_setting_field_with_gaps(&segment), (None, vec![]));
    fs::write(segment.join("imported_audio.jsonl"), b"{").expect("write malformed setting");
    assert_eq!(
        load_setting_field_with_gaps(&segment).1[0].reason,
        "malformed_json"
    );

    let meetings = temporary
        .path()
        .join("chronicle/20260808/talents/meetings.md");
    fs::create_dir_all(&meetings).expect("make unreadable meetings path");
    let (_, gaps) = extract_meeting_participants_with_gaps(temporary.path(), "20260808");
    assert_eq!(gaps[0].source, "meeting_day");
    assert_eq!(gaps[0].reason, "unreadable");
}

#[test]
fn ac2_speakers_partial_data_keeps_names_and_reports_wrong_shape() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    fs::write(
        segment.join("talents/speakers.json"),
        br#"["Alice", 4, "  "]"#,
    )
    .expect("write speakers");
    let (names, gaps) = load_segment_speakers_with_gaps(&segment);
    assert_eq!(names, ["Alice"]);
    assert_eq!(gaps[0].source, "speakers");
    assert_eq!(gaps[0].reason, "wrong_shape");
}

#[test]
fn ac3_screen_only_keeps_person_attendees() {
    let temporary = TempDir::new();
    let segment = segment(&temporary);
    fs::write(
        segment.join("talents/screen.json"),
        json!({"entities": [
            {"type": "Person", "role": "attendee", "name": " Alice "},
            {"type": "Project", "role": "attendee", "name": "Ignored"},
            {"type": "Person", "role": "host", "name": "Ignored"}
        ]})
        .to_string(),
    )
    .expect("write screen");
    let (names, gaps) = extract_screen_participants_with_gaps(&segment);
    assert_eq!(names, ["Alice"]);
    assert!(gaps.is_empty());
}

#[test]
fn ac4_setting_parser_drops_configured_owner_variants() {
    let temporary = TempDir::new();
    let config = temporary.path().join("config/journal.json");
    fs::create_dir_all(config.parent().expect("config parent")).expect("create config parent");
    fs::write(
        config,
        json!({"identity": {"preferred": "Avery", "name": "Avery Stone", "aliases": ["A. Stone"]}})
            .to_string(),
    )
    .expect("write config");
    assert_eq!(
        parse_setting_names(temporary.path(), "Avery and Jordan at coffee").expect("parse setting"),
        ["Jordan"]
    );
    assert_eq!(
        parse_setting_names(temporary.path(), "Meeting with Priya and Mateo")
            .expect("parse setting"),
        ["Priya", "Mateo"]
    );
}
