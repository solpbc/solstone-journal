// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use solstone_core_processing_record::vocab;
use solstone_core_system_health::{
    BODY_CARD_STREAMS, DataState, DataStateMap, FilesystemSegmentSource, HealthError,
    SENSED_TERMINAL_STATES, SegmentInput, SegmentSource, classify_segment_completion,
    day_is_complete, derive_modality_state, scan_day,
};
use tempfile::TempDir;

const DAY: &str = "20990202";
const SEGMENT_NAME_ORACLE: &str = include_str!("../../../fixtures/segment_name_oracle.json");

fn day_path(root: &Path) -> PathBuf {
    root.join("chronicle").join(DAY)
}

fn segment(root: &Path, stream: Option<&str>, name: &str) -> PathBuf {
    let path = match stream {
        Some(stream) => day_path(root).join(stream).join(name),
        None => day_path(root).join(name),
    };
    fs::create_dir_all(&path).unwrap();
    path
}

fn write(path: &Path, name: &str, contents: &str) {
    fs::write(path.join(name), contents).unwrap();
}

fn now() -> DateTime<Utc> {
    DateTime::from(SystemTime::now())
}

fn data_state(values: &[(&str, &str)]) -> DataStateMap {
    DataStateMap(BTreeMap::from_iter(values.iter().map(
        |(modality, state)| ((*modality).to_owned(), (*state).to_owned()),
    )))
}

#[test]
fn vocabulary_is_closed_without_widening_sensed_terminals() {
    assert_eq!(
        [
            DataState::Analyzed,
            DataState::Empty,
            DataState::Pending,
            DataState::Analyzing,
            DataState::Failed,
            DataState::FailedFinal,
            DataState::Purged,
            DataState::Absent,
        ]
        .map(DataState::as_str),
        [
            "analyzed",
            "empty",
            "pending",
            "analyzing",
            "failed",
            "failed_final",
            "purged",
            "absent",
        ]
    );
    assert_eq!(SENSED_TERMINAL_STATES.len(), 4);
    assert_eq!(BODY_CARD_STREAMS, ["import.apple_health", "import.oura"]);
}

#[test]
fn day_scan_reports_hand_derived_seven_segment_fixture() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let short = segment(root, Some("import.audio"), "070000_17");
    write(&short, "audio.jsonl", "{}\n");
    let audio = segment(root, Some("import.audio"), "080000_300");
    write(&audio, "audio.jsonl", "{}\n{\"start\":0}\n");
    let screen = segment(root, Some("import.screen"), "081500_300");
    write(&screen, "screen.jsonl", "{}\n{\"timestamp\":0}\n");
    let both = segment(root, Some("import.both"), "083000_300");
    write(&both, "audio.jsonl", "{}\n{\"start\":0}\n");
    write(&both, "screen.jsonl", "{}\n{\"timestamp\":0}\n");
    let browser = segment(root, Some("import.browser"), "084500_300");
    write(&browser, "browser_capture.jsonl", "present\n");
    let card = segment(root, Some("import.apple_health"), "090000_300");
    write(&card, "imported.md", "card\n");
    let default = segment(root, None, "091500_300");
    write(&default, "audio.jsonl", "{}\n{\"start\":0}\n");

    let (audio_ranges, screen_ranges, segments) =
        scan_day(&FilesystemSegmentSource, root, DAY, now()).unwrap();

    assert_eq!(
        audio_ranges,
        [
            ("07:00".into(), "07:15".into()),
            ("08:00".into(), "08:15".into()),
            ("08:30".into(), "08:45".into()),
            ("09:15".into(), "09:30".into())
        ]
    );
    assert_eq!(screen_ranges, [("08:15".into(), "08:45".into())]);
    assert_eq!(segments.len(), 7);
    assert_eq!(segments[0].types, ["audio"]);
    assert_eq!(segments[0].start, "07:00");
    assert_eq!(segments[0].end, "07:01");
    assert_eq!(segments[1].types, ["audio"]);
    assert_eq!(segments[2].types, ["screen"]);
    assert_eq!(segments[3].types, ["audio", "screen"]);
    assert_eq!(segments[4].types, ["browser"]);
    assert_eq!(segments[5].types, ["markdown"]);
    assert_eq!(segments[6].stream, "_default");
}

#[test]
fn markdown_and_pdf_branches_preserve_audio_participation() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let ordinary = segment(root, Some("import.notes"), "100000_300");
    write(&ordinary, "imported.md", "owner text\n");
    let with_pdf = segment(root, Some("import.oura"), "101500_300");
    write(&with_pdf, "imported.md", "card text\n");
    write(&with_pdf, "attachment.PDF", "pdf\n");

    let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, DAY, now()).unwrap();
    assert_eq!(segments[0].types, ["audio"]);
    assert_eq!(segments[0].data_state.0["audio"], "analyzed");
    assert_eq!(segments[1].types, ["audio"]);
    let completion = classify_segment_completion(
        &segments
            .iter()
            .cloned()
            .map(SegmentInput::from)
            .collect::<Vec<_>>(),
        &BTreeMap::new(),
    );
    assert_eq!(completion.not_thought, 2);
}

#[test]
fn raw_name_parse_drops_decorated_directory_but_keeps_canonical_sibling() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let oracle: serde_json::Value = serde_json::from_str(SEGMENT_NAME_ORACLE).unwrap();
    let decorated_name = "093000_300_summary";
    assert!(
        oracle["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| { row["name"] == decorated_name && row["rust_key"] == "093000_300" })
    );
    let canonical = segment(root, Some("import.audio"), "093000_300");
    write(&canonical, "audio.jsonl", "{}\n{\"start\":0}\n");
    let decorated = segment(root, Some("import.audio"), decorated_name);
    write(&decorated, "audio.jsonl", "{}\n{\"start\":0}\n");

    let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, DAY, now()).unwrap();
    assert_eq!(
        segments
            .iter()
            .map(|segment| &segment.key)
            .collect::<Vec<_>>(),
        ["093000_300"]
    );
}

struct FailingSource;

impl SegmentSource for FailingSource {
    fn segments(
        &self,
        _journal: &Path,
        _day: &str,
    ) -> Result<Vec<solstone_core_journal_io::Segment>, HealthError> {
        Err(HealthError::Source(
            "injected enumeration failure".to_owned(),
        ))
    }
}

#[test]
fn equal_start_sort_is_stream_deterministic_and_enumeration_errors_propagate() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let zeta = segment(root, Some("zeta"), "120000_300");
    let alpha = segment(root, Some("alpha"), "120000_300");
    write(&zeta, "audio.jsonl", "{}\n{\"start\":0}\n");
    write(&alpha, "audio.jsonl", "{}\n{\"start\":0}\n");

    let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, DAY, now()).unwrap();
    assert_eq!(
        segments
            .iter()
            .map(|segment| &segment.stream)
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
    assert!(scan_day(&FailingSource, root, DAY, now()).is_err());
}

#[test]
fn marker_ladder_uses_injected_time_and_processing_record_precedence() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path();
    let instant = now();
    let marker = path.join(".analyzing_audio");
    fs::write(&marker, "{}\n").unwrap();
    fs::File::open(&marker)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(SystemTime::from(instant - Duration::seconds(1_800))),
        )
        .unwrap();
    assert_eq!(
        derive_modality_state(path, "audio", false, true, false, None, instant),
        DataState::Analyzing
    );
    assert_eq!(
        derive_modality_state(
            path,
            "audio",
            false,
            true,
            false,
            None,
            instant + Duration::seconds(1)
        ),
        DataState::Failed
    );
    fs::write(&marker, "not-json").unwrap();
    assert_eq!(
        derive_modality_state(path, "audio", false, true, false, None, instant),
        DataState::Failed
    );

    let failed_two = json!({"state":vocab::STATE_FAILED, "attempts":2});
    let failed_three = json!({"state":vocab::STATE_FAILED, "attempts":3});
    let corrupt = json!({"state":vocab::STATE_FAILED, "reason_code":vocab::REASON_CORRUPT_INPUT, "attempts":0});
    assert_eq!(
        derive_modality_state(path, "audio", true, true, false, Some(&failed_two), instant),
        DataState::Failed
    );
    assert_eq!(
        derive_modality_state(
            path,
            "audio",
            true,
            true,
            false,
            Some(&failed_three),
            instant
        ),
        DataState::FailedFinal
    );
    assert_eq!(
        derive_modality_state(path, "audio", true, true, false, Some(&corrupt), instant),
        DataState::FailedFinal
    );
    let empty = json!({"state":vocab::STATE_EMPTY});
    assert_eq!(
        derive_modality_state(path, "audio", true, true, false, Some(&empty), instant),
        DataState::Analyzed
    );
    let completion = classify_segment_completion(
        &[SegmentInput {
            key: "failed".into(),
            stream: "one".into(),
            data_state: data_state(&[("audio", "failed_final")]),
        }],
        &BTreeMap::new(),
    );
    assert_eq!(completion.exhausted, ["failed"]);
}

#[test]
fn bounded_jsonl_probes_and_processing_record_fallbacks_are_conservative() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let bounded = segment(root, Some("bounded"), "130000_300");
    write(
        &bounded,
        "audio.jsonl",
        &("x".repeat(vocab::MAX_FIRST_ROW_BYTES) + "\n{\"start\":0}\n"),
    );
    let non_utf8 = segment(root, Some("nonutf8"), "131500_300");
    fs::write(non_utf8.join("audio.jsonl"), b"{\"start\":0}\n\xff").unwrap();
    let fallback = segment(root, Some("fallback"), "133000_300");
    write(&fallback, "a_audio.jsonl", "not-json\n");
    write(
        &fallback,
        "b_audio.jsonl",
        &format!(
            "\n{{\"_solstone_processing\":{{\"state\":\"{}\"}}}}\n",
            vocab::STATE_EMPTY
        ),
    );

    let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, DAY, now()).unwrap();
    assert_eq!(segments[0].data_state.0["audio"], "pending");
    assert_eq!(segments[1].data_state.0["audio"], "pending");
    assert_eq!(segments[2].data_state.0["audio"], "empty");
}

#[test]
fn midnight_range_wraps_while_segment_end_clamps() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let late = segment(root, Some("late"), "234500_900");
    write(&late, "audio.jsonl", "{}\n{\"start\":0}\n");
    let crossing = segment(root, Some("crossing"), "235800_300");
    write(&crossing, "audio.jsonl", "{}\n{\"start\":0}\n");

    let (audio_ranges, _, segments) = scan_day(&FilesystemSegmentSource, root, DAY, now()).unwrap();
    assert_eq!(audio_ranges, [("23:45".into(), "00:00".into())]);
    assert_eq!(
        segments
            .iter()
            .find(|segment| segment.key == "235800_300")
            .unwrap()
            .end,
        "23:59"
    );
}

#[test]
fn day_complete_follows_marker_presence_and_inclusive_order() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    assert!(day_is_complete(root, DAY).unwrap());
    let health = day_path(root).join("health");
    fs::create_dir_all(&health).unwrap();
    let stream = health.join("stream.updated");
    fs::write(
        &stream,
        r#"{"version":1,"generation":1,"fingerprint":null}"#,
    )
    .unwrap();
    assert!(!day_is_complete(root, DAY).unwrap());
    let daily = health.join("daily.updated");
    fs::write(&daily, r#"{"version":1,"generation":1,"fingerprint":null}"#).unwrap();
    assert!(day_is_complete(root, DAY).unwrap());
}
