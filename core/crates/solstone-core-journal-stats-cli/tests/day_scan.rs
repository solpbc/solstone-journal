// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use solstone_core_system_health::{
    FilesystemHealthLogSource, FilesystemSegmentSource, HealthError, SegmentSource,
};
use tempfile::TempDir;

use crate::{
    CacheStatus, DayCacheWriter, DayScan, DayScanRequest, FilesystemDayCacheWriter,
    JournalStatsError,
    cache::{load_fresh_day_cache, save_day_cache},
};

const DAY: &str = "20260105";

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 6, 12, 0, 0).unwrap()
}

fn day_path(root: &Path) -> PathBuf {
    root.join("chronicle").join(DAY)
}

fn segment(root: &Path, stream: Option<&str>, key: &str) -> PathBuf {
    let path = match stream {
        Some(stream) => day_path(root).join(stream).join(key),
        None => day_path(root).join(key),
    };
    fs::create_dir_all(&path).unwrap();
    path
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn talent(path: impl AsRef<Path>, metadata: Value) {
    write(
        path,
        &format!(
            "{}\nbody\n",
            serde_json::to_string_pretty(&metadata).unwrap()
        ),
    );
}

fn talent_roots(root: &Path) -> (PathBuf, PathBuf) {
    (root.join("system-talents"), root.join("apps"))
}

fn scan_filesystem(root: &Path, system: &Path, apps: &Path) -> crate::ScanDayOutcome {
    let segments = FilesystemSegmentSource;
    let health = FilesystemHealthLogSource::new(root);
    let writer = FilesystemDayCacheWriter;
    crate::scan_day(DayScanRequest {
        journal_root: root,
        day: DAY,
        now: now(),
        system_talent_root: system,
        apps_root: apps,
        segment_source: &segments,
        health_source: &health,
        cache_writer: &writer,
    })
    .unwrap()
}

fn tree_bytes(path: &Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| {
            if entry.file_type().unwrap().is_dir() {
                tree_bytes(&entry.path())
            } else {
                entry.metadata().unwrap().len()
            }
        })
        .sum()
}

fn set_modified(path: &Path, modified: SystemTime) {
    fs::File::open(path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

fn cache_payload() -> DayScan {
    DayScan::default()
}

fn write_cache(path: &Path, payload: &DayScan) {
    write(path, &serde_json::to_string(payload).unwrap());
}

#[test]
fn ac1_comprehensive_day_tree_populates_every_day_field() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = day_path(root);
    let (system, apps) = talent_roots(root);

    talent(
        system.join("daily_present.md"),
        json!({"type":"generate","schedule":"daily","priority":1,"output":"md"}),
    );
    talent(
        system.join("daily_missing.md"),
        json!({"type":"generate","schedule":"daily","priority":1,"output":"json"}),
    );
    talent(
        system.join("daily_disabled.md"),
        json!({"type":"generate","schedule":"daily","priority":1,"output":"json","disabled":true}),
    );
    talent(
        system.join("segment_only.md"),
        json!({"type":"generate","schedule":"segment","priority":1,"output":"md"}),
    );
    talent(
        apps.join("example/talent/app_daily.md"),
        json!({"type":"generate","schedule":"daily","priority":1,"output":"md"}),
    );

    let first = segment(root, Some("stream"), "143000_120");
    write(
        first.join("audio.jsonl"),
        "{\"header\":true}\n{\"start\":\"14:30:00\"}\n{\"start\":\"14:31:00\"}\n",
    );
    write(
        first.join("screen.jsonl"),
        "{\"header\":true}\n{\"frame_id\":1,\"timestamp\":0}\n{\"frame_id\":2,\"timestamp\":90}\n",
    );
    write(first.join("browser_page.jsonl"), "browser row\n");
    write(first.join("clip.flac"), "raw");
    write(first.join("clip.jsonl"), "processed\n");
    write(first.join("wrong.m4a"), "raw");
    write(first.join("wrong.m4a.jsonl"), "wrong sibling\n");

    let second = segment(root, Some("stream"), "150000_60");
    write(
        second.join("foo_audio.jsonl"),
        "{\"header\":true}\n{\"start\":\"15:00:00\"}\n",
    );
    write(
        second.join("bar_transcript.jsonl"),
        "{\"header\":true}\n{\"start\":\"15:02:00\"}\n",
    );
    write(second.join("foo_screen.jsonl"), "{\"header\":true}\n");

    let direct = segment(root, None, "160000_60");
    write(direct.join("browser_page.jsonl"), "browser row\n");
    write(day.join("talents/daily_present.md"), "done\n");
    write(day.join("talents/daily_disabled.json"), "{}\n");
    write(day.join("talents/_example_app_daily.md"), "done\n");
    write(day.join("talents/facet/nested.json"), "{}\n");
    let expected_bytes = tree_bytes(&day);

    let outcome = scan_filesystem(root, &system, &apps);
    let stats = &outcome.scan.stats;
    assert_eq!(outcome.cache_status, CacheStatus::Saved);
    assert_eq!(stats.transcript_sessions, 3);
    assert_eq!(stats.transcript_segments, 4);
    assert_eq!(stats.transcript_duration, 60.0);
    assert_eq!(stats.transcript_ranges, 2);
    assert_eq!(stats.percept_sessions, 2);
    assert_eq!(stats.percept_frames, 2);
    assert_eq!(stats.percept_duration, 90.0);
    assert_eq!(stats.percept_ranges, 2);
    assert_eq!(stats.browser_segments, 2);
    assert_eq!(stats.pending_segments, 1);
    assert_eq!(stats.segments_pending_think, 1);
    assert_eq!(stats.outputs_processed, 3);
    assert_eq!(stats.outputs_pending, 1);
    assert_eq!(stats.day_bytes, expected_bytes);
    assert!(!stats.segment_fold_failed);
}

#[test]
fn ac2_sidecar_failures_are_tolerant_after_session_counting() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let (system, apps) = talent_roots(root);
    let first = segment(root, Some("stream"), "120000_60");
    write(
        first.join("one_audio.jsonl"),
        "header\nnot-json\n{\"start\":\"not-a-time\"}\n",
    );
    let second = segment(root, Some("stream"), "121000_60");
    write(
        second.join("two_audio.jsonl"),
        "header\n{\"start\":\"12:10:00\"}\n{\"start\":\"12:10:30\"}\n",
    );
    let unreadable = segment(root, Some("stream"), "122000_60").join("bad_transcript.jsonl");
    fs::create_dir_all(unreadable).unwrap();

    let screen = segment(root, Some("stream"), "123000_60");
    write(
        screen.join("screen.jsonl"),
        "{\"header\":true}\n{\"frame_id\":1,\"timestamp\":0}\n{\"frame_id\":2,\"timestamp\":30,\"error\":\"decode failed\"}\n",
    );

    let outcome = scan_filesystem(root, &system, &apps);
    assert_eq!(outcome.scan.stats.transcript_sessions, 3);
    assert_eq!(outcome.scan.stats.transcript_segments, 3);
    assert_eq!(outcome.scan.stats.transcript_duration, 30.0);
    assert_eq!(outcome.scan.stats.percept_frames, 1);
    assert_eq!(outcome.scan.stats.percept_duration, 0.0);
}

#[test]
fn ac3_pending_media_requires_the_replaced_jsonl_sibling() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let (system, apps) = talent_roots(root);
    let complete = segment(root, Some("stream"), "120000_60");
    write(complete.join("clip.flac"), "raw");
    write(complete.join("clip.jsonl"), "processed\n");
    let pending = segment(root, Some("stream"), "121000_60");
    write(pending.join("clip.flac"), "raw");
    write(
        pending.join("clip.flac.jsonl"),
        "not the replaced sibling\n",
    );

    assert_eq!(
        scan_filesystem(root, &system, &apps)
            .scan
            .stats
            .pending_segments,
        1
    );
}

#[test]
fn ac4_daily_talent_outputs_include_disabled_and_exclude_other_schedules() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let day = day_path(root);
    let (system, apps) = talent_roots(root);
    talent(
        system.join("enabled_present.md"),
        json!({"type":"generate","schedule":"daily","priority":1,"output":"md"}),
    );
    talent(
        system.join("enabled_missing.md"),
        json!({"type":"generate","schedule":"daily","priority":1,"output":"json"}),
    );
    talent(
        system.join("disabled.md"),
        json!({"type":"generate","schedule":"daily","priority":1,"output":"md","disabled":true}),
    );
    talent(
        system.join("segment.md"),
        json!({"type":"generate","schedule":"segment","priority":1,"output":"md"}),
    );
    talent(
        system.join("non_generate.md"),
        json!({"type":"cogitate","cwd":"journal"}),
    );
    write(day.join("talents/enabled_present.md"), "done\n");
    write(day.join("talents/disabled.md"), "done\n");

    let outcome = scan_filesystem(root, &system, &apps);
    assert_eq!(outcome.scan.stats.outputs_processed, 2);
    assert_eq!(outcome.scan.stats.outputs_pending, 1);
}

struct FailingSegmentSource;

impl SegmentSource for FailingSegmentSource {
    fn segments(
        &self,
        _journal: &Path,
        _day: &str,
    ) -> Result<Vec<solstone_core_journal_io::Segment>, HealthError> {
        Err(HealthError::InvalidDay(
            "forced segment source failure".to_owned(),
        ))
    }
}

#[test]
fn ac5_segment_fold_failure_zeros_only_the_fold_fields() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let (system, apps) = talent_roots(root);
    let sidecar = segment(root, Some("stream"), "120000_60");
    write(
        sidecar.join("audio.jsonl"),
        "header\n{\"start\":\"12:00:00\"}\n{\"start\":\"12:01:00\"}\n",
    );
    let health = FilesystemHealthLogSource::new(root);
    let writer = FilesystemDayCacheWriter;
    let outcome = crate::scan_day(DayScanRequest {
        journal_root: root,
        day: DAY,
        now: now(),
        system_talent_root: &system,
        apps_root: &apps,
        segment_source: &FailingSegmentSource,
        health_source: &health,
        cache_writer: &writer,
    })
    .unwrap();
    assert_eq!(outcome.scan.stats.transcript_sessions, 1);
    assert_eq!(outcome.scan.stats.transcript_segments, 2);
    assert_eq!(outcome.scan.stats.transcript_duration, 60.0);
    assert_eq!(outcome.scan.stats.transcript_ranges, 0);
    assert_eq!(outcome.scan.stats.percept_ranges, 0);
    assert_eq!(outcome.scan.stats.browser_segments, 0);
    assert_eq!(outcome.scan.stats.segments_pending_think, 0);
    assert!(outcome.scan.stats.segment_fold_failed);
}

#[test]
fn ac6_activities_accumulate_across_facets_and_split_the_heatmap() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let (system, apps) = talent_roots(root);
    fs::create_dir_all(day_path(root)).unwrap();
    write(
        root.join(format!("facets/one/activities/{DAY}.jsonl")),
        "{\"activity\":\"meeting\",\"segments\":[\"105900_120\"]}\n{\"activity\":\"skip\",\"segments\":[]}\n{\"activity\":\"fallback\",\"segments\":[\"bad\"]}\n",
    );
    write(
        root.join(format!("facets/two/activities/{DAY}.jsonl")),
        "{\"activity\":\"meeting\",\"segments\":[\"105900_120\"]}\n{\"activity\":\"late\",\"segments\":[\"235900_120\"]}\n",
    );

    let outcome = scan_filesystem(root, &system, &apps);
    let meeting = &outcome.scan.agent_data["meeting"];
    assert_eq!(meeting.count, 2);
    assert_eq!(meeting.minutes, 4.0);
    assert_eq!(outcome.scan.facet_data["one"].count, 2);
    assert_eq!(outcome.scan.facet_data["one"].minutes, 3.0);
    assert_eq!(outcome.scan.facet_data["two"].count, 2);
    assert_eq!(outcome.scan.facet_data["two"].minutes, 3.0);
    assert_eq!(outcome.scan.agent_data["fallback"].minutes, 1.0);
    assert_eq!(outcome.scan.heatmap_data.weekday, 0);
    assert_eq!(outcome.scan.heatmap_data.hours[&10], 2.0);
    assert_eq!(outcome.scan.heatmap_data.hours[&11], 2.0);
    assert!((outcome.scan.heatmap_data.hours[&23] - (59.0 / 60.0)).abs() < f64::EPSILON);
    assert!(!outcome.scan.heatmap_data.hours.contains_key(&0));
}

#[test]
fn ac7_freshness_requires_new_schema_complete_stats_and_strict_mtime() {
    let temporary = TempDir::new().unwrap();
    let day = day_path(temporary.path());
    fs::create_dir_all(&day).unwrap();
    let cache = day.join("stats.json");
    assert!(load_fresh_day_cache(&day).unwrap().is_none());

    write(&cache, "not json");
    assert!(load_fresh_day_cache(&day).unwrap().is_none());
    write(&cache, &json!({"schema_version":7,"stats":{}}).to_string());
    assert!(load_fresh_day_cache(&day).unwrap().is_none());
    let mut missing_field = serde_json::to_value(cache_payload()).unwrap();
    missing_field["stats"]
        .as_object_mut()
        .unwrap()
        .remove("day_bytes");
    write(&cache, &missing_field.to_string());
    assert!(load_fresh_day_cache(&day).unwrap().is_none());

    let input = day.join("talents/input.json");
    write(&input, "{}\n");
    write_cache(&cache, &cache_payload());
    let input_mtime = fs::metadata(&input).unwrap().modified().unwrap();
    set_modified(&cache, input_mtime);
    assert!(load_fresh_day_cache(&day).unwrap().is_none());
    set_modified(&cache, input_mtime - Duration::from_secs(1));
    assert!(load_fresh_day_cache(&day).unwrap().is_none());
    set_modified(&cache, input_mtime + Duration::from_secs(2));
    assert!(load_fresh_day_cache(&day).unwrap().is_some());

    let mut no_fold_marker = serde_json::to_value(cache_payload()).unwrap();
    no_fold_marker["stats"]
        .as_object_mut()
        .unwrap()
        .remove("segment_fold_failed");
    write(&cache, &no_fold_marker.to_string());
    set_modified(&cache, input_mtime + Duration::from_secs(3));
    assert!(load_fresh_day_cache(&day).unwrap().is_some());
}

#[test]
fn ac8_freshness_reads_never_write_or_create_a_cache() {
    let temporary = TempDir::new().unwrap();
    let day = day_path(temporary.path());
    fs::create_dir_all(&day).unwrap();
    assert!(load_fresh_day_cache(&day).unwrap().is_none());
    let cache = day.join("stats.json");
    assert!(!cache.exists());

    write_cache(&cache, &cache_payload());
    let before_bytes = fs::read(&cache).unwrap();
    let before_mtime = fs::metadata(&cache).unwrap().modified().unwrap();
    assert!(load_fresh_day_cache(&day).unwrap().is_some());
    assert_eq!(fs::read(&cache).unwrap(), before_bytes);
    assert_eq!(
        fs::metadata(&cache).unwrap().modified().unwrap(),
        before_mtime
    );
}

#[test]
fn ac9_only_bounded_inputs_invalidate_and_a_saved_cache_stays_fresh() {
    let temporary = TempDir::new().unwrap();
    let day = day_path(temporary.path());
    fs::create_dir_all(&day).unwrap();
    let cache = day.join("stats.json");
    write_cache(&cache, &cache_payload());
    let cache_time = fs::metadata(&cache).unwrap().modified().unwrap();
    let later = cache_time + Duration::from_secs(2);

    write(day.join("talents/x.json"), "{}\n");
    set_modified(&day.join("talents/x.json"), later);
    assert!(load_fresh_day_cache(&day).unwrap().is_none());

    write_cache(&cache, &cache_payload());
    set_modified(&cache, later + Duration::from_secs(2));
    write(day.join("notes.txt"), "new but unbounded\n");
    set_modified(&day.join("notes.txt"), later + Duration::from_secs(3));
    assert!(load_fresh_day_cache(&day).unwrap().is_some());

    let before_final_save = SystemTime::now();
    set_modified(
        &day.join("talents/x.json"),
        before_final_save - Duration::from_secs(5),
    );
    save_day_cache(&FilesystemDayCacheWriter, &cache, &cache_payload()).unwrap();
    assert!(load_fresh_day_cache(&day).unwrap().is_some());
}

struct FailingWriter;

impl DayCacheWriter for FailingWriter {
    fn write_day_cache(&self, path: &Path, _payload: &DayScan) -> Result<(), JournalStatsError> {
        Err(JournalStatsError::TalentConfig {
            path: path.to_path_buf(),
            message: "forced writer failure".to_owned(),
        })
    }
}

#[test]
fn ac10_cache_write_failure_keeps_the_computed_result() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    let (system, apps) = talent_roots(root);
    let source = segment(root, Some("stream"), "120000_60");
    write(
        source.join("audio.jsonl"),
        "header\n{\"start\":\"12:00:00\"}\n{\"start\":\"12:01:00\"}\n",
    );
    let segments = FilesystemSegmentSource;
    let health = FilesystemHealthLogSource::new(root);
    let outcome = crate::scan_day(DayScanRequest {
        journal_root: root,
        day: DAY,
        now: now(),
        system_talent_root: &system,
        apps_root: &apps,
        segment_source: &segments,
        health_source: &health,
        cache_writer: &FailingWriter,
    })
    .unwrap();
    assert!(matches!(
        outcome.cache_status,
        CacheStatus::SaveFailed { .. }
    ));
    assert_eq!(outcome.scan.stats.transcript_sessions, 1);
    assert_eq!(outcome.scan.stats.transcript_segments, 2);
    assert_eq!(outcome.scan.stats.transcript_duration, 60.0);
    assert!(!day_path(root).join("stats.json").exists());
}
