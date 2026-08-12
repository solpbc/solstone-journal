// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Local, NaiveDate, TimeZone};
use serde_json::json;
use solstone_core_system::schedule::{ScheduleNow, build_schedule_report};

#[derive(Debug, PartialEq, Eq)]
struct TreeEntry {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
    mode: u32,
}

struct Bed {
    root: PathBuf,
}

impl Bed {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-schedule-report-{name}-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config");
        fs::create_dir_all(root.join("health")).expect("health");
        Self { root }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config/schedules.json")
    }

    fn state(&self) -> PathBuf {
        self.root.join("health/scheduler.json")
    }

    fn write_config(&self, value: serde_json::Value) {
        fs::write(self.config(), serde_json::to_vec(&value).expect("json")).expect("config");
    }

    fn write_state(&self, value: serde_json::Value) {
        fs::write(self.state(), serde_json::to_vec(&value).expect("json")).expect("state");
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn now(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> ScheduleNow {
    let local = NaiveDate::from_ymd_opt(year, month, day)
        .expect("date")
        .and_hms_opt(hour, minute, 0)
        .expect("time");
    let unix_millis = Local
        .from_local_datetime(&local)
        .earliest()
        .expect("representable local time")
        .timestamp_millis();
    ScheduleNow { local, unix_millis }
}

fn report(bed: &Bed) -> solstone_core_system::schedule::ScheduleReport {
    build_schedule_report(bed.config(), bed.state(), now(2026, 3, 22, 10, 0))
}

fn snapshot_tree(root: &std::path::Path) -> Vec<TreeEntry> {
    fn walk(root: &std::path::Path, path: &std::path::Path, entries: &mut Vec<TreeEntry>) {
        let metadata = fs::metadata(path).expect("tree metadata");
        entries.push(TreeEntry {
            path: path
                .strip_prefix(root)
                .expect("relative path")
                .to_path_buf(),
            bytes: (!metadata.is_dir()).then(|| fs::read(path).expect("tree file")),
            mode: file_mode(&metadata),
        });
        if metadata.is_dir() {
            let mut children = fs::read_dir(path)
                .expect("tree directory")
                .map(|entry| entry.expect("tree entry").path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                walk(root, &child, entries);
            }
        }
    }

    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    entries
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn file_mode(_metadata: &fs::Metadata) -> u32 {
    0
}

fn read_only_report(bed: &Bed) -> solstone_core_system::schedule::ScheduleReport {
    let before = snapshot_tree(&bed.root);
    let report = report(bed);
    assert_eq!(
        snapshot_tree(&bed.root),
        before,
        "schedule report changed the journal tree"
    );
    report
}

#[test]
fn missing_config_is_a_quiet_empty_report() {
    let bed = Bed::new("missing-config");
    let report = read_only_report(&bed);
    assert_eq!(report.exit_code, 0);
    assert!(report.diagnostics.is_empty());
    assert!(report.rows.is_empty());
    assert!(report.render().contains("No schedules configured."));
}

#[test]
fn malformed_top_level_config_is_loud_and_has_no_rows() {
    let bed = Bed::new("bad-config");
    fs::write(bed.config(), b"{").expect("malformed config");
    let report = read_only_report(&bed);
    assert_eq!(report.exit_code, 1);
    assert_eq!(report.rows.len(), 0);
    assert_eq!(report.diagnostics.len(), 1);
}

#[test]
fn non_object_and_unreadable_config_are_loud_and_have_no_rows() {
    let non_object = Bed::new("config-shape");
    non_object.write_config(json!([]));
    let shape_report = read_only_report(&non_object);
    assert_eq!(shape_report.exit_code, 1);
    assert!(shape_report.rows.is_empty());
    assert_eq!(shape_report.diagnostics.len(), 1);

    let unreadable = Bed::new("config-unreadable");
    fs::create_dir(unreadable.config()).expect("directory in config file position");
    let report = read_only_report(&unreadable);
    assert_eq!(report.exit_code, 1);
    assert!(report.rows.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
}

#[test]
fn raw_entries_preserve_display_only_cases() {
    let bed = Bed::new("raw-entries");
    bed.write_config(json!({
        "daily_time": "03:00",
        "non-object": 1,
        "bad-cmd": {"cmd": ["journal", 1], "every": "daily"},
        "unknown": {"cmd": "journal heartbeat", "every": "yearly"},
        "disabled": {"cmd": ["journal", "noop"], "every": "daily", "enabled": false},
        "normal": {"cmd": "journal heartbeat", "every": "daily"}
    }));
    let report = read_only_report(&bed);
    assert_eq!(report.exit_code, 1);
    assert_eq!(report.diagnostics.len(), 2);
    assert!(report.rows.iter().all(|row| row.name != "non-object"));
    let bad = report
        .rows
        .iter()
        .find(|row| row.name == "bad-cmd")
        .expect("bad row");
    assert_eq!(bad.cmd, "<invalid>");
    assert_eq!(bad.next_due, "?");
    let unknown = report
        .rows
        .iter()
        .find(|row| row.name == "unknown")
        .expect("unknown row");
    assert_eq!(unknown.next_due, "?");
    let disabled = report
        .rows
        .iter()
        .find(|row| row.name == "disabled")
        .expect("disabled row");
    assert!(disabled.disabled);
    assert_eq!(disabled.next_due, "disabled");
}

#[test]
fn untrusted_state_is_loud_and_invalidates_computed_columns_only() {
    let bed = Bed::new("bad-state");
    bed.write_config(json!({
        "normal": {"cmd": ["journal", "heartbeat"], "every": "daily"},
        "unknown": {"cmd": ["journal", "heartbeat"], "every": "yearly"},
        "disabled": {"cmd": ["journal", "heartbeat"], "every": "daily", "enabled": false}
    }));
    fs::write(bed.state(), b"[").expect("malformed state");
    let report = read_only_report(&bed);
    assert_eq!(report.exit_code, 1);
    assert_eq!(report.diagnostics.len(), 1);
    let normal = report
        .rows
        .iter()
        .find(|row| row.name == "normal")
        .expect("normal row");
    assert_eq!(normal.last_run, "invalid");
    assert_eq!(normal.next_due, "invalid");
    let unknown = report
        .rows
        .iter()
        .find(|row| row.name == "unknown")
        .expect("unknown row");
    assert_eq!(unknown.next_due, "?");
    let disabled = report
        .rows
        .iter()
        .find(|row| row.name == "disabled")
        .expect("disabled row");
    assert_eq!(disabled.next_due, "disabled");
}

#[test]
fn non_object_and_unreadable_state_are_loud_and_invalid() {
    for name in ["state-shape", "state-unreadable"] {
        let bed = Bed::new(name);
        bed.write_config(json!({"normal": {"cmd": ["journal", "heartbeat"], "every": "daily"}}));
        if name == "state-shape" {
            bed.write_state(json!([]));
        } else {
            fs::create_dir(bed.state()).expect("directory in state file position");
        }
        let report = read_only_report(&bed);
        let row = report.rows.first().expect("normal row");
        assert_eq!(report.exit_code, 1);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(row.last_run, "invalid");
        assert_eq!(row.next_due, "invalid");
    }
}

#[test]
fn trusted_epoch_failures_are_soft_and_do_not_write_journal_state() {
    let bed = Bed::new("epochs");
    bed.write_config(json!({
        "missing": {"cmd": ["journal", "heartbeat"], "every": "hourly"},
        "text": {"cmd": ["journal", "heartbeat"], "every": "hourly"},
        "range": {"cmd": ["journal", "heartbeat"], "every": "hourly"}
    }));
    bed.write_state(json!({"text": {"last_run": "nope"}, "range": {"last_run": 1e100}}));
    let report = read_only_report(&bed);
    assert_eq!(report.exit_code, 0);
    assert!(report.diagnostics.is_empty());
    assert_eq!(
        report
            .rows
            .iter()
            .find(|row| row.name == "missing")
            .expect("missing")
            .last_run,
        "never"
    );
    assert_eq!(
        report
            .rows
            .iter()
            .find(|row| row.name == "text")
            .expect("text")
            .last_run,
        "invalid"
    );
    assert_eq!(
        report
            .rows
            .iter()
            .find(|row| row.name == "range")
            .expect("range")
            .last_run,
        "invalid"
    );
}
