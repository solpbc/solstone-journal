// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use solstone_core_system_health::{BACKLOG_DEFAULT_WINDOW, BacklogView, HealthError};
use tempfile::TempDir;

use crate::document::{TokenUsage, assemble_document};
use crate::{
    ActivityTotals, BacklogViewReader, DayScan, DayStats, DocumentWriter, JournalStatsError,
    StatsDocument, run_cli,
};

const FIXTURE: &str = include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");
const DAY: &str = "20260105";
const OTHER_DAY: &str = "20260104";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 6, 12, 0, 0).unwrap()
}

fn words(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn setup() -> (TempDir, PathBuf, PathBuf) {
    let temporary = TempDir::new().unwrap();
    fs::create_dir_all(temporary.path().join("chronicle").join(DAY)).unwrap();
    let system_talent_root = temporary.path().join("system-talents");
    let apps_root = temporary.path().join("apps");
    (temporary, system_talent_root, apps_root)
}

fn empty_backlog() -> BacklogView {
    BacklogView {
        window: BACKLOG_DEFAULT_WINDOW,
        days: Vec::new(),
        pending_days: 0,
        stuck_days: 0,
        oldest_pending_day: None,
        errors: Vec::new(),
        degraded: false,
        malformed_line_count: 0,
    }
}

fn populated_backlog() -> BacklogView {
    BacklogView {
        pending_days: 15,
        stuck_days: 16,
        ..empty_backlog()
    }
}

struct EmptyBacklog;

impl BacklogViewReader for EmptyBacklog {
    fn read_backlog_view(
        &self,
        _journal_root: &Path,
        _now: DateTime<Utc>,
    ) -> Result<BacklogView, HealthError> {
        Ok(empty_backlog())
    }
}

struct FailingBacklog;

impl BacklogViewReader for FailingBacklog {
    fn read_backlog_view(
        &self,
        _journal_root: &Path,
        _now: DateTime<Utc>,
    ) -> Result<BacklogView, HealthError> {
        Err(HealthError::Source("injected backlog failure".to_owned()))
    }
}

#[derive(Default)]
struct RecordingWriter {
    document: RefCell<Option<StatsDocument>>,
}

impl RecordingWriter {
    fn document(&self) -> StatsDocument {
        self.document.borrow().clone().unwrap()
    }
}

impl DocumentWriter for RecordingWriter {
    fn write_document(
        &self,
        _path: &Path,
        payload: &StatsDocument,
    ) -> Result<(), JournalStatsError> {
        self.document.replace(Some(payload.clone()));
        Ok(())
    }
}

#[derive(Default)]
struct FailingWriter {
    calls: Cell<usize>,
}

impl DocumentWriter for FailingWriter {
    fn write_document(
        &self,
        _path: &Path,
        _payload: &StatsDocument,
    ) -> Result<(), JournalStatsError> {
        self.calls.set(self.calls.get() + 1);
        Err(JournalStatsError::Validation(
            "injected writer failure".to_owned(),
        ))
    }
}

fn run_with(
    root: &Path,
    system: &Path,
    apps: &Path,
    args: &[&str],
    reader: &dyn BacklogViewReader,
    writer: &dyn DocumentWriter,
) -> crate::CliRun {
    run_cli(&words(args), root, now(), system, apps, reader, writer)
}

fn fixture_help() -> &'static str {
    let header = "=== journal-stats --help\n";
    let start = FIXTURE.find(header).unwrap() + header.len();
    let rest = &FIXTURE[start..];
    let end = rest.find("\n=== ").unwrap_or(rest.len());
    &rest[..end]
}

fn value(document: StatsDocument) -> Value {
    serde_json::to_value(document).unwrap()
}

fn set_modified(path: &Path, modified: SystemTime) {
    fs::File::open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

#[test]
fn ac1_help_matches_real_reference_fixture() {
    let temporary = TempDir::new().unwrap();
    let reader = EmptyBacklog;
    let writer = RecordingWriter::default();
    let result = run_cli(
        &words(&["--help"]),
        temporary.path(),
        now(),
        temporary.path(),
        temporary.path(),
        &reader,
        &writer,
    );

    assert_eq!(crate::cli::HELP, fixture_help());
    assert_eq!(result.stdout, fixture_help());
    assert_eq!(result.stderr, "");
    assert_eq!(result.exit_code, 0);
}

#[test]
fn ac2_run_builds_full_expected_document() {
    let mut scan = DayScan {
        stats: DayStats {
            transcript_sessions: 1,
            transcript_segments: 2,
            transcript_duration: 3.125,
            transcript_ranges: 4,
            percept_sessions: 5,
            percept_frames: 6,
            percept_duration: 7.25,
            percept_ranges: 8,
            browser_segments: 9,
            pending_segments: 10,
            segments_pending_think: 11,
            outputs_processed: 12,
            outputs_pending: 13,
            day_bytes: 14,
            segment_fold_failed: false,
        },
        ..DayScan::default()
    };
    scan.agent_data.insert(
        "focus".to_owned(),
        ActivityTotals {
            count: 2,
            minutes: 1.125,
        },
    );
    scan.facet_data.insert(
        "work".to_owned(),
        ActivityTotals {
            count: 3,
            minutes: 2.675,
        },
    );
    scan.heatmap_data.weekday = 2;
    scan.heatmap_data.hours.insert(3, 4.5);
    scan.heatmap_data.hours.insert(4, 1.25);
    let mut scans = std::collections::BTreeMap::new();
    scans.insert(DAY.to_owned(), scan);
    let mut tokens = TokenUsage::default();
    tokens
        .by_day
        .entry(DAY.to_owned())
        .or_default()
        .entry("model".to_owned())
        .or_default()
        .insert("input".to_owned(), 17);
    tokens
        .by_model
        .entry("model".to_owned())
        .or_default()
        .insert("input".to_owned(), 17);

    assert_eq!(
        value(assemble_document(
            &scans,
            tokens,
            populated_backlog(),
            now()
        )),
        json!({
            "schema_version": 8,
            "generated_at": "2026-01-06T12:00:00.000000+00:00",
            "day_count": 1,
            "days": {DAY: {
                "transcript_sessions": 1,
                "transcript_segments": 2,
                "transcript_duration": 3.125,
                "transcript_ranges": 4,
                "percept_sessions": 5,
                "percept_frames": 6,
                "percept_duration": 7.25,
                "percept_ranges": 8,
                "browser_segments": 9,
                "pending_segments": 10,
                "segments_pending_think": 11,
                "outputs_processed": 12,
                "outputs_pending": 13,
                "day_bytes": 14
            }},
            "totals": {
                "transcript_sessions": 1,
                "transcript_segments": 2,
                "transcript_duration": 3.125,
                "transcript_ranges": 4,
                "percept_sessions": 5,
                "percept_frames": 6,
                "percept_duration": 7.25,
                "percept_ranges": 8,
                "browser_segments": 9,
                "pending_segments": 10,
                "segments_pending_think": 11,
                "outputs_processed": 12,
                "outputs_pending": 13,
                "day_bytes": 14,
                "total_transcript_duration": 3.125,
                "total_percept_duration": 7.25,
                "backlog_pending_days": 15,
                "backlog_stuck_days": 16
            },
            "heatmap": [
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 4.5, 1.25, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
            ],
            "tokens": {"by_day": {DAY: {"model": {"input": 17}}}, "by_model": {"model": {"input": 17}}},
            "talents": {"counts": {"focus": 2}, "minutes": {"focus": 1.12}, "counts_by_day": {DAY: {"focus": 2}}},
            "facets": {"counts": {"work": 3}, "minutes": {"work": 2.67}, "counts_by_day": {DAY: {"work": 3}}},
            "backlog": {
                "window": 30,
                "days": [],
                "pending_days": 15,
                "stuck_days": 16,
                "oldest_pending_day": null,
                "errors": [],
                "degraded": false,
                "malformed_line_count": 0
            },
            "segment_fold_failed_days": []
        })
    );
}

#[test]
fn ac2_run_cli_writes_a_document() {
    let (temporary, system, apps) = setup();
    let reader = EmptyBacklog;
    let writer = RecordingWriter::default();

    let result = run_with(temporary.path(), &system, &apps, &[], &reader, &writer);

    assert_eq!(result.exit_code, 0);
    assert_eq!(value(writer.document())["day_count"], 1);
    assert!(
        result.stdout.starts_with("Wrote stats for 1 day(s) to "),
        "{}",
        result.stdout
    );
    assert!(result.stdout.contains("/stats.json\n"), "{}", result.stdout);
}

#[test]
fn ac3_unknown_argument_has_reference_usage_error() {
    let temporary = TempDir::new().unwrap();
    let reader = EmptyBacklog;
    let writer = RecordingWriter::default();
    let result = run_cli(
        &words(&["--nonsense"]),
        temporary.path(),
        now(),
        temporary.path(),
        temporary.path(),
        &reader,
        &writer,
    );

    assert_eq!(result.exit_code, 2);
    assert_eq!(result.stdout, "");
    assert_eq!(
        result.stderr,
        "usage: journal journal-stats [-h] [--no-cache] [-v] [-d]\n\
         journal journal-stats: error: unrecognized arguments: --nonsense\n"
    );
}

#[test]
fn ac4_token_miss_uses_filename_day_and_tolerates_bad_entries() {
    let (temporary, system, apps) = setup();
    let root = temporary.path();
    let tokens = root.join("tokens");
    fs::create_dir_all(&tokens).unwrap();
    // now() is 2026-01-06 12:00 UTC. A UTC re-bucket would file these under
    // 20260106. The filename is 20260105; filename wins.
    let timestamp = now().timestamp();
    fs::write(
        tokens.join("20260105.jsonl"),
        format!(
            "{{\"timestamp\":{timestamp},\"model\":\"model\",\"usage\":{{\"input\":3,\"skip\":\"x\",\"true_value\":true,\"false_value\":false,\"float_value\":1.0,\"string_value\":\"4\"}}}}\n\
             {{\"timestamp\":{timestamp},\"usage\":{{\"output\":2}}}}\n\
             {{\"timestamp\":0,\"model\":\"epoch\",\"usage\":{{\"input\":9}}}}\n\
             not-json\n"
        ),
    )
    .unwrap();
    let reader = EmptyBacklog;
    let writer = RecordingWriter::default();

    assert_eq!(
        run_with(root, &system, &apps, &[], &reader, &writer).exit_code,
        0
    );
    let document = value(writer.document());
    assert_eq!(document["tokens"]["by_day"][DAY]["model"]["input"], 3);
    assert!(document["tokens"]["by_day"].get("20260106").is_none());
    assert!(
        document["tokens"]["by_day"][DAY]["model"]
            .get("skip")
            .is_none()
    );
    assert_eq!(document["tokens"]["by_day"][DAY]["unknown"]["output"], 2);
    assert_eq!(document["tokens"]["by_model"]["model"]["input"], 3);
    assert_eq!(document["tokens"]["by_day"][DAY]["model"]["true_value"], 1);
    assert_eq!(document["tokens"]["by_day"][DAY]["model"]["false_value"], 0);
    assert!(
        document["tokens"]["by_day"][DAY]["model"]
            .get("float_value")
            .is_none()
    );
    assert!(
        document["tokens"]["by_day"][DAY]["model"]
            .get("string_value")
            .is_none()
    );
    assert!(document["tokens"]["by_day"].get("19700101").is_none());
}

#[test]
fn ac5_token_hit_assigns_file_stem_and_soft_fails_cache_io() {
    let (temporary, system, apps) = setup();
    let root = temporary.path();
    let tokens = root.join("tokens");
    fs::create_dir_all(&tokens).unwrap();
    let timestamp = now().timestamp() - 86_400;
    let source = tokens.join("20260105.jsonl");
    fs::write(
        &source,
        format!("{{\"timestamp\":{timestamp},\"model\":\"source\",\"usage\":{{\"input\":1}}}}\n"),
    )
    .unwrap();
    let cache = tokens.join("20260105.tokens_cache.json");
    fs::write(&cache, "{\"cached\":{\"input\":9}}").unwrap();
    let source_mtime = fs::metadata(&source).unwrap().modified().unwrap();
    set_modified(&cache, source_mtime + Duration::from_secs(2));

    let broken_source = tokens.join("20260104.jsonl");
    fs::write(
        &broken_source,
        format!(
            "{{\"timestamp\":{},\"model\":\"fallback\",\"usage\":{{\"input\":2}}}}\n",
            timestamp - 86_400
        ),
    )
    .unwrap();
    let broken_cache = tokens.join("20260104.tokens_cache.json");
    fs::create_dir(&broken_cache).unwrap();
    let broken_mtime = fs::metadata(&broken_source).unwrap().modified().unwrap();
    set_modified(&broken_cache, broken_mtime + Duration::from_secs(2));

    let reader = EmptyBacklog;
    let writer = RecordingWriter::default();
    let result = run_with(root, &system, &apps, &["--debug"], &reader, &writer);
    let document = value(writer.document());

    assert_eq!(result.exit_code, 0);
    assert_eq!(document["tokens"]["by_day"][DAY]["cached"]["input"], 9);
    assert!(document["tokens"]["by_day"][DAY].get("source").is_none());
    assert_eq!(
        document["tokens"]["by_day"]["20260104"]["fallback"]["input"],
        2
    );
    assert!(result.stderr.contains("Token cache load failed"));
    assert!(result.stderr.contains("Token cache save failed"));
}

#[test]
fn ac6_token_cache_and_cold_scan_agree_on_filename_day() {
    let (temporary, system, apps) = setup();
    let root = temporary.path();
    let tokens = root.join("tokens");
    fs::create_dir_all(&tokens).unwrap();
    // Timestamp UTC-buckets to 20260104. Filename is 20260105. Both runs
    // must file under the filename so a cache hit cannot move the day.
    fs::write(
        tokens.join("20260105.jsonl"),
        format!(
            "{{\"timestamp\":{},\"model\":\"model\",\"usage\":{{\"input\":4}}}}\n",
            now().timestamp() - 172_800
        ),
    )
    .unwrap();
    let reader = EmptyBacklog;
    let first_writer = RecordingWriter::default();
    let second_writer = RecordingWriter::default();

    assert_eq!(
        run_with(root, &system, &apps, &[], &reader, &first_writer).exit_code,
        0
    );
    let first = value(first_writer.document());
    assert_eq!(first["tokens"]["by_day"][DAY]["model"]["input"], 4);
    assert!(first["tokens"]["by_day"].get("20260104").is_none());
    assert_eq!(
        run_with(root, &system, &apps, &[], &reader, &second_writer).exit_code,
        0
    );
    let second = value(second_writer.document());
    assert_eq!(second["tokens"]["by_day"][DAY]["model"]["input"], 4);
    assert!(second["tokens"]["by_day"].get("20260104").is_none());
}

#[test]
fn ac7_no_cache_bypasses_day_and_token_caches() {
    let (temporary, system, apps) = setup();
    let root = temporary.path();
    let day_cache = root.join("chronicle").join(DAY).join("stats.json");
    let mut cached_scan = DayScan::default();
    cached_scan.stats.transcript_sessions = 99;
    fs::write(&day_cache, serde_json::to_string(&cached_scan).unwrap()).unwrap();

    let tokens = root.join("tokens");
    fs::create_dir_all(&tokens).unwrap();
    let timestamp = now().timestamp() - 86_400;
    let source = tokens.join("20260105.jsonl");
    fs::write(
        &source,
        format!("{{\"timestamp\":{timestamp},\"model\":\"model\",\"usage\":{{\"input\":1}}}}\n"),
    )
    .unwrap();
    let cache = tokens.join("20260105.tokens_cache.json");
    fs::write(&cache, "{\"model\":{\"input\":99}}").unwrap();
    let source_mtime = fs::metadata(&source).unwrap().modified().unwrap();
    set_modified(&cache, source_mtime + Duration::from_secs(2));

    let reader = EmptyBacklog;
    let cached_writer = RecordingWriter::default();
    let uncached_writer = RecordingWriter::default();
    assert_eq!(
        run_with(root, &system, &apps, &[], &reader, &cached_writer).exit_code,
        0
    );
    let day_cache_bytes = fs::read(&day_cache).unwrap();
    let day_cache_mtime = fs::metadata(&day_cache).unwrap().modified().unwrap();
    let token_cache_bytes = fs::read(&cache).unwrap();
    let token_cache_mtime = fs::metadata(&cache).unwrap().modified().unwrap();
    let uncached_day_cache = root.join("chronicle").join(OTHER_DAY).join("stats.json");
    fs::create_dir_all(uncached_day_cache.parent().unwrap()).unwrap();
    let uncached_token = tokens.join(format!("{OTHER_DAY}.jsonl"));
    fs::write(
        &uncached_token,
        format!(
            "{{\"timestamp\":{},\"model\":\"other\",\"usage\":{{\"input\":2}}}}\n",
            timestamp - 86_400
        ),
    )
    .unwrap();
    let uncached_token_cache = tokens.join(format!("{OTHER_DAY}.tokens_cache.json"));
    assert_eq!(
        run_with(
            root,
            &system,
            &apps,
            &["--no-cache"],
            &reader,
            &uncached_writer
        )
        .exit_code,
        0
    );
    let cached = value(cached_writer.document());
    let uncached = value(uncached_writer.document());
    assert_eq!(cached["days"][DAY]["transcript_sessions"], 99);
    assert_eq!(uncached["days"][DAY]["transcript_sessions"], 0);
    assert_eq!(cached["tokens"]["by_day"][DAY]["model"]["input"], 99);
    assert_eq!(uncached["tokens"]["by_day"][DAY]["model"]["input"], 1);
    assert_eq!(fs::read(&day_cache).unwrap(), day_cache_bytes);
    assert_eq!(
        fs::metadata(&day_cache).unwrap().modified().unwrap(),
        day_cache_mtime
    );
    assert_eq!(fs::read(&cache).unwrap(), token_cache_bytes);
    assert_eq!(
        fs::metadata(&cache).unwrap().modified().unwrap(),
        token_cache_mtime
    );
    assert!(!uncached_day_cache.exists());
    assert!(!uncached_token_cache.exists());
}

#[test]
fn rounded_minutes_match_python_half_even_behavior() {
    let mut scan = DayScan::default();
    for (name, minutes) in [
        ("exact_even", 1.125),
        ("exact_odd", 1.375),
        ("above_tie", 1.135),
        ("below_tie", 2.675),
        ("small", 0.005),
    ] {
        scan.agent_data
            .insert(name.to_owned(), ActivityTotals { count: 1, minutes });
        scan.facet_data
            .insert(name.to_owned(), ActivityTotals { count: 1, minutes });
    }
    let mut scans = std::collections::BTreeMap::new();
    scans.insert(DAY.to_owned(), scan);

    let document = value(assemble_document(
        &scans,
        TokenUsage::default(),
        empty_backlog(),
        now(),
    ));
    let expected = json!({
        "above_tie": 1.14,
        "below_tie": 2.67,
        "exact_even": 1.12,
        "exact_odd": 1.38,
        "small": 0.01,
    });

    assert_eq!(document["talents"]["minutes"], expected);
    assert_eq!(document["facets"]["minutes"], expected);
}

#[test]
fn day_cache_save_failure_is_debug_only() {
    let (temporary, system, apps) = setup();
    let root = temporary.path();
    fs::create_dir(root.join("chronicle").join(DAY).join("stats.json")).unwrap();
    let reader = EmptyBacklog;
    let normal_writer = RecordingWriter::default();
    let debug_writer = RecordingWriter::default();

    let normal = run_with(root, &system, &apps, &[], &reader, &normal_writer);
    let debug = run_with(root, &system, &apps, &["--debug"], &reader, &debug_writer);

    assert_eq!(normal.exit_code, 0);
    assert!(!normal.stderr.contains("Day cache save failed"));
    assert_eq!(debug.exit_code, 0);
    assert!(debug.stderr.contains("Day cache save failed for 20260105"));
}

#[test]
fn run_errors_identify_the_failed_operation() {
    let (temporary, system, apps) = setup();
    fs::create_dir_all(&system).unwrap();
    fs::write(system.join("invalid.md"), "{\n\"type\": generate\n}\n").unwrap();
    let reader = EmptyBacklog;
    let writer = RecordingWriter::default();

    let result = run_with(temporary.path(), &system, &apps, &[], &reader, &writer);

    assert_eq!(result.exit_code, 1);
    assert!(result.stderr.starts_with("Error scanning 20260105:"));
    assert!(!result.stderr.starts_with("Error writing stats.json:"));
}

#[test]
fn ac8_counts_by_day_requires_positive_count() {
    let mut scan = DayScan::default();
    scan.agent_data.insert(
        "zero".to_owned(),
        ActivityTotals {
            count: 0,
            minutes: 1.0,
        },
    );
    scan.facet_data.insert(
        "facet".to_owned(),
        ActivityTotals {
            count: 0,
            minutes: 2.0,
        },
    );
    let mut scans = std::collections::BTreeMap::new();
    scans.insert(DAY.to_owned(), scan);

    let document = value(assemble_document(
        &scans,
        crate::document::TokenUsage::default(),
        empty_backlog(),
        now(),
    ));

    assert_eq!(document["talents"]["counts"], json!({"zero": 0}));
    assert_eq!(document["talents"]["minutes"], json!({"zero": 1.0}));
    assert_eq!(document["talents"]["counts_by_day"], json!({}));
    assert_eq!(document["facets"]["counts_by_day"], json!({}));
}

#[test]
fn ac9_heatmap_accumulates_all_days() {
    let mut first = DayScan::default();
    first.heatmap_data.weekday = 0;
    first.heatmap_data.hours.insert(10, 1.25);
    let mut second = DayScan::default();
    second.heatmap_data.weekday = 0;
    second.heatmap_data.hours.insert(10, 0.75);
    second.heatmap_data.hours.insert(11, 2.0);
    let mut scans = std::collections::BTreeMap::new();
    scans.insert("20260105".to_owned(), first);
    scans.insert("20260112".to_owned(), second);

    let document = value(assemble_document(
        &scans,
        crate::document::TokenUsage::default(),
        empty_backlog(),
        now(),
    ));

    assert_eq!(document["heatmap"][0][10], 2.0);
    assert_eq!(document["heatmap"][0][11], 2.0);
    assert_eq!(document["heatmap"][1][10], 0.0);
}

#[test]
fn ac10_backlog_reader_error_produces_degraded_view() {
    let (temporary, system, apps) = setup();
    let writer = RecordingWriter::default();
    let result = run_with(
        temporary.path(),
        &system,
        &apps,
        &[],
        &FailingBacklog,
        &writer,
    );

    assert_eq!(result.exit_code, 0);
    let document = value(writer.document());
    assert_eq!(document["backlog"]["degraded"], true);
    assert_eq!(document["backlog"]["malformed_line_count"], 0);
    assert_eq!(document["backlog"]["days"], json!([]));
}

#[test]
fn ac11_document_writer_failure_preserves_existing_bytes() {
    let (temporary, system, apps) = setup();
    let root = temporary.path();
    let existing = root.join("stats.json");
    fs::write(&existing, b"existing document\n").unwrap();
    let writer = FailingWriter::default();
    let reader = EmptyBacklog;

    let result = run_with(root, &system, &apps, &[], &reader, &writer);

    assert_eq!(result.exit_code, 1);
    assert_eq!(
        result.stderr,
        "Error writing stats.json: stats validation failed: injected writer failure\n"
    );
    assert_eq!(writer.calls.get(), 1);
    assert_eq!(fs::read(&existing).unwrap(), b"existing document\n");
}

#[test]
fn ac12_duration_fold_and_generated_at_are_stable() {
    let scan = DayScan {
        stats: DayStats {
            transcript_duration: 1.25,
            percept_duration: 2.5,
            ..DayStats::default()
        },
        ..DayScan::default()
    };
    let mut scans = std::collections::BTreeMap::new();
    scans.insert(DAY.to_owned(), scan);
    let document = value(assemble_document(
        &scans,
        crate::document::TokenUsage::default(),
        empty_backlog(),
        now(),
    ));

    assert_eq!(document["generated_at"], "2026-01-06T12:00:00.000000+00:00");
    assert_eq!(document["totals"]["transcript_duration"], 1.25);
    assert_eq!(document["totals"]["percept_duration"], 2.5);
    assert_eq!(document["totals"]["total_transcript_duration"], 1.25);
    assert_eq!(document["totals"]["total_percept_duration"], 2.5);
    assert_eq!(document["totals"]["day_bytes"], 0);
}

#[test]
fn ac13_segment_fold_failure_signal_is_top_level() {
    let scan = DayScan {
        stats: DayStats {
            segment_fold_failed: true,
            ..DayStats::default()
        },
        ..DayScan::default()
    };
    let mut scans = std::collections::BTreeMap::new();
    scans.insert(DAY.to_owned(), scan);
    let document = value(assemble_document(
        &scans,
        crate::document::TokenUsage::default(),
        empty_backlog(),
        now(),
    ));

    assert_eq!(document["segment_fold_failed_days"], json!([DAY]));
    assert!(document["days"][DAY].get("segment_fold_failed").is_none());
}
