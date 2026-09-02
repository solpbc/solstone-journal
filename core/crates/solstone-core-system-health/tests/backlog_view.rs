// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use solstone_core_system_health::{
    BACKLOG_DEFAULT_WINDOW, BACKLOG_STATE_COMPLETE, BACKLOG_STATE_PENDING, BACKLOG_STATE_STUCK,
    BACKLOG_STATE_UNKNOWN, BacklogDay, BacklogError, BacklogUnit, BacklogView, BackoffSummary,
    CappedDailySummary, CappedDailyUnit, FilesystemHealthLogSource, FilesystemSegmentSource,
    HealthError, HealthLogSource, MODALITY_INPUT_AGED_MS, SegmentRepairSummary, SegmentSource,
    read_backlog_view, read_backoff_summary, read_segment_repair_attempted,
    read_segment_repair_summary,
};
use tempfile::TempDir;

const NOW_MS: i64 = 1_800_000_000_000;

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp_millis(NOW_MS).unwrap()
}

fn health(root: &Path, day: &str, lines: &[&str]) {
    let directory = root.join("chronicle").join(day).join("health");
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("001.jsonl"), lines.join("\n") + "\n").unwrap();
}

fn screen_segment(root: &Path, day: &str, segment: &str) -> PathBuf {
    let path = root.join("chronicle").join(day).join(segment);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("screen.jsonl"), "{}\n{\"timestamp\":0}\n").unwrap();
    path
}

fn raw_audio_segment(root: &Path, day: &str, segment: &str, modified: SystemTime) -> PathBuf {
    let path = root.join("chronicle").join(day).join(segment);
    fs::create_dir_all(&path).unwrap();
    let audio = path.join("audio.wav");
    fs::write(&audio, b"fixture").unwrap();
    fs::File::open(&audio)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    path
}

fn pending_audio_jsonl_segment(
    root: &Path,
    day: &str,
    segment: &str,
    modified: SystemTime,
) -> PathBuf {
    let path = root.join("chronicle").join(day).join(segment);
    fs::create_dir_all(&path).unwrap();
    let jsonl = path.join("audio.jsonl");
    fs::write(&jsonl, "{}\n").unwrap();
    fs::File::open(&jsonl)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    path
}

fn ms_ago(ms_before_now: i64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(u64::try_from(NOW_MS - ms_before_now).unwrap())
}

fn incomplete(root: &Path, day: &str, ms: i64) {
    incomplete_at(
        root,
        day,
        SystemTime::UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap()),
    );
}

fn incomplete_at(root: &Path, day: &str, modified: SystemTime) {
    let path = root
        .join("chronicle")
        .join(day)
        .join("health/stream.updated");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "stream\n").unwrap();
    fs::File::open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

fn view(root: &Path, window: usize) -> BacklogView {
    read_backlog_view(
        &FilesystemHealthLogSource::new(root),
        &FilesystemSegmentSource,
        root,
        window,
        now(),
    )
    .unwrap()
}

#[test]
fn complete_pending_and_stuck_days_keep_python_state_and_counts() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("chronicle/20990101")).unwrap();

    let pending_day = "20990102";
    screen_segment(root, pending_day, "120000_60");
    health(
        root,
        pending_day,
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}"#,
        ],
    );
    incomplete(root, pending_day, NOW_MS - 1_000);

    let stuck_day = "20990103";
    screen_segment(root, stuck_day, "120000_60");
    health(
        root,
        stuck_day,
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}"#,
            r#"{"event":"talent.fail","ts":2000,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents","reason_code":"no_output"}"#,
            r#"{"event":"talent.fail","ts":3000,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents","reason_code":"no_output"}"#,
            r#"{"event":"talent.fail","ts":4000,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents","reason_code":"no_output"}"#,
        ],
    );
    incomplete(root, stuck_day, 4_000);

    let result = view(root, 30);
    assert_eq!(result.days.len(), 3);
    assert_eq!(result.days[0].state, BACKLOG_STATE_STUCK);
    assert_eq!(result.days[1].state, BACKLOG_STATE_PENDING);
    assert_eq!(result.days[2].state, BACKLOG_STATE_COMPLETE);
    assert_eq!((result.pending_days, result.stuck_days), (1, 1));
    assert_eq!(result.oldest_pending_day.as_deref(), Some(pending_day));
    assert!(result.errors.is_empty());
    assert!(!result.degraded);
    assert_eq!(
        result.days[0],
        BacklogDay {
            day: stuck_day.to_owned(),
            state: BACKLOG_STATE_STUCK.to_owned(),
            segments: 1,
            units: 1,
            not_sensed: 0,
            why: vec![BacklogUnit {
                mode: "segment".to_owned(),
                name: "documents".to_owned(),
                facet: None,
                stream: Some("_default".to_owned()),
                segment: Some("120000_60".to_owned()),
                why: "failed".to_owned(),
                reason_code: Some("no_output".to_owned()),
                provider: None,
                model: None,
                trailing_fail_count: 3,
                last_fail_ts: Some(4_000),
                stuck: true,
            }],
            reason: Some("failing_step".to_owned()),
            reason_code: Some("no_output".to_owned()),
            provider: None,
            model: None,
            error: None,
            backoff: None,
            segment_repair: None,
            capped_daily: None,
        }
    );
}

#[test]
fn repair_reader_honors_fingerprint_and_malformed_state_becomes_unknown_day() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    fs::create_dir_all(root.join("chronicle").join(day)).unwrap();
    let state = root.join("health/catchup-state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        &state,
        json!({"version":1,"entries":{format!("{day}:segment-repair"):{
            "fingerprint":"wrong", "attempts":1, "consecutive_non_completion":1
        }}})
        .to_string(),
    )
    .unwrap();
    assert!(read_segment_repair_summary(root, day).is_none());
    assert!(read_segment_repair_attempted(root, day));

    fs::write(&state, "not json").unwrap();
    let result = view(root, 1);
    assert_eq!(result.days[0].state, BACKLOG_STATE_UNKNOWN);
    assert_eq!(
        result.days[0].error.as_ref().unwrap().stage,
        "segment_repair"
    );
    assert!(result.errors.is_empty());
    assert!(result.degraded);
}

#[test]
fn catchup_readers_project_backoff_and_progressing_repair() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    fs::create_dir_all(root.join("chronicle").join(day)).unwrap();
    let fingerprint = solstone_core_system::catchup::read_raw_input_fingerprint(root, day).unwrap();
    let state = root.join("health/catchup-state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        state,
        json!({"version":1,"entries":{
            format!("{day}:daily-catchup"):{"entered_backoff_at":1,"attempts":3,"consecutive_non_completion":3,"last_outcome":"","next_retry_at":2.5},
            format!("{day}:segment-repair"):{"fingerprint":fingerprint,"attempts":2,"last_outcome":"progressing","cleared":false,"remaining":0,"exit_reason":"wall_clock_exceeded","next_retry_at":4.5}
        }})
        .to_string(),
    )
    .unwrap();
    let backoff = read_backoff_summary(root, day).unwrap();
    assert!(backoff.backoff_stuck);
    assert_eq!(backoff.last_outcome, "");
    assert!(read_segment_repair_attempted(root, day));
    let repair = read_segment_repair_summary(root, day).unwrap();
    assert_eq!(repair.status, "progressing");
    assert_eq!(repair.cleared, Some(Value::Bool(false)));
    assert_eq!(repair.remaining, Some(json!(0)));
    assert_eq!(
        repair.repair_reason_code.as_deref(),
        Some("wall_clock_exceeded")
    );
}

#[test]
fn progressing_repair_empty_exit_reason_falls_back_to_reason_code() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    fs::create_dir_all(root.join("chronicle").join(day)).unwrap();
    let fingerprint = solstone_core_system::catchup::read_raw_input_fingerprint(root, day).unwrap();
    let state = root.join("health/catchup-state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        state,
        json!({"version":1,"entries":{format!("{day}:segment-repair"):{
            "fingerprint":fingerprint,"attempts":1,"last_outcome":"progressing",
            "exit_reason":"","reason_code":"wall_clock_exceeded"
        }}})
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        read_segment_repair_summary(root, day)
            .unwrap()
            .repair_reason_code
            .as_deref(),
        Some("wall_clock_exceeded")
    );
}

struct FailingHealth;

impl HealthLogSource for FailingHealth {
    fn health_log_paths(&self, _day: &str) -> Result<Vec<PathBuf>, HealthError> {
        Err(HealthError::Source("injected health failure".to_owned()))
    }
}

struct FailingSegments;

impl SegmentSource for FailingSegments {
    fn segments(
        &self,
        _journal: &Path,
        _day: &str,
    ) -> Result<Vec<solstone_core_journal_io::Segment>, HealthError> {
        Err(HealthError::Source("injected segment failure".to_owned()))
    }
}

#[test]
fn terminal_and_segment_fold_errors_are_view_errors() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    incomplete(root, day, NOW_MS);
    let terminal =
        read_backlog_view(&FailingHealth, &FilesystemSegmentSource, root, 1, now()).unwrap();
    assert_eq!(
        terminal.days[0].error.as_ref().unwrap().stage,
        "terminal_states"
    );
    assert_eq!(terminal.errors.len(), 1);
    assert_eq!(terminal.days[0].state, BACKLOG_STATE_UNKNOWN);
    assert!(terminal.degraded);
    assert_eq!(terminal.oldest_pending_day, None);

    let segment = read_backlog_view(
        &FilesystemHealthLogSource::new(root),
        &FailingSegments,
        root,
        1,
        now(),
    )
    .unwrap();
    assert_eq!(
        segment.days[0].error.as_ref().unwrap().stage,
        "segment_completion"
    );
    assert_eq!(segment.errors.len(), 1);
    assert_eq!(segment.days[0].state, BACKLOG_STATE_UNKNOWN);
    assert!(segment.degraded);
    assert_eq!(segment.oldest_pending_day, None);
}

#[test]
fn malformed_lines_are_counted_for_each_successful_health_fold() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    screen_segment(root, day, "120000_60");
    health(
        root,
        day,
        &[
            "{malformed",
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}"#,
        ],
    );
    incomplete(root, day, NOW_MS);
    let result = view(root, 1);
    assert_eq!(result.malformed_line_count, 2);
}

#[test]
fn default_and_explicit_windows_are_bounded_and_echoed() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    for day in 1..=31 {
        fs::create_dir_all(root.join("chronicle").join(format!("209901{day:02}"))).unwrap();
    }
    let default = view(root, BACKLOG_DEFAULT_WINDOW);
    assert_eq!(default.window, BACKLOG_DEFAULT_WINDOW);
    assert_eq!(default.days.len(), BACKLOG_DEFAULT_WINDOW);
    let explicit = view(root, 2);
    assert_eq!(explicit.window, 2);
    assert_eq!(explicit.days.len(), 2);
}

#[test]
fn complete_days_surface_capped_daily_and_repair_escalation() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    health(
        root,
        day,
        &[
            r#"{"event":"talent.fail","ts":1,"mode":"daily","name":"summary","reason_code":"provider_request_rejected"}"#,
        ],
    );
    let fingerprint = solstone_core_system::catchup::read_raw_input_fingerprint(root, day).unwrap();
    let state = root.join("health/catchup-state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        state,
        json!({"version":1,"entries":{format!("{day}:segment-repair"):{
            "fingerprint":fingerprint,"attempts":1,"consecutive_non_completion":1,"entered_backoff_at":1
        }}})
        .to_string(),
    )
    .unwrap();
    let result = view(root, 1);
    let value = &result.days[0];
    assert_eq!(value.state, BACKLOG_STATE_STUCK);
    assert_eq!(value.reason.as_deref(), Some("segment_repair_stuck"));
    assert_eq!(value.capped_daily.as_ref().unwrap().count, 1);
    assert_eq!(
        value.capped_daily.as_ref().unwrap().unit.reason_code,
        "provider_request_rejected"
    );
}

#[test]
fn capped_daily_read_error_is_a_local_unknown_not_a_view_error() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("chronicle/20990101")).unwrap();
    let result =
        read_backlog_view(&FailingHealth, &FilesystemSegmentSource, root, 1, now()).unwrap();
    assert_eq!(result.days[0].state, BACKLOG_STATE_UNKNOWN);
    assert_eq!(result.days[0].error.as_ref().unwrap().stage, "capped_daily");
    assert!(result.errors.is_empty());
}

#[test]
fn backoff_makes_an_otherwise_empty_incomplete_day_stuck() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    incomplete(root, day, NOW_MS);
    let state = root.join("health/catchup-state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        state,
        json!({"version":1,"entries":{format!("{day}:daily-catchup"):{
            "entered_backoff_at":1,"attempts":3,"consecutive_non_completion":3,"last_outcome":"timeout","next_retry_at":2
        }}})
        .to_string(),
    )
    .unwrap();
    let result = view(root, 1);
    assert_eq!(result.days[0].state, BACKLOG_STATE_STUCK);
    assert_eq!(result.days[0].reason.as_deref(), Some("catchup_backoff"));
    assert_eq!(
        result.days[0].backoff.as_ref().unwrap().last_outcome,
        "timeout"
    );
}

fn stuck_segment_day(root: &Path, day: &str, reason_code: Option<&str>) {
    screen_segment(root, day, "120000_60");
    let reason = reason_code
        .map(|reason_code| format!(",\"reason_code\":\"{reason_code}\""))
        .unwrap_or_default();
    health(
        root,
        day,
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}"#,
            &format!(
                r#"{{"event":"talent.fail","ts":2000,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents"{reason}}}"#
            ),
            &format!(
                r#"{{"event":"talent.fail","ts":3000,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents"{reason}}}"#
            ),
            &format!(
                r#"{{"event":"talent.fail","ts":4000,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents"{reason}}}"#
            ),
        ],
    );
    incomplete(root, day, 4_000);
}

fn write_repair_record(root: &Path, day: &str, record: Value) {
    let state = root.join("health/catchup-state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        state,
        json!({"version": 1, "entries": {format!("{day}:segment-repair"): record}}).to_string(),
    )
    .unwrap();
}

#[test]
fn repair_escalation_never_downgrades_a_stuck_day() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    stuck_segment_day(root, day, Some("no_output"));
    let fingerprint = solstone_core_system::catchup::read_raw_input_fingerprint(root, day).unwrap();
    write_repair_record(
        root,
        day,
        json!({
            "fingerprint": fingerprint, "attempts": 1,
            "consecutive_non_completion": 1
        }),
    );

    let result = view(root, 1);
    assert_eq!(result.days[0].state, BACKLOG_STATE_STUCK);
    assert_eq!(result.days[0].reason.as_deref(), Some("failing_step"));
}

#[test]
fn repair_reason_replaces_only_an_absent_reason_code() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    stuck_segment_day(root, day, None);
    let fingerprint = solstone_core_system::catchup::read_raw_input_fingerprint(root, day).unwrap();
    write_repair_record(
        root,
        day,
        json!({
            "fingerprint": fingerprint, "attempts": 1,
            "consecutive_non_completion": 1, "entered_backoff_at": 1
        }),
    );

    let result = view(root, 1);
    assert_eq!(result.days[0].state, BACKLOG_STATE_STUCK);
    assert_eq!(
        result.days[0].reason.as_deref(),
        Some("segment_repair_stuck")
    );
    assert_eq!(
        result.days[0].reason_code.as_deref(),
        Some("segment_repair_stuck")
    );
}

#[test]
fn repair_reason_does_not_replace_catchup_backoff_reason_code() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    incomplete(root, day, NOW_MS);
    let fingerprint = solstone_core_system::catchup::read_raw_input_fingerprint(root, day).unwrap();
    let state = root.join("health/catchup-state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(
        state,
        json!({"version":1,"entries":{
            format!("{day}:daily-catchup"):{"entered_backoff_at":1},
            format!("{day}:segment-repair"):{
                "fingerprint":fingerprint,"attempts":1,
                "consecutive_non_completion":1,"entered_backoff_at":1
            }
        }})
        .to_string(),
    )
    .unwrap();

    let result = view(root, 1);
    assert_eq!(result.days[0].state, BACKLOG_STATE_STUCK);
    assert_eq!(result.days[0].reason.as_deref(), Some("catchup_backoff"));
    assert_eq!(
        result.days[0].reason_code.as_deref(),
        Some("catchup_backoff")
    );
}

#[test]
fn repair_reader_only_emits_the_validated_status_vocabulary() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    fs::create_dir_all(root.join("chronicle").join(day)).unwrap();
    let fingerprint = solstone_core_system::catchup::read_raw_input_fingerprint(root, day).unwrap();

    for (record, expected) in [
        (
            json!({"fingerprint": fingerprint, "attempts": 1, "last_outcome": "progressing"}),
            "progressing",
        ),
        (
            json!({"fingerprint": fingerprint, "attempts": 1, "consecutive_non_completion": 1}),
            "degraded",
        ),
        (
            json!({"fingerprint": fingerprint, "attempts": 1, "consecutive_non_completion": 1, "entered_backoff_at": 1}),
            "stuck",
        ),
    ] {
        write_repair_record(root, day, record);
        assert_eq!(
            read_segment_repair_summary(root, day).unwrap().status,
            expected
        );
    }
    fs::write(root.join("health/catchup-state.json"), "not json").unwrap();
    assert_eq!(
        read_segment_repair_summary(root, day).unwrap().status,
        "unknown"
    );
}

#[test]
fn two_trailing_failures_are_not_stuck() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    screen_segment(root, day, "120000_60");
    health(
        root,
        day,
        &[
            r#"{"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"active"}"#,
            r#"{"event":"talent.fail","ts":2000,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents"}"#,
            r#"{"event":"talent.fail","ts":3000,"mode":"segment","stream":"_default","segment":"120000_60","name":"documents"}"#,
        ],
    );
    incomplete(root, day, 3_000);

    let result = view(root, 1);
    assert_eq!(result.days[0].state, BACKLOG_STATE_PENDING);
    assert!(!result.days[0].why[0].stuck);
}

#[test]
fn stream_marker_mtime_is_truncated_to_whole_milliseconds() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    stuck_segment_day(root, day, None);
    incomplete_at(
        root,
        day,
        SystemTime::UNIX_EPOCH + Duration::from_nanos(4_000_999_999),
    );

    let result = view(root, 1);
    assert!(result.days[0].why[0].stuck);
    assert_eq!(result.days[0].state, BACKLOG_STATE_STUCK);
}

#[test]
fn corrupt_raw_unit_sets_the_day_reason() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    let segment = root.join("chronicle").join(day).join("120000_60");
    fs::create_dir_all(&segment).unwrap();
    fs::write(segment.join("screen.jsonl"), "{}\n").unwrap();
    fs::write(
        segment.join(".analyze_failed_screen"),
        r#"{"reason":"marker_corrupt","failed_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    incomplete(root, day, 0);

    let result = view(root, 1);
    assert!(
        result.days[0]
            .why
            .iter()
            .any(|unit| unit.why == "corrupt_raw" && unit.stuck)
    );
    assert_eq!(result.days[0].reason.as_deref(), Some("corrupt_raw"));
}

#[test]
fn non_segment_order_uses_activity_before_stream() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    health(
        root,
        day,
        &[
            r#"{"event":"talent.fail","ts":1,"mode":"activity","name":"summary","facet":"work","stream":"a","activity":"z"}"#,
            r#"{"event":"talent.fail","ts":2,"mode":"activity","name":"summary","facet":"work","stream":"z","activity":"a"}"#,
        ],
    );
    incomplete(root, day, NOW_MS);
    let result = view(root, 1);
    assert_eq!(result.days[0].why[0].stream.as_deref(), Some("z"));
    assert_eq!(result.days[0].why[1].stream.as_deref(), Some("a"));
}

#[test]
fn custom_serialization_matches_maximal_and_minimal_documents() {
    let unit = BacklogUnit {
        mode: "daily".to_owned(),
        name: "summary".to_owned(),
        facet: None,
        stream: None,
        segment: None,
        why: "failed".to_owned(),
        reason_code: Some("no_output".to_owned()),
        provider: Some("provider".to_owned()),
        model: Some("model".to_owned()),
        trailing_fail_count: 3,
        last_fail_ts: None,
        stuck: true,
    };
    let day = BacklogDay {
        day: "20990101".to_owned(),
        state: "stuck".to_owned(),
        segments: 0,
        units: 1,
        not_sensed: 0,
        why: vec![unit],
        reason: Some("failing_step".to_owned()),
        reason_code: Some("no_output".to_owned()),
        provider: Some("provider".to_owned()),
        model: Some("model".to_owned()),
        error: Some(BacklogError {
            day: "20990101".to_owned(),
            stage: "terminal_states".to_owned(),
            message: "failed".to_owned(),
        }),
        backoff: Some(BackoffSummary {
            backoff_stuck: true,
            attempts: 1,
            consecutive_non_completion: 1,
            last_outcome: String::new(),
            next_retry_at: 0.0,
        }),
        segment_repair: Some(SegmentRepairSummary {
            status: "progressing".to_owned(),
            attempts: 1,
            consecutive_non_completion: 0,
            last_outcome: Some(String::new()),
            next_retry_at: Some(0.0),
            repair_reason_code: Some("wall_clock_exceeded".to_owned()),
            timeout_seconds: Some(60),
            bounded: Some(false),
            cleared: Some(Value::Bool(false)),
            remaining: Some(json!(0)),
        }),
        capped_daily: Some(CappedDailySummary {
            count: 2,
            unit: CappedDailyUnit {
                name: "summary".to_owned(),
                facet: Some("work".to_owned()),
                reason_code: "no_output".to_owned(),
                count: 2,
            },
        }),
    };
    let maximal = serde_json::to_value(BacklogView {
        window: 1,
        days: vec![day],
        pending_days: 0,
        stuck_days: 1,
        oldest_pending_day: Some("20990101".to_owned()),
        errors: vec![BacklogError {
            day: "20990101".to_owned(),
            stage: "terminal_states".to_owned(),
            message: "failed".to_owned(),
        }],
        degraded: true,
        malformed_line_count: 7,
    })
    .unwrap();
    assert_eq!(
        maximal,
        json!({
            "window": 1,
            "days": [{
                "day": "20990101", "state": "stuck", "segments": 0, "units": 1,
                "not_sensed": 0, "reason": "failing_step",
                "why": [{
                    "mode": "daily", "name": "summary", "facet": null, "stream": null,
                    "segment": null, "why": "failed", "reason_code": "no_output",
                    "provider": "provider", "model": "model", "trailing_fail_count": 3,
                    "last_fail_ts": null, "stuck": true
                }],
                "error": {"day": "20990101", "stage": "terminal_states", "message": "failed"},
                "reason_code": "no_output", "provider": "provider", "model": "model",
                "backoff_stuck": true, "backoff_attempts": 1,
                "backoff_consecutive_non_completion": 1, "backoff_last_outcome": "",
                "backoff_next_retry_at": 0.0,
                "segment_repair_status": "progressing", "segment_repair_attempts": 1,
                "segment_repair_consecutive_non_completion": 0,
                "segment_repair_last_outcome": null, "segment_repair_next_retry_at": 0.0,
                "segment_repair_reason_code": "wall_clock_exceeded",
                "segment_repair_timeout_seconds": 60, "segment_repair_bounded": false,
                "segment_repair_cleared": false, "segment_repair_remaining": 0,
                "capped_daily_unit_count": 2,
                "capped_daily_unit": {
                    "name": "summary", "facet": "work", "reason_code": "no_output", "count": 2
                }
            }],
            "pending_days": 0, "stuck_days": 1, "oldest_pending_day": "20990101",
            "errors": [{"day": "20990101", "stage": "terminal_states", "message": "failed"}],
            "degraded": true, "malformed_line_count": 7
        })
    );

    let minimal = serde_json::to_value(BacklogView {
        window: 1,
        days: vec![BacklogDay {
            day: "20990102".to_owned(),
            state: BACKLOG_STATE_COMPLETE.to_owned(),
            segments: 0,
            units: 0,
            not_sensed: 0,
            why: vec![],
            reason: None,
            reason_code: None,
            provider: None,
            model: None,
            error: None,
            backoff: None,
            segment_repair: None,
            capped_daily: None,
        }],
        pending_days: 0,
        stuck_days: 0,
        oldest_pending_day: None,
        errors: vec![],
        degraded: false,
        malformed_line_count: 0,
    })
    .unwrap();
    assert_eq!(
        minimal,
        json!({
            "window": 1,
            "days": [{
                "day": "20990102", "state": "complete", "segments": 0, "units": 0,
                "not_sensed": 0, "reason": null, "why": [], "error": null
            }],
            "pending_days": 0, "stuck_days": 0, "oldest_pending_day": null,
            "errors": [], "degraded": false, "malformed_line_count": 0
        })
    );
}

#[test]
fn empty_strings_are_omitted_at_day_and_unit_levels() {
    let value = serde_json::to_value(BacklogDay {
        day: "20990101".to_owned(),
        state: BACKLOG_STATE_PENDING.to_owned(),
        segments: 0,
        units: 1,
        not_sensed: 0,
        why: vec![BacklogUnit {
            mode: "daily".to_owned(),
            name: "summary".to_owned(),
            facet: None,
            stream: None,
            segment: None,
            why: "failed".to_owned(),
            reason_code: Some(String::new()),
            provider: Some(String::new()),
            model: Some(String::new()),
            trailing_fail_count: 0,
            last_fail_ts: None,
            stuck: false,
        }],
        reason: None,
        reason_code: Some(String::new()),
        provider: Some(String::new()),
        model: Some(String::new()),
        error: None,
        backoff: None,
        segment_repair: None,
        capped_daily: None,
    })
    .unwrap();
    let day = value.as_object().unwrap();
    let unit = value["why"][0].as_object().unwrap();
    for key in ["reason_code", "provider", "model"] {
        assert!(!day.contains_key(key), "day unexpectedly contains {key}");
        assert!(!unit.contains_key(key), "unit unexpectedly contains {key}");
    }
}

#[test]
fn in_window_raw_audio_is_not_backlog() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    raw_audio_segment(root, day, "120000_60", ms_ago(1_000));
    incomplete(root, day, NOW_MS - 1_000);
    let result = view(root, 30);
    assert_eq!(result.days[0].state, BACKLOG_STATE_COMPLETE);
    assert_eq!(result.days[0].not_sensed, 0);
    assert_eq!(result.days[0].segments, 0);
    assert_eq!(result.pending_days, 0);
}

#[test]
fn aged_raw_audio_is_pending() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    raw_audio_segment(root, day, "120000_60", ms_ago(MODALITY_INPUT_AGED_MS));
    incomplete(root, day, NOW_MS - MODALITY_INPUT_AGED_MS);
    let result = view(root, 30);
    assert_eq!(result.days[0].state, BACKLOG_STATE_PENDING);
    assert_eq!(result.days[0].not_sensed, 1);
    assert_eq!(result.days[0].segments, 1);
    assert_eq!(result.pending_days, 1);
    assert_eq!(result.oldest_pending_day.as_deref(), Some(day));
}

#[test]
fn jsonl_only_pending_uses_file_mtime_not_directory() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let fresh_day = "20990102";
    pending_audio_jsonl_segment(root, fresh_day, "120000_60", ms_ago(1_000));
    incomplete(root, fresh_day, NOW_MS - 1_000);
    let aged_day = "20990101";
    pending_audio_jsonl_segment(root, aged_day, "120000_60", ms_ago(MODALITY_INPUT_AGED_MS));
    incomplete(root, aged_day, NOW_MS - MODALITY_INPUT_AGED_MS);
    let result = view(root, 30);
    let fresh = result
        .days
        .iter()
        .find(|day| day.day == fresh_day)
        .expect("fresh jsonl day");
    let aged = result
        .days
        .iter()
        .find(|day| day.day == aged_day)
        .expect("aged jsonl day");
    assert_eq!(fresh.state, BACKLOG_STATE_COMPLETE);
    assert_eq!(fresh.not_sensed, 0);
    assert_eq!(aged.state, BACKLOG_STATE_PENDING);
    assert_eq!(aged.not_sensed, 1);
}

#[test]
fn multi_modality_not_sensed_counts_once() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    let path = raw_audio_segment(root, day, "120000_60", ms_ago(MODALITY_INPUT_AGED_MS));
    let screen = path.join("screen.webm");
    fs::write(&screen, b"fixture").unwrap();
    fs::File::open(&screen)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(ms_ago(MODALITY_INPUT_AGED_MS)))
        .unwrap();
    incomplete(root, day, NOW_MS - MODALITY_INPUT_AGED_MS);
    let result = view(root, 30);
    assert_eq!(result.days[0].state, BACKLOG_STATE_PENDING);
    assert_eq!(result.days[0].not_sensed, 1);
}

#[test]
fn screen_marker_touch_does_not_mask_aged_audio() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = "20990101";
    let path = raw_audio_segment(root, day, "120000_60", ms_ago(MODALITY_INPUT_AGED_MS));
    screen_segment(root, day, "120000_60");
    // Directory mtime is not the age source; this pins that a recent
    // directory touch from screen processing cannot hide aged audio.
    fs::File::open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(ms_ago(1_000)))
        .unwrap();
    incomplete(root, day, NOW_MS - 1_000);
    let result = view(root, 30);
    assert_eq!(result.days[0].state, BACKLOG_STATE_PENDING);
    assert_eq!(result.days[0].not_sensed, 1);
}

// Unreadable-mtime integration coverage is omitted: making metadata
// unreadable is not portable. `missing_input_mtime_counts_as_backlog`
// in backlog.rs proves the None → counts-as-backlog rule at
// `aged_not_sensed_count`, which is the layer that implements it.
