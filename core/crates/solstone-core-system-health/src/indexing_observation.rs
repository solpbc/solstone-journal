// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only indexing observations and equal-timestamp outcome fold across
//! Think health operational logs and Cortex talent summaries.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_format::paths::relative_to_journal;
use solstone_core_format::segment::is_date_key;
use solstone_core_journal_config::owner_zone;
use solstone_core_journal_io::{MalformedPolicy, day_dirs, read_jsonl_with_report};

use crate::vocabulary::BACKLOG_DEFAULT_WINDOW;
use crate::{HealthEvent, RunLogRecord};

pub const LIMIT_TOKEN_UNOBSERVED_OUTSIDE_DAYS: &str =
    "unobserved_history_outside_selected_think_days";
pub const LIMIT_TOKEN_UNFINISHED_NO_DAY_INDEX: &str = "unfinished_uses_have_no_day_index_row";
pub const LIMIT_TOKEN_FAILURES_NOT_RETAINED_PAST_HISTORY: &str =
    "failures_not_retained_past_inspected_history";
pub const LIMIT_TOKEN_PUBLICATION_NOT_CRASH_ATOMIC: &str =
    "publication_notification_not_crash_atomic";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingAttemptObservation {
    pub path: String,
    pub identity: String,
    pub outcome: String,
    pub ts: i64,
    pub warnings: Vec<String>,
    pub cause: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingDiagnostics {
    pub think_days: Vec<String>,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub partial: bool,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingObservations {
    pub outcomes: Vec<IndexingAttemptObservation>,
    pub diagnostics: IndexingDiagnostics,
}

#[derive(Debug, Clone)]
struct RawAttempt {
    path: String,
    identity: String,
    outcome: String,
    ts: i64,
    warnings: Vec<String>,
    cause: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeCategory {
    Indexed,
    Excluded,
    Declined,
    Failed,
    Ambiguous,
    Unknown,
}

impl OutcomeCategory {
    fn from_outcome_str(s: &str) -> Self {
        match s {
            "indexed" => Self::Indexed,
            "excluded" => Self::Excluded,
            "declined" => Self::Declined,
            "failed" => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

/// Normalize recorded path identity for search indexing:
/// - Absolute paths under the journal root are stripped to journal-relative paths.
/// - Relative paths with backslashes or `..` keep the raw recorded string.
/// - Relative paths drop empty and `.` components, and drop a leading `chronicle/`
///   component when immediately followed by an 8-digit date key.
pub fn normalize_indexing_identity(journal: &Path, raw_path: &str) -> String {
    let p = Path::new(raw_path);
    if p.is_absolute() {
        if let Some(rel) = relative_to_journal(journal, p) {
            return rel;
        }
        return raw_path.to_owned();
    }
    if raw_path.contains('\\') || raw_path.split('/').any(|part| part == "..") {
        return raw_path.to_owned();
    }
    let parts: Vec<&str> = raw_path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        return raw_path.to_owned();
    }
    if parts.len() >= 2 && parts[0] == "chronicle" && is_date_key(parts[1]) {
        return parts[1..].join("/");
    }
    parts.join("/")
}

/// Computes the inclusive owner-zone calendar-day window bounds:
/// Start is local midnight of (today - 29 days) in the owner zone,
/// choosing the earlier offset when ambiguous and the later offset in a gap.
/// End is `now` as epoch milliseconds.
pub fn indexing_window_bounds(journal: &Path, now: DateTime<Utc>) -> (i64, i64) {
    let zone = owner_zone(journal);
    let today = now.with_timezone(&zone).date_naive();
    let start_day = today
        .checked_sub_signed(TimeDelta::days((BACKLOG_DEFAULT_WINDOW - 1) as i64))
        .unwrap_or(today);
    let midnight = start_day.and_hms_opt(0, 0, 0).unwrap();
    let start_dt = zone
        .from_local_datetime(&midnight)
        .earliest()
        .or_else(|| {
            zone.from_local_datetime(&(midnight + TimeDelta::hours(1)))
                .earliest()
        })
        .unwrap_or_else(|| now.with_timezone(&zone));
    let window_start_ms = start_dt.timestamp_millis();
    let window_end_ms = now.timestamp_millis();
    (window_start_ms, window_end_ms)
}

/// Read indexing observations from Think operational logs and Cortex talent summaries.
///
/// Search and index health relies on journal publication attempts folded across
/// chronicle days and talents logs:
/// - Think days are selected from the newest chronicle day directories up to
///   `BACKLOG_DEFAULT_WINDOW` (30), sorted descending.
/// - Event timestamps are bounded by the inclusive 30-calendar-day owner-zone window.
/// - A partial read keeps any failure already observed from readable files.
/// - Chronicle days outside the selected 30 directories are unobserved even if an event
///   inside them carries a recent timestamp within the window.
/// - A thinking use which never finishes has no day-index row in `talents/<day>.jsonl`.
/// - This landing does not retain index failures past inspected history and does not
///   make publication notification crash-atomic.
pub fn read_indexing_observations(journal: &Path, now: DateTime<Utc>) -> IndexingObservations {
    let mut partial = false;
    let mut detail_lines = Vec::new();
    let (window_start_ms, window_end_ms) = indexing_window_bounds(journal, now);

    let (think_days, selected_days) = match day_dirs(journal) {
        Ok(dir_map) => {
            let mut days: Vec<String> = dir_map.into_keys().collect();
            days.sort_by(|a, b| b.cmp(a));
            days.truncate(BACKLOG_DEFAULT_WINDOW);
            (days.clone(), days)
        }
        Err(err) => {
            partial = true;
            log::warn!("failed to list chronicle day directories: {err}");
            detail_lines.push(format!("failed to list chronicle days: {err}"));
            (Vec::new(), Vec::new())
        }
    };

    let mut raw_attempts: Vec<RawAttempt> = Vec::new();

    // 1. Scan selected chronicle think days
    for day in &selected_days {
        let health_dir = journal.join("chronicle").join(day).join("health");
        let entries = match fs::read_dir(&health_dir) {
            Ok(read_dir) => {
                let mut paths = Vec::new();
                for entry in read_dir {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            partial = true;
                            detail_lines
                                .push(format!("unreadable health directory entry: {error}"));
                            continue;
                        }
                    };
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                        paths.push(path);
                    }
                }
                paths.sort();
                paths
            }
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    partial = true;
                    log::warn!(
                        "failed to read health directory {}: {err}",
                        health_dir.display()
                    );
                    detail_lines.push(format!(
                        "unreadable health directory {}: {err}",
                        health_dir.display()
                    ));
                }
                continue;
            }
        };

        for path in entries {
            let report = match read_jsonl_with_report::<RunLogRecord>(
                &path,
                Vec::new(),
                MalformedPolicy::Skip,
            ) {
                Ok(rep) => rep,
                Err(err) => {
                    partial = true;
                    log::warn!("failed to read health log {}: {err}", path.display());
                    detail_lines.push(format!("unreadable health log {}: {err}", path.display()));
                    continue;
                }
            };
            if report.malformed_line_count > 0 {
                partial = true;
                detail_lines.push(format!("malformed lines in {}", path.display()));
            }
            for record in report.records {
                let HealthEvent::Unknown(kind, fields) = record.value.event else {
                    continue;
                };
                if kind != "index.attempt" {
                    continue;
                }
                let path_str = match fields.get("path").and_then(Value::as_str) {
                    Some(p) if !p.is_empty() => p.to_owned(),
                    _ => {
                        partial = true;
                        detail_lines.push("index.attempt missing path in think log".to_owned());
                        continue;
                    }
                };
                let outcome = fields
                    .get("outcome")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();
                let warnings = fields
                    .get("warnings")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                let cause = fields
                    .get("cause")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let ts = record.value.ts;
                if ts >= window_start_ms && ts <= window_end_ms {
                    let identity = normalize_indexing_identity(journal, &path_str);
                    raw_attempts.push(RawAttempt {
                        path: path_str,
                        identity,
                        outcome,
                        ts,
                        warnings,
                        cause,
                    });
                }
            }
        }
    }

    // 2. Scan flat talents/*.jsonl in Cortex store
    let talents_dir = journal.join("talents");
    match fs::read_dir(&talents_dir) {
        Ok(read_dir) => {
            let mut paths = Vec::new();
            for entry in read_dir {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        partial = true;
                        detail_lines.push(format!("unreadable talent directory entry: {error}"));
                        continue;
                    }
                };
                let path = entry.path();
                if path.extension().and_then(|x| x.to_str()) == Some("jsonl")
                    && path
                        .file_stem()
                        .and_then(|x| x.to_str())
                        .is_some_and(|stem| stem.len() == 8)
                {
                    paths.push(path);
                }
            }
            paths.sort();

            for path in paths {
                let content = match fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(err) => {
                        partial = true;
                        log::warn!("failed to read talent log {}: {err}", path.display());
                        detail_lines
                            .push(format!("unreadable talent log {}: {err}", path.display()));
                        continue;
                    }
                };

                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let value: Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => {
                            partial = true;
                            detail_lines.push(format!("malformed line in {}", path.display()));
                            continue;
                        }
                    };
                    let Some(obj) = value.as_object() else {
                        continue;
                    };
                    let Some(attempts_val) = obj.get("index_attempts") else {
                        continue;
                    };
                    let Some(attempts_arr) = attempts_val.as_array() else {
                        partial = true;
                        detail_lines.push(format!(
                            "index_attempts is not an array in {}",
                            path.display()
                        ));
                        continue;
                    };

                    for item in attempts_arr {
                        let Some(item_obj) = item.as_object() else {
                            partial = true;
                            continue;
                        };
                        let Some(ts) = item_obj.get("ts").and_then(Value::as_i64) else {
                            partial = true;
                            detail_lines.push(format!("missing attempt ts in {}", path.display()));
                            continue;
                        };
                        let Some(path_str) = item_obj.get("path").and_then(Value::as_str) else {
                            partial = true;
                            detail_lines
                                .push(format!("missing attempt path in {}", path.display()));
                            continue;
                        };
                        if path_str.is_empty() {
                            partial = true;
                            continue;
                        }
                        let outcome = item_obj
                            .get("outcome")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_owned();
                        let warnings = item_obj
                            .get("warnings")
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let cause = item_obj
                            .get("cause")
                            .and_then(Value::as_str)
                            .map(str::to_owned);

                        if ts >= window_start_ms && ts <= window_end_ms {
                            let identity = normalize_indexing_identity(journal, path_str);
                            raw_attempts.push(RawAttempt {
                                path: path_str.to_owned(),
                                identity,
                                outcome,
                                ts,
                                warnings,
                                cause,
                            });
                        }
                    }
                }
            }
        }
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                partial = true;
                log::warn!(
                    "failed to read talents directory {}: {err}",
                    talents_dir.display()
                );
                detail_lines.push(format!(
                    "unreadable talents directory {}: {err}",
                    talents_dir.display()
                ));
            }
        }
    }

    // 3. Fold attempts by identity then timestamp
    let mut by_identity: BTreeMap<String, BTreeMap<i64, Vec<RawAttempt>>> = BTreeMap::new();
    for attempt in raw_attempts {
        by_identity
            .entry(attempt.identity.clone())
            .or_default()
            .entry(attempt.ts)
            .or_default()
            .push(attempt);
    }

    let mut outcomes = Vec::new();
    for (identity, buckets) in by_identity {
        let mut current_state: Option<(OutcomeCategory, RawAttempt)> = None;

        for (_ts, mut attempts) in buckets {
            let mut has_failed = false;
            let mut has_declined = false;
            let mut has_unknown = false;
            let mut has_indexed = false;
            let mut has_excluded = false;

            for att in &attempts {
                match OutcomeCategory::from_outcome_str(&att.outcome) {
                    OutcomeCategory::Failed => has_failed = true,
                    OutcomeCategory::Declined => has_declined = true,
                    OutcomeCategory::Indexed => has_indexed = true,
                    OutcomeCategory::Excluded => has_excluded = true,
                    OutcomeCategory::Unknown | OutcomeCategory::Ambiguous => has_unknown = true,
                }
            }

            let bad_count = (has_failed as u8) + (has_declined as u8) + (has_unknown as u8);
            let bucket_cat = if bad_count >= 2 {
                OutcomeCategory::Ambiguous
            } else if has_failed {
                OutcomeCategory::Failed
            } else if has_declined {
                OutcomeCategory::Declined
            } else if has_unknown {
                OutcomeCategory::Unknown
            } else if has_indexed {
                OutcomeCategory::Indexed
            } else if has_excluded {
                OutcomeCategory::Excluded
            } else {
                OutcomeCategory::Unknown
            };

            // Filter candidates that match winning category
            attempts.retain(|att| {
                let cat = OutcomeCategory::from_outcome_str(&att.outcome);
                match bucket_cat {
                    OutcomeCategory::Ambiguous => {
                        matches!(
                            cat,
                            OutcomeCategory::Failed
                                | OutcomeCategory::Declined
                                | OutcomeCategory::Unknown
                                | OutcomeCategory::Ambiguous
                        )
                    }
                    OutcomeCategory::Failed => cat == OutcomeCategory::Failed,
                    OutcomeCategory::Declined => cat == OutcomeCategory::Declined,
                    OutcomeCategory::Indexed => cat == OutcomeCategory::Indexed,
                    OutcomeCategory::Excluded => cat == OutcomeCategory::Excluded,
                    OutcomeCategory::Unknown => cat == OutcomeCategory::Unknown,
                }
            });

            // Tie-break: lexicographically smallest (path, cause.unwrap_or_default(), warnings joined by \n)
            attempts.sort_by(|a, b| {
                let a_cause = a.cause.as_deref().unwrap_or_default();
                let b_cause = b.cause.as_deref().unwrap_or_default();
                let a_warn = a.warnings.join("\n");
                let b_warn = b.warnings.join("\n");
                (a.path.as_str(), a_cause, a_warn.as_str()).cmp(&(
                    b.path.as_str(),
                    b_cause,
                    b_warn.as_str(),
                ))
            });

            let winning_attempt = attempts
                .into_iter()
                .next()
                .expect("bucket has at least one attempt");

            // Apply progression rules:
            match bucket_cat {
                OutcomeCategory::Indexed
                | OutcomeCategory::Failed
                | OutcomeCategory::Declined
                | OutcomeCategory::Ambiguous => {
                    current_state = Some((bucket_cat, winning_attempt));
                }
                OutcomeCategory::Excluded => {
                    if current_state.is_none()
                        || matches!(
                            current_state.as_ref().map(|(c, _)| *c),
                            Some(OutcomeCategory::Excluded)
                        )
                    {
                        current_state = Some((bucket_cat, winning_attempt));
                    }
                }
                OutcomeCategory::Unknown => {
                    if current_state.is_none()
                        || matches!(
                            current_state.as_ref().map(|(c, _)| *c),
                            Some(OutcomeCategory::Excluded)
                        )
                    {
                        current_state = Some((bucket_cat, winning_attempt));
                    }
                }
            }
        }

        if let Some((cat, winner)) = current_state {
            let outcome_str = match cat {
                OutcomeCategory::Indexed => "indexed".to_owned(),
                OutcomeCategory::Excluded => "excluded".to_owned(),
                OutcomeCategory::Declined => "declined".to_owned(),
                OutcomeCategory::Failed => "failed".to_owned(),
                OutcomeCategory::Ambiguous => "ambiguous".to_owned(),
                OutcomeCategory::Unknown => winner.outcome,
            };
            outcomes.push(IndexingAttemptObservation {
                path: winner.path,
                identity,
                outcome: outcome_str,
                ts: winner.ts,
                warnings: winner.warnings,
                cause: winner.cause,
            });
        }
    }

    outcomes.sort_by(|a, b| a.identity.cmp(&b.identity));

    for outcome in &outcomes {
        if matches!(
            outcome.outcome.as_str(),
            "failed" | "declined" | "ambiguous"
        ) {
            let cause = outcome.cause.as_deref().unwrap_or("");
            let warnings = if outcome.warnings.is_empty() {
                String::new()
            } else {
                format!(" warnings: {}", outcome.warnings.join(", "))
            };
            let cause_text = if cause.is_empty() {
                String::new()
            } else {
                format!(" cause: {cause}")
            };
            detail_lines.push(format!(
                "attempt {} {}{}{}",
                outcome.path, outcome.outcome, cause_text, warnings
            ));
        }
    }

    detail_lines.sort();

    let mut lines = vec![
        LIMIT_TOKEN_UNOBSERVED_OUTSIDE_DAYS.to_owned(),
        LIMIT_TOKEN_UNFINISHED_NO_DAY_INDEX.to_owned(),
        LIMIT_TOKEN_FAILURES_NOT_RETAINED_PAST_HISTORY.to_owned(),
        LIMIT_TOKEN_PUBLICATION_NOT_CRASH_ATOMIC.to_owned(),
    ];
    lines.extend(detail_lines);

    IndexingObservations {
        outcomes,
        diagnostics: IndexingDiagnostics {
            think_days,
            window_start_ms,
            window_end_ms,
            partial,
            lines,
        },
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_utc_journal(dir: &Path) {
        let config_dir = dir.join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
    }

    #[test]
    fn window_start_and_now_boundaries() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
        let (start, end) = indexing_window_bounds(dir.path(), now);
        let start_dt = Utc.with_ymd_and_hms(2026, 9, 6, 0, 0, 0).unwrap();
        assert_eq!(start, start_dt.timestamp_millis());
        assert_eq!(end, now.timestamp_millis());

        // Chronicle day with health log
        let health_dir = dir.path().join("chronicle/20261005/health");
        fs::create_dir_all(&health_dir).unwrap();
        let log_path = health_dir.join("think.test.jsonl");

        let in_start = serde_json::json!({
            "event": "index.attempt",
            "ts": start,
            "path": "20260906/talents/flow.md",
            "outcome": "failed",
            "cause": "start edge"
        });
        let before_start = serde_json::json!({
            "event": "index.attempt",
            "ts": start - 1,
            "path": "20260905/talents/flow.md",
            "outcome": "failed",
            "cause": "before start"
        });
        let at_now = serde_json::json!({
            "event": "index.attempt",
            "ts": end,
            "path": "20261005/talents/now.md",
            "outcome": "failed",
            "cause": "at now"
        });
        let after_now = serde_json::json!({
            "event": "index.attempt",
            "ts": end + 1,
            "path": "20261005/talents/now.md",
            "outcome": "indexed" // Must not clear failure
        });

        fs::write(
            &log_path,
            format!(
                "{}\n{}\n{}\n{}\n",
                in_start, before_start, at_now, after_now
            ),
        )
        .unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        let identities: Vec<&str> = obs.outcomes.iter().map(|o| o.identity.as_str()).collect();
        assert!(identities.contains(&"20260906/talents/flow.md"));
        assert!(!identities.contains(&"20260905/talents/flow.md"));

        let now_item = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/talents/now.md")
            .unwrap();
        assert_eq!(now_item.outcome, "failed");
    }

    #[test]
    fn equal_timestamp_and_multi_outcome_progression() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
        let health_dir = dir.path().join("chronicle/20261005/health");
        fs::create_dir_all(&health_dir).unwrap();

        let base_ts = now.timestamp_millis() - 50_000;
        // 1. Equal ts failed + indexed stays failed
        let row1 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/a.md","outcome":"indexed"});
        let row2 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/a.md","outcome":"failed","cause":"bad"});
        // 2. Equal ts failed + declined is ambiguous
        let row3 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/b.md","outcome":"declined"});
        let row4 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/b.md","outcome":"failed"});
        // 3. Equal ts indexed + excluded is indexed
        let row5 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/c.md","outcome":"excluded"});
        let row6 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/c.md","outcome":"indexed"});

        // 4. Later indexed on same identity clears failure; later excluded or unknown does not; different path does not
        let row7 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/d.md","outcome":"failed"});
        let row8 = serde_json::json!({"event":"index.attempt","ts":base_ts + 1000,"path":"20261005/d.md","outcome":"indexed"});

        let row9 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"20261005/e.md","outcome":"failed"});
        let row10 = serde_json::json!({"event":"index.attempt","ts":base_ts + 1000,"path":"20261005/e.md","outcome":"excluded"});
        let row11 = serde_json::json!({"event":"index.attempt","ts":base_ts + 2000,"path":"20261005/e.md","outcome":"some_unknown"});

        let row12 = serde_json::json!({"event":"index.attempt","ts":base_ts + 3000,"path":"20261005/other.md","outcome":"indexed"});
        let row13 = serde_json::json!({"event":"phase.complete","ts":base_ts + 4000,"phase":"indexer","extensions":{"success":true}});

        let content = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            row1, row2, row3, row4, row5, row6, row7, row8, row9, row10, row11, row12, row13
        );
        fs::write(health_dir.join("think.test.jsonl"), content).unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        let a = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/a.md")
            .unwrap();
        assert_eq!(a.outcome, "failed");

        let b = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/b.md")
            .unwrap();
        assert_eq!(b.outcome, "ambiguous");

        let c = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/c.md")
            .unwrap();
        assert_eq!(c.outcome, "indexed");

        let d = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/d.md")
            .unwrap();
        assert_eq!(d.outcome, "indexed");

        let e = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/e.md")
            .unwrap();
        assert_eq!(e.outcome, "failed");
    }

    #[test]
    fn path_identity_equivalence_and_missing_file_display() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
        let health_dir = dir.path().join("chronicle/20261005/health");
        fs::create_dir_all(&health_dir).unwrap();

        let base_ts = now.timestamp_millis() - 50_000;
        let abs = dir.path().join("chronicle/20261005/talents/flow.md");
        let row1 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":"chronicle/20261005/talents/flow.md","outcome":"failed"});
        let row2 = serde_json::json!({"event":"index.attempt","ts":base_ts + 1000,"path":"20261005/talents/flow.md","outcome":"failed"});
        let row3 = serde_json::json!({"event":"index.attempt","ts":base_ts + 2000,"path":abs.to_str().unwrap(),"outcome":"indexed"});

        // Missing file display path kept
        let missing_path = "/nonexistent/path/outside/journal/note.md";
        let row4 = serde_json::json!({"event":"index.attempt","ts":base_ts,"path":missing_path,"outcome":"failed","cause":"missing"});

        fs::write(
            health_dir.join("think.test.jsonl"),
            format!("{}\n{}\n{}\n{}\n", row1, row2, row3, row4),
        )
        .unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        assert_eq!(obs.outcomes.len(), 2);
        let flow = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/talents/flow.md")
            .unwrap();
        assert_eq!(flow.outcome, "indexed");
        assert_eq!(flow.ts, base_ts + 2000);

        let missing = obs
            .outcomes
            .iter()
            .find(|o| o.identity == missing_path)
            .unwrap();
        assert_eq!(missing.outcome, "failed");
        assert_eq!(missing.path, missing_path);
    }

    #[test]
    fn talents_file_old_name_with_attempt_inside_window() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
        let talents_dir = dir.path().join("talents");
        fs::create_dir_all(&talents_dir).unwrap();

        // 19700101.jsonl with outer ts 0 / 1000 (outside window), but attempt ts inside window
        let summary1 = serde_json::json!({
            "use_id": "one",
            "name": "flow",
            "day": "19700101",
            "ts": 1000,
            "status": "completed",
            "index_attempts": [
                {
                    "ts": now.timestamp_millis() - 5000,
                    "path": "20261005/talents/flow.md",
                    "outcome": "failed",
                    "warnings": [],
                    "cause": "write error"
                }
            ]
        });
        // Summary without index_attempts field -> unobserved, clears nothing
        let summary2 = serde_json::json!({
            "use_id": "two",
            "name": "flow",
            "day": "19700101",
            "ts": 2000,
            "status": "completed"
        });
        // Summary with empty array -> clears nothing
        let summary3 = serde_json::json!({
            "use_id": "three",
            "name": "flow",
            "day": "19700101",
            "ts": 3000,
            "status": "completed",
            "index_attempts": []
        });

        fs::write(
            talents_dir.join("19700101.jsonl"),
            format!("{}\n{}\n{}\n", summary1, summary2, summary3),
        )
        .unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        let flow = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/talents/flow.md")
            .unwrap();
        assert_eq!(flow.outcome, "failed");
    }

    #[test]
    fn unreadable_sibling_health_log_keeps_failure_and_sets_partial() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();
        let health_dir = dir.path().join("chronicle/20261005/health");
        fs::create_dir_all(&health_dir).unwrap();

        let readable = health_dir.join("think.a.jsonl");
        fs::write(
            &readable,
            serde_json::json!({
                "event": "index.attempt",
                "ts": now.timestamp_millis() - 1000,
                "path": "20261005/note.md",
                "outcome": "failed",
                "cause": "unreadable test"
            })
            .to_string(),
        )
        .unwrap();

        let unreadable = health_dir.join("think.b.jsonl");
        fs::create_dir(&unreadable).unwrap();

        let obs = read_indexing_observations(dir.path(), now);
        assert!(
            obs.diagnostics
                .lines
                .iter()
                .any(|line| line.starts_with("unreadable health log"))
        );

        assert!(obs.diagnostics.partial);
        let note = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20261005/note.md")
            .unwrap();
        assert_eq!(note.outcome, "failed");
    }

    #[test]
    fn thirty_first_oldest_chronicle_directory_is_unobserved() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap();

        // Create 31 chronicle days: 20260901 through 20261001 (31 days)
        for d in 1..=31 {
            let day_str = format!("202609{:02}", d);
            let health_dir = dir.path().join("chronicle").join(&day_str).join("health");
            fs::create_dir_all(&health_dir).unwrap();
            let row = serde_json::json!({
                "event": "index.attempt",
                "ts": now.timestamp_millis() - 1000,
                "path": format!("{day_str}/note.md"),
                "outcome": "failed"
            });
            fs::write(health_dir.join("think.jsonl"), row.to_string()).unwrap();
        }

        let obs = read_indexing_observations(dir.path(), now);
        assert_eq!(obs.diagnostics.think_days.len(), 30);
        // 20260901 is the 31st (oldest), so it should be unobserved
        assert!(!obs.diagnostics.think_days.contains(&"20260901".to_string()));
        let unobserved = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20260901/note.md");
        assert!(unobserved.is_none());

        // 20260902 is within the 30 newest, so it is observed
        assert!(obs.diagnostics.think_days.contains(&"20260902".to_string()));
        let observed = obs
            .outcomes
            .iter()
            .find(|o| o.identity == "20260902/note.md");
        assert!(observed.is_some());
    }
}
