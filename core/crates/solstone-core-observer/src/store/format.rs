// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::path::Path;

use chrono::{Local, TimeZone};
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};

use super::history::{HistoryStop, load_history};
use super::paths::{history_dir, history_path};
use super::record::ObserverRecord;

const CONNECTED_THRESHOLD_MS: i64 = 2 * 60 * 1000;

/// Display zone for human-formatted observer clocks.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TimeDisplay {
    Local,
    Utc,
}

pub fn status_label(record: &ObserverRecord, now_ms: i64) -> &'static str {
    if record.revoked() {
        return "revoked";
    }
    match record.last_seen() {
        Some(last_seen) if now_ms - last_seen < CONNECTED_THRESHOLD_MS => "connected",
        _ => "disconnected",
    }
}

pub fn fmt_bytes(value: f64) -> String {
    if value < 1024.0 {
        format_number(value, " B")
    } else if value < 1024.0 * 1024.0 {
        format!("{:.1} KB", value / 1024.0)
    } else if value < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", value / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", value / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_number(value: f64, suffix: &str) -> String {
    if value.fract() == 0.0 {
        format!("{}{suffix}", value as i64)
    } else {
        format!("{value}{suffix}")
    }
}

pub fn fmt_time(value: Option<i64>, zone: TimeDisplay) -> String {
    value
        .and_then(|value| display_time(value, zone))
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "never".to_owned())
}

fn display_time(value: i64, zone: TimeDisplay) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    match zone {
        TimeDisplay::Utc => chrono::Utc
            .timestamp_millis_opt(value)
            .single()
            .map(|time| time.fixed_offset()),
        TimeDisplay::Local => Local
            .timestamp_millis_opt(value)
            .single()
            .map(|time| time.fixed_offset()),
    }
}

pub fn fmt_compact_age(value: &Value, now_ms: i64) -> String {
    let Some(value) = value.as_i64() else {
        return "—".to_owned();
    };
    if value < 0 || now_ms < value {
        return "—".to_owned();
    }
    let seconds = (now_ms - value) / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}

pub fn render_list(
    records: &[ObserverRecord],
    json_output: bool,
    now_ms: i64,
    zone: TimeDisplay,
) -> String {
    if json_output {
        return serde_json::to_string(&Value::Array(
            records
                .iter()
                .map(|record| list_entry(record, now_ms))
                .collect(),
        ))
        .expect("JSON values serialize");
    }
    if records.is_empty() {
        return "No devices registered.".to_owned();
    }
    let mut lines = vec![
        format!(
            "{:<20} {:<18} {:<14} {:<10} {:<18} {:<12} {:>10} {:>12}",
            "Name", "Prefix", "Status", "Binding", "Last Seen", "Last Segment", "Segments", "Bytes"
        ),
        "-".repeat(118),
    ];
    for record in records {
        let stats = record.stats();
        lines.push(format!(
            "{:<20} {:<18} {:<14} {:<10} {:<18} {:<12} {:>10} {:>12}",
            record.name().unwrap_or_default(),
            record.prefix(),
            status_label(record, now_ms),
            record.device_binding_kind().unwrap_or("unbound"),
            fmt_time(record.last_seen(), zone),
            compact_field(record.last_segment_received_at(), now_ms),
            metric_display(stats, "segments_received"),
            fmt_bytes(metric_number(stats, "bytes_received"))
        ));
    }
    lines.join("\n")
}

pub fn render_status_all(
    records: &[ObserverRecord],
    json_output: bool,
    now_ms: i64,
    zone: TimeDisplay,
) -> String {
    if !json_output && records.is_empty() {
        return "No devices registered.".to_owned();
    }
    let labels: Vec<_> = records
        .iter()
        .map(|record| status_label(record, now_ms))
        .collect();
    let connected = labels.iter().filter(|&&label| label == "connected").count();
    let disconnected = labels
        .iter()
        .filter(|&&label| label == "disconnected")
        .count();
    let revoked = labels.iter().filter(|&&label| label == "revoked").count();
    let total_segments: f64 = records
        .iter()
        .map(|record| metric_number(record.stats(), "segments_received"))
        .sum();
    let total_bytes: f64 = records
        .iter()
        .map(|record| metric_number(record.stats(), "bytes_received"))
        .sum();
    if json_output {
        return serde_json::to_string(&json!({"total":records.len(),"connected":connected,"disconnected":disconnected,"revoked":revoked,"total_segments":number_value(total_segments),"total_bytes":number_value(total_bytes),"observers":records.iter().map(|record| status_entry(record, now_ms)).collect::<Vec<_>>() })).expect("JSON values serialize");
    }
    let mut lines = vec![
        format!("Devices: {} total", records.len()),
        format!("  Connected:    {connected}"),
        format!("  Disconnected: {disconnected}"),
        format!("  Revoked:      {revoked}"),
        format!("  Total segments: {}", format_number(total_segments, "")),
        format!("  Total bytes:    {}", fmt_bytes(total_bytes)),
        String::new(),
        format!(
            "{:<20} {:<18} {:<14} {:<10} {:<18} {:<12}",
            "Name", "Prefix", "Status", "Binding", "Last Seen", "Last Segment"
        ),
        "-".repeat(98),
    ];
    for record in records {
        lines.push(format!(
            "{:<20} {:<18} {:<14} {:<10} {:<18} {:<12}",
            record.name().unwrap_or_default(),
            record.prefix(),
            status_label(record, now_ms),
            record.device_binding_kind().unwrap_or("unbound"),
            fmt_time(record.last_seen(), zone),
            compact_field(record.last_segment_received_at(), now_ms)
        ));
    }
    lines.join("\n")
}

pub fn render_status_single(
    journal_root: &Path,
    record: &ObserverRecord,
    json_output: bool,
    now_ms: i64,
    zone: TimeDisplay,
) -> String {
    if json_output {
        let stats = record.stats();
        return serde_json::to_string(&json!({"name":record.name().unwrap_or_default(),"prefix":record.prefix(),"status":status_label(record, now_ms),"device_binding_kind":record.device_binding_kind(),"created_at":record.created_at(),"last_seen":record.last_seen(),"last_segment_received_at":record.last_segment_received_at(),"last_segment_day":record.last_segment_day(),"revoked":record.revoked(),"segments":metric_value(stats,"segments_received"),"bytes":metric_value(stats,"bytes_received")})).expect("JSON values serialize");
    }
    let today = display_time(now_ms, zone)
        .expect("valid current time")
        .format("%Y%m%d")
        .to_string();
    let today_read = load_history(&history_path(journal_root, &record.prefix(), &today));
    let history = today_read.records;
    let upload_records = uploads(&history);
    let (received_at, day) =
        if record.last_segment_received_at().is_none() && !upload_records.is_empty() {
            let last = upload_records.last().expect("nonempty");
            let timestamp = last.get("ts").and_then(Value::as_i64);
            (
                timestamp,
                timestamp
                    .map(|_| today.clone())
                    .or_else(|| record.last_segment_day().map(ToOwned::to_owned)),
            )
        } else {
            (
                record.last_segment_received_at(),
                record.last_segment_day().map(ToOwned::to_owned),
            )
        };
    let age = received_at
        .map(Value::from)
        .map(|value| fmt_compact_age(&value, now_ms))
        .unwrap_or_else(|| "—".to_owned());
    let context = day
        .filter(|day| !day.is_empty())
        .map(|day| format!("{age} ({day})"))
        .unwrap_or(age);
    let mut lines = vec![format!("Device: {}", record.name().unwrap_or_default())];
    field(&mut lines, "Prefix:", &record.prefix());
    field(&mut lines, "Status:", status_label(record, now_ms));
    field(
        &mut lines,
        "Binding:",
        record.device_binding_kind().unwrap_or("unbound"),
    );
    field(&mut lines, "Created:", &fmt_time(record.created_at(), zone));
    field(
        &mut lines,
        "Last seen:",
        &fmt_time(record.last_seen(), zone),
    );
    field(&mut lines, "Last segment:", &context);
    if record.revoked() {
        field(
            &mut lines,
            "Revoked at:",
            &fmt_time(record.revoked_at(), zone),
        );
    }
    field(
        &mut lines,
        "Segments:",
        &metric_display(record.stats(), "segments_received"),
    );
    field(
        &mut lines,
        "Bytes:",
        &fmt_bytes(metric_number(record.stats(), "bytes_received")),
    );
    if let Some(duplicates) = record
        .stats()
        .and_then(|stats| stats.get("duplicates_rejected"))
        .filter(|value| python_truthy(value))
    {
        field(&mut lines, "Duplicates:", &format!("{duplicates} rejected"));
    }
    if !history.is_empty() || today_read.stopped.is_some() {
        lines.push(String::new());
        lines.push(today_count_line(
            &today,
            upload_records.len(),
            today_read.stopped.as_ref(),
        ));
        for upload in upload_records.iter().rev().take(5).rev() {
            let files = upload
                .get("files")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let total: f64 = files
                .iter()
                .filter_map(Value::as_object)
                .map(|file| file.get("size").and_then(Value::as_f64).unwrap_or(0.0))
                .sum();
            lines.push(format!(
                "    {}  {} file(s)  {}  {}",
                upload.get("segment").and_then(Value::as_str).unwrap_or("?"),
                files.len(),
                fmt_bytes(total),
                fmt_time(python_numeric_ms(upload.get("ts")), zone)
            ));
        }
    }
    let mut days: Vec<String> = list_dir_entries(&history_dir(journal_root, &record.prefix()))
        .ok()
        .into_iter()
        .flatten()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .filter_map(|entry| {
            entry
                .name
                .to_str()
                .and_then(|name| name.strip_suffix(".jsonl"))
                .map(ToOwned::to_owned)
        })
        .collect();
    days.sort_by(|left, right| right.cmp(left));
    days.truncate(7);
    if !days.is_empty() {
        lines.push(String::new());
        lines.push("  Recent days:".to_owned());
        for day in days {
            let read = load_history(&history_path(journal_root, &record.prefix(), &day));
            lines.push(day_count_line(
                &day,
                uploads(&read.records).len(),
                read.stopped.as_ref(),
            ));
        }
    }
    lines.join("\n")
}

fn torn_suffix(stopped: Option<&HistoryStop>) -> String {
    match stopped {
        Some(HistoryStop::Malformed { line }) => format!("; history torn at line {line}"),
        Some(HistoryStop::Io) => "; history torn".to_owned(),
        None => String::new(),
    }
}

fn today_count_line(today: &str, count: usize, stopped: Option<&HistoryStop>) -> String {
    let tear = torn_suffix(stopped);
    if tear.is_empty() {
        format!("  Today ({today}): {count} segment(s) synced")
    } else {
        format!("  Today ({today}): {count} segment(s){tear}")
    }
}

fn day_count_line(day: &str, count: usize, stopped: Option<&HistoryStop>) -> String {
    format!("    {day}: {count} segment(s){}", torn_suffix(stopped))
}

fn list_entry(record: &ObserverRecord, now_ms: i64) -> Value {
    json!({"name":record.name().unwrap_or_default(),"prefix":record.prefix(),"status":status_label(record,now_ms),"device_binding_kind":record.device_binding_kind(),"last_seen":record.last_seen(),"last_segment_received_at":record.last_segment_received_at(),"last_segment_day":record.last_segment_day(),"segments":metric_value(record.stats(),"segments_received"),"bytes":metric_value(record.stats(),"bytes_received")})
}
fn status_entry(record: &ObserverRecord, now_ms: i64) -> Value {
    json!({"name":record.name().unwrap_or_default(),"prefix":record.prefix(),"status":status_label(record,now_ms),"device_binding_kind":record.device_binding_kind(),"last_seen":record.last_seen(),"last_segment_received_at":record.last_segment_received_at(),"last_segment_day":record.last_segment_day()})
}
fn metric_value(stats: Option<&Map<String, Value>>, key: &str) -> Value {
    stats
        .and_then(|stats| stats.get(key))
        .cloned()
        .unwrap_or_else(|| Value::from(0))
}
fn python_numeric_ms(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Bool(value)) => Some(i64::from(*value)),
        Some(value) => value.as_i64(),
        None => None,
    }
}
fn metric_number(stats: Option<&Map<String, Value>>, key: &str) -> f64 {
    metric_value(stats, key).as_f64().unwrap_or(0.0)
}
fn metric_display(stats: Option<&Map<String, Value>>, key: &str) -> String {
    metric_value(stats, key).to_string()
}
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}
fn compact_field(value: Option<i64>, now_ms: i64) -> String {
    value
        .map(Value::from)
        .map(|value| fmt_compact_age(&value, now_ms))
        .unwrap_or_else(|| "—".to_owned())
}
fn field(lines: &mut Vec<String>, label: &str, value: &str) {
    lines.push(format!("  {:<13} {value}", label));
}
fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn uploads(records: &[Value]) -> Vec<&Map<String, Value>> {
    let mut latest: HashMap<&str, Option<&str>> = HashMap::new();
    for record in records.iter().filter_map(Value::as_object) {
        if let Some(segment) = record
            .get("segment")
            .and_then(Value::as_str)
            .filter(|segment| !segment.is_empty())
        {
            latest.insert(segment, record.get("type").and_then(Value::as_str));
        }
    }
    records
        .iter()
        .filter_map(Value::as_object)
        .filter(|record| !record.get("type").is_some_and(python_truthy))
        .filter(|record| {
            record
                .get("segment")
                .and_then(Value::as_str)
                .is_none_or(|segment| latest.get(segment).copied().flatten() != Some("pruned"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn record() -> ObserverRecord {
        ObserverRecord::from_value(json!({"key":"abcdefghx","name":"one","last_seen":1000,"last_segment_received_at":null,"stats":{"segments_received":2,"bytes_received":1024}})).expect("record")
    }
    #[test]
    fn compact_age_has_python_em_dash_cases_and_bucket_centers() {
        assert_eq!(fmt_compact_age(&Value::Null, 1), "—");
        assert_eq!(fmt_compact_age(&json!(true), 1), "—");
        assert_eq!(fmt_compact_age(&json!(-1), 1), "—");
        assert_eq!(fmt_compact_age(&json!(10_000), 1), "—");
        assert_eq!(fmt_compact_age(&json!(70_000), 100_000), "30s");
        assert_eq!(fmt_compact_age(&json!(1_000), 1_801_000), "30m");
    }
    #[test]
    fn empty_registry_has_all_four_render_forms() {
        assert_eq!(
            render_list(&[], false, 0, TimeDisplay::Utc),
            "No devices registered."
        );
        assert_eq!(render_list(&[], true, 0, TimeDisplay::Utc), "[]");
        assert_eq!(
            render_status_all(&[], false, 0, TimeDisplay::Utc),
            "No devices registered."
        );
        assert!(render_status_all(&[], true, 0, TimeDisplay::Utc).contains("\"total\":0"));
    }
    #[test]
    fn list_uses_python_column_widths() {
        let output = render_list(&[record()], false, 2_000, TimeDisplay::Utc);
        assert_eq!(output.lines().nth(1), Some("-".repeat(118).as_str()));
    }

    #[test]
    fn status_day_counts_surface_tear() {
        use crate::store::paths::history_path;
        use crate::test_support::reserve_temp_path;
        use std::fs;
        const NOW: i64 = 1_767_236_400_000;
        let root = reserve_temp_path("observer-format-torn");
        let record = record();
        let path = history_path(&root, &record.prefix(), "20260101");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "{\"segment\":\"090000_300\",\"files\":[{\"size\":1}],\"ts\":1}\n{broken}\n{\"segment\":\"090000_301\",\"files\":[{\"size\":2}],\"ts\":2}\n",
        )
        .unwrap();
        let output = render_status_single(&root, &record, false, NOW, TimeDisplay::Utc);
        assert!(output.contains("history torn at line 2"));
        assert!(!output.contains("segment(s) synced"), "{output}");
        for line in output.lines() {
            if line.contains("segment(s)") {
                assert!(line.contains("history torn"), "{line}");
            }
        }
        fs::remove_dir_all(root).ok();
    }
}
