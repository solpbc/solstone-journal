// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Ordered day-level pipeline health response for the native Health API.

use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Timelike, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use solstone_core_system_health::{
    FilesystemHealthLogSource, FilesystemSegmentSource, SegmentInput, TerminalEvent,
    classify_segment_completion, read_segment_progress, read_terminal_states, scan_day,
};

use super::report::HealthError;

const MODES: [&str; 6] = ["segment", "daily", "activity", "weekly", "flush", "cadence"];
const FAILED_LIST_CAP: usize = 20;
const ACTIVITY_WORK_EVENTS: [&str; 6] = [
    "run.start",
    "run.complete",
    "talent.dispatch",
    "talent.complete",
    "talent.fail",
    "talent.skip",
];

#[derive(Debug, Serialize)]
pub(crate) struct PipelineReport {
    day: String,
    generated_at: i64,
    status: String,
    anomalies: Vec<Value>,
    runs: PipelineRuns,
    talents: PipelineTalents,
    activities: PipelineActivities,
    exhausted_segments: ExhaustedSegments,
}

#[derive(Debug, Default, Serialize)]
struct PipelineRuns {
    segment: PipelineRun,
    daily: PipelineRun,
    activity: PipelineRun,
    weekly: PipelineRun,
    flush: PipelineRun,
    cadence: PipelineRun,
}

#[derive(Debug, Default, Serialize)]
struct PipelineRun {
    count: u64,
    duration_ms_total: i64,
}

#[derive(Debug, Default, Serialize)]
struct PipelineTalents {
    dispatched: u64,
    completed: u64,
    failed: u64,
    outstanding_failed: u64,
    skipped: u64,
    capped: u64,
    failed_list: Vec<Value>,
    failed_list_truncated: bool,
}

#[derive(Debug, Default, Serialize)]
struct PipelineActivities {
    detected: u64,
    persisted: u64,
    talents_fired: bool,
}
#[derive(Debug, Default, Serialize)]
struct ExhaustedSegments {
    count: u64,
    segments: Vec<String>,
}

impl PipelineReport {
    fn new(day: String, now: DateTime<Utc>) -> Self {
        Self {
            day,
            generated_at: now.timestamp_millis(),
            status: "healthy".to_owned(),
            anomalies: Vec::new(),
            runs: PipelineRuns::default(),
            talents: PipelineTalents::default(),
            activities: PipelineActivities::default(),
            exhausted_segments: ExhaustedSegments::default(),
        }
    }

    fn run_mut(&mut self, mode: &str) -> &mut PipelineRun {
        match mode {
            "segment" => &mut self.runs.segment,
            "daily" => &mut self.runs.daily,
            "activity" => &mut self.runs.activity,
            "weekly" => &mut self.runs.weekly,
            "flush" => &mut self.runs.flush,
            "cadence" => &mut self.runs.cadence,
            _ => unreachable!("mode is selected from MODES"),
        }
    }
}

pub(crate) fn resolve_pipeline_day(value: &str) -> Result<NaiveDate, HealthError> {
    if value.len() != 8 || !value.as_bytes().iter().all(u8::is_ascii_digit) {
        return Err(HealthError::InvalidRequest(
            "day must be a calendar date in YYYYMMDD format".to_owned(),
        ));
    }
    NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| {
        HealthError::InvalidRequest("day must be a calendar date in YYYYMMDD format".to_owned())
    })
}

pub(crate) fn summarize_pipeline_day(
    journal_root: &Path,
    date: NaiveDate,
    now: DateTime<Utc>,
) -> Result<PipelineReport, HealthError> {
    let day = date.format("%Y%m%d").to_string();
    let mut summary = PipelineReport::new(day.clone(), now);
    let health_dir = journal_root.join("chronicle").join(&day).join("health");
    if !health_dir.is_dir() {
        let segments = match completion_for_day(journal_root, &day, now) {
            Ok(segments) => segments,
            Err(error) => {
                log::warn!("native pipeline completion fold failed day={day}: {error:?}");
                summary.status = "unknown".to_owned();
                summary
                    .anomalies
                    .push(json!({"kind":"segments_not_thought","error":"no_health_dir"}));
                return Ok(summary);
            }
        };
        set_exhausted(&mut summary, &segments);
        if day < now.date_naive().format("%Y%m%d").to_string() && segments.total > 0 {
            summary.status = "unknown".to_owned();
            summary
                .anomalies
                .push(json!({"kind":"segments_not_thought","error":"no_health_dir"}));
        }
        return Ok(summary);
    }
    if let Err(error) = scan_health_logs(&health_dir, &day, &mut summary) {
        log::warn!("native pipeline health scan failed day={day}: {error}");
        summary.status = "unknown".to_owned();
        summary
            .anomalies
            .push(json!({"kind":"segments_not_thought","error":"scan_failed"}));
        return Ok(summary);
    }
    let health_source = FilesystemHealthLogSource::new(journal_root);
    let terminal = read_terminal_states(&health_source, &day, true)
        .map_err(|error| HealthError::internal(error.to_string()))?;
    let mut outstanding = terminal
        .value
        .into_iter()
        .filter(|(_, state)| state.latest_event == TerminalEvent::Fail)
        .map(|(unit, state)| json!({"mode":unit.mode,"name":unit.name,"use_id":state.use_id,"state":state.state}))
        .collect::<Vec<_>>();
    outstanding.sort_by_key(|value| {
        (
            value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            value
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            value
                .get("use_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        )
    });
    summary.talents.outstanding_failed = outstanding.len() as u64;
    summary.talents.failed_list = outstanding.iter().take(FAILED_LIST_CAP).cloned().collect();
    summary.talents.failed_list_truncated = outstanding.len() > FAILED_LIST_CAP;
    for failure in &summary.talents.failed_list {
        let mut anomaly = failure.as_object().cloned().unwrap_or_default();
        anomaly.insert(
            "kind".to_owned(),
            Value::String("talent_failure".to_owned()),
        );
        summary.anomalies.push(Value::Object(anomaly));
    }
    if summary.activities.detected > 0 && !summary.activities.talents_fired {
        summary
            .anomalies
            .push(json!({"kind":"activity_agents_missing"}));
    }
    let today = now.date_naive().format("%Y%m%d").to_string();
    if (day == today && now.hour() >= 23 || day < today) && summary.runs.daily.count == 0 {
        summary
            .anomalies
            .push(json!({"kind":"daily_agents_missing"}));
    }
    match completion_for_day(journal_root, &day, now) {
        Ok(completion) => {
            set_exhausted(&mut summary, &completion);
            if completion.not_thought > 0 {
                summary.anomalies.push(json!({"kind":"segments_not_thought","not_thought":completion.not_thought,"not_sensed":completion.not_sensed,"total":completion.total}));
            }
        }
        Err(error) => {
            log::warn!("native pipeline completion fold failed day={day}: {error:?}");
            summary
                .anomalies
                .push(json!({"kind":"segments_not_thought","error":"fold_failed"}));
        }
    }
    let stale = summary.anomalies.iter().any(|value| {
        matches!(
            value.get("kind").and_then(Value::as_str),
            Some("activity_agents_missing" | "daily_agents_missing" | "segments_not_thought")
        )
    });
    let failure = summary
        .anomalies
        .iter()
        .any(|value| value.get("kind").and_then(Value::as_str) == Some("talent_failure"));
    if stale {
        summary.status = "stale".to_owned();
    } else if failure {
        summary.status = "warning".to_owned();
    }
    Ok(summary)
}

fn scan_health_logs(
    directory: &Path,
    day: &str,
    summary: &mut PipelineReport,
) -> Result<(), std::io::Error> {
    let mut paths = fs::read_dir(directory)?
        .map(|entry| entry.map(|value| value.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    for path in paths {
        let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(mode) = MODES
            .iter()
            .find(|mode| filename.ends_with(&format!("_{mode}.jsonl")))
            .copied()
        else {
            continue;
        };
        summary.run_mut(mode).count += 1;
        let text = fs::read_to_string(path)?;
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(record) = record.as_object() else {
                continue;
            };
            if record
                .get("day")
                .and_then(Value::as_str)
                .is_some_and(|value| value != day)
            {
                continue;
            }
            let event = record
                .get("event")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if record.get("mode").and_then(Value::as_str) == Some("activity")
                && ACTIVITY_WORK_EVENTS.contains(&event)
            {
                summary.activities.talents_fired = true;
            }
            match event {
                "talent.dispatch" => summary.talents.dispatched += 1,
                "talent.complete" => summary.talents.completed += 1,
                "talent.fail" => summary.talents.failed += 1,
                "talent.skip" if record.get("reason").and_then(Value::as_str) == Some("capped") => {
                    summary.talents.capped += 1
                }
                "talent.skip" => summary.talents.skipped += 1,
                "activity.detected" => summary.activities.detected += 1,
                "activity.persisted" => summary.activities.persisted += 1,
                "run.complete" => {
                    summary.run_mut(mode).duration_ms_total +=
                        duration_ms(record.get("duration_ms"))
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn completion_for_day(
    journal_root: &Path,
    day: &str,
    now: DateTime<Utc>,
) -> Result<solstone_core_system_health::SegmentCompletion, HealthError> {
    let health = FilesystemHealthLogSource::new(journal_root);
    let progress = read_segment_progress(&health, day)
        .map_err(|error| HealthError::internal(error.to_string()))?;
    let (_, _, segments) = scan_day(&FilesystemSegmentSource, journal_root, day, now)
        .map_err(|error| HealthError::internal(error.to_string()))?;
    let inputs = segments
        .into_iter()
        .map(SegmentInput::from)
        .collect::<Vec<_>>();
    Ok(classify_segment_completion(&inputs, &progress.value))
}

fn set_exhausted(
    summary: &mut PipelineReport,
    completion: &solstone_core_system_health::SegmentCompletion,
) {
    summary.exhausted_segments = ExhaustedSegments {
        count: completion.exhausted.len() as u64,
        segments: completion.exhausted.clone(),
    };
}

fn duration_ms(value: Option<&Value>) -> i64 {
    value
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use chrono::{Duration, TimeZone, Utc};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{PipelineReport, summarize_pipeline_day};

    fn temporary() -> TempDir {
        TempDir::new_in("/var/tmp").unwrap()
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap()
    }

    fn write_log(root: &Path, day: &str, filename: &str, rows: &[Value]) {
        let path = root
            .join("chronicle")
            .join(day)
            .join("health")
            .join(filename);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "{}\n",
                rows.iter()
                    .map(|row| serde_json::to_string(row).unwrap())
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .unwrap();
    }

    fn screen_segment(root: &Path, day: &str, segment: &str) {
        let path = root.join("chronicle").join(day).join(segment);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("screen.jsonl"), "{}\n{\"timestamp\":0}\n").unwrap();
    }

    fn exhausted_screen_segment(root: &Path, day: &str, segment: &str) {
        let path = root.join("chronicle").join(day).join(segment);
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("screen.jsonl"),
            "{\"_solstone_processing\":{\"state\":\"failed\",\"attempts\":3}}\n",
        )
        .unwrap();
    }

    fn value(report: &PipelineReport) -> Value {
        serde_json::to_value(report).unwrap()
    }

    #[test]
    fn serde_preserves_pipeline_root_order_and_is_compact() {
        let report = PipelineReport::new(
            "20260401".to_owned(),
            Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap(),
        );
        assert_eq!(
            serde_json::to_string(&report).unwrap(),
            "{\"day\":\"20260401\",\"generated_at\":1775001600000,\"status\":\"healthy\",\"anomalies\":[],\"runs\":{\"segment\":{\"count\":0,\"duration_ms_total\":0},\"daily\":{\"count\":0,\"duration_ms_total\":0},\"activity\":{\"count\":0,\"duration_ms_total\":0},\"weekly\":{\"count\":0,\"duration_ms_total\":0},\"flush\":{\"count\":0,\"duration_ms_total\":0},\"cadence\":{\"count\":0,\"duration_ms_total\":0}},\"talents\":{\"dispatched\":0,\"completed\":0,\"failed\":0,\"outstanding_failed\":0,\"skipped\":0,\"capped\":0,\"failed_list\":[],\"failed_list_truncated\":false},\"activities\":{\"detected\":0,\"persisted\":0,\"talents_fired\":false},\"exhausted_segments\":{\"count\":0,\"segments\":[]}}"
        );
    }

    #[test]
    fn summarize_pipeline_day_folds_real_mode_talent_and_activity_logs() {
        let temporary = temporary();
        let root = temporary.path();
        let day = "20260410";
        for (mode, duration) in [
            ("segment", 10),
            ("daily", 20),
            ("activity", 30),
            ("weekly", 40),
            ("flush", 50),
            ("cadence", 60),
        ] {
            let mut rows =
                vec![json!({"event":"run.complete","day":day,"mode":mode,"duration_ms":duration})];
            if mode == "activity" {
                rows.extend([
                    json!({"event":"talent.dispatch","day":day,"mode":"activity","name":"schedule"}),
                    json!({"event":"activity.detected","day":day,"mode":"activity"}),
                    json!({"event":"activity.persisted","day":day,"mode":"activity"}),
                ]);
            }
            if mode == "segment" {
                rows.extend([
                    json!({"event":"talent.complete","day":day,"mode":"segment","name":"documents"}),
                    json!({"event":"talent.skip","day":day,"mode":"segment","name":"optional","reason":"capped"}),
                    json!({"event":"talent.skip","day":day,"mode":"segment","name":"later","reason":"queue"}),
                ]);
            }
            write_log(root, day, &format!("001_{mode}.jsonl"), &rows);
        }
        let report = value(&summarize_pipeline_day(root, now().date_naive(), now()).unwrap());
        assert_eq!(report["status"], "healthy");
        for (mode, duration) in [
            ("segment", 10),
            ("daily", 20),
            ("activity", 30),
            ("weekly", 40),
            ("flush", 50),
            ("cadence", 60),
        ] {
            assert_eq!(report["runs"][mode]["count"], 1);
            assert_eq!(report["runs"][mode]["duration_ms_total"], duration);
        }
        assert_eq!(
            report["talents"],
            json!({"dispatched":1,"completed":1,"failed":0,"outstanding_failed":0,"skipped":1,"capped":1,"failed_list":[],"failed_list_truncated":false})
        );
        assert_eq!(
            report["activities"],
            json!({"detected":1,"persisted":1,"talents_fired":true})
        );
        assert_eq!(
            report["exhausted_segments"],
            json!({"count":0,"segments":[]})
        );
    }

    #[test]
    fn outstanding_failures_are_sorted_and_capped_at_twenty() {
        let temporary = temporary();
        let day = "20260410";
        let rows = (0..21)
            .map(|index| json!({"event":"talent.fail","day":day,"mode":"daily","name":format!("talent-{index:02}"),"use_id":format!("use-{index:02}"),"ts":index}))
            .collect::<Vec<_>>();
        write_log(temporary.path(), day, "001_daily.jsonl", &rows);
        let report =
            value(&summarize_pipeline_day(temporary.path(), now().date_naive(), now()).unwrap());
        assert_eq!(report["status"], "warning");
        assert_eq!(report["talents"]["failed"], 21);
        assert_eq!(report["talents"]["outstanding_failed"], 21);
        assert_eq!(
            report["talents"]["failed_list"].as_array().unwrap().len(),
            20
        );
        assert_eq!(report["talents"]["failed_list_truncated"], true);
        assert_eq!(report["talents"]["failed_list"][0]["name"], "talent-00");
        assert_eq!(report["talents"]["failed_list"][19]["name"], "talent-19");
        assert_eq!(
            report["anomalies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|entry| entry["kind"] == "talent_failure")
                .count(),
            20
        );
    }

    #[test]
    fn past_day_with_segments_and_no_health_directory_is_unknown_and_returns_early() {
        let temporary = temporary();
        let day = (now().date_naive() - Duration::days(1))
            .format("%Y%m%d")
            .to_string();
        screen_segment(temporary.path(), &day, "120000_60");
        let report = value(
            &summarize_pipeline_day(
                temporary.path(),
                now().date_naive() - Duration::days(1),
                now(),
            )
            .unwrap(),
        );
        assert_eq!(report["status"], "unknown");
        assert_eq!(
            report["anomalies"],
            json!([{"kind":"segments_not_thought","error":"no_health_dir"}])
        );
        assert_eq!(report["runs"]["daily"]["count"], 0);
    }

    #[test]
    fn no_health_directory_completion_failure_degrades_to_unknown() {
        let temporary = temporary();
        let date = now().date_naive() - Duration::days(1);
        let day = date.format("%Y%m%d").to_string();
        let blocked = temporary
            .path()
            .join("chronicle")
            .join(&day)
            .join("blocked");
        fs::create_dir_all(&blocked).unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let report = value(&summarize_pipeline_day(temporary.path(), date, now()).unwrap());
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(report["status"], "unknown");
        assert_eq!(
            report["anomalies"],
            json!([{"kind":"segments_not_thought","error":"no_health_dir"}])
        );
    }

    #[test]
    fn unreadable_health_log_degrades_to_unknown_scan_failed() {
        let temporary = temporary();
        let path = temporary
            .path()
            .join("chronicle/20260410/health/001_daily.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, [0xff]).unwrap();
        let report =
            value(&summarize_pipeline_day(temporary.path(), now().date_naive(), now()).unwrap());
        assert_eq!(report["status"], "unknown");
        assert_eq!(
            report["anomalies"],
            json!([{"kind":"segments_not_thought","error":"scan_failed"}])
        );
    }

    #[test]
    fn matching_health_log_directory_degrades_to_unknown_scan_failed() {
        let temporary = temporary();
        fs::create_dir_all(
            temporary
                .path()
                .join("chronicle/20260410/health/001_daily.jsonl"),
        )
        .unwrap();
        let report =
            value(&summarize_pipeline_day(temporary.path(), now().date_naive(), now()).unwrap());
        assert_eq!(report["status"], "unknown");
        assert_eq!(
            report["anomalies"],
            json!([{"kind":"segments_not_thought","error":"scan_failed"}])
        );
    }

    #[test]
    fn activity_work_without_activity_talent_degrades_to_stale() {
        let temporary = temporary();
        let day = "20260410";
        write_log(
            temporary.path(),
            day,
            "001_daily.jsonl",
            &[json!({"event":"run.complete","day":day,"mode":"daily"})],
        );
        write_log(
            temporary.path(),
            day,
            "001_activity.jsonl",
            &[json!({"event":"activity.detected","day":day,"mode":"activity"})],
        );
        let report =
            value(&summarize_pipeline_day(temporary.path(), now().date_naive(), now()).unwrap());
        assert_eq!(report["status"], "stale");
        assert!(
            report["anomalies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["kind"] == "activity_agents_missing")
        );
    }

    #[test]
    fn activity_run_complete_counts_as_activity_work() {
        let temporary = temporary();
        let day = "20260410";
        write_log(
            temporary.path(),
            day,
            "001_daily.jsonl",
            &[json!({"event":"run.complete","day":day,"mode":"daily"})],
        );
        write_log(
            temporary.path(),
            day,
            "001_activity.jsonl",
            &[
                json!({"event":"activity.detected","day":day,"mode":"activity"}),
                json!({"event":"run.complete","day":day,"mode":"activity"}),
            ],
        );
        let report =
            value(&summarize_pipeline_day(temporary.path(), now().date_naive(), now()).unwrap());
        assert_eq!(report["status"], "healthy");
        assert_eq!(report["activities"]["talents_fired"], true);
        assert!(
            !report["anomalies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["kind"] == "activity_agents_missing")
        );
    }

    #[test]
    fn past_health_day_without_daily_run_degrades_to_stale() {
        let temporary = temporary();
        let date = now().date_naive() - Duration::days(1);
        let day = date.format("%Y%m%d").to_string();
        write_log(
            temporary.path(),
            &day,
            "001_segment.jsonl",
            &[json!({"event":"run.complete","day":day,"mode":"segment"})],
        );
        let report = value(&summarize_pipeline_day(temporary.path(), date, now()).unwrap());
        assert_eq!(report["status"], "stale");
        assert!(
            report["anomalies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["kind"] == "daily_agents_missing")
        );
    }

    #[test]
    fn exhausted_segment_markers_are_reported_with_their_exact_keys() {
        let temporary = temporary();
        let day = "20260410";
        exhausted_screen_segment(temporary.path(), day, "120000_60");
        write_log(
            temporary.path(),
            day,
            "001_daily.jsonl",
            &[
                json!({"event":"run.complete","day":day,"mode":"daily"}),
                json!({"event":"sense.complete","ts":1,"mode":"segment","stream":"_default","segment":"120000_60","density":"idle"}),
            ],
        );
        let report =
            value(&summarize_pipeline_day(temporary.path(), now().date_naive(), now()).unwrap());
        assert_eq!(report["status"], "healthy");
        assert_eq!(
            report["exhausted_segments"],
            json!({"count":1,"segments":["120000_60"]})
        );
    }
}
