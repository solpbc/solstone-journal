// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure health-glance projection.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::model::{BacklogSource, BacklogValidity};
use crate::needs_you::format_degraded_capture_line;

pub const BACKLOG_FRESHNESS_MAX_AGE_HOURS: i64 = 36;

pub fn build_health_glance(
    capture: &Value,
    pipeline: &Value,
    last_observe: Option<&str>,
    backlog: &BacklogSource,
    brain: &Value,
    now: DateTime<Utc>,
) -> Value {
    let mut issues = backlog_issues(backlog, now);
    if let Some(issue) = capture_issue(capture) {
        issues.push(issue);
    }
    if let Some(issue) = pipeline_issue(pipeline) {
        issues.push(issue);
    }
    if let Some(issue) = brain_issue(brain) {
        issues.push(issue);
    }
    if !issues.is_empty() {
        let severity = if issues.iter().any(|issue| issue["severity"] == "red") {
            "red"
        } else {
            "amber"
        };
        let count = issues.len();
        return json!({"verdict":"attention","severity":severity,"headline":if count == 1 { "1 thing needs your attention".to_owned() } else { format!("{count} things need your attention") },"last_observation":null,"cta":null,"issues":issues});
    }
    let status = observer_state(capture);
    if status != "active" && status != "no_observers" {
        return json!({"verdict":"unavailable","severity":"amber","headline":"i don't know the status of your devices right now.","last_observation":null,"cta":null,"issues":[]});
    }
    if brain.get("state").and_then(Value::as_str) == Some("checking") {
        return json!({"verdict":"checking","severity":"amber","headline":brain.get("headline").cloned().unwrap_or(Value::Null),"last_observation":null,"cta":null,"issues":[]});
    }
    if brain.get("state").and_then(Value::as_str) == Some("blocked")
        && brain.get("progressing").and_then(Value::as_bool) == Some(true)
    {
        return json!({"verdict":"progressing","severity":"amber","headline":brain.get("headline").cloned().unwrap_or(Value::Null),"last_observation":null,"cta":null,"issues":[]});
    }
    if status == "active" {
        json!({"verdict":"ok","severity":"green","headline":"everything's working","last_observation":last_observe,"cta":null,"issues":[]})
    } else {
        json!({"verdict":"ok","severity":"green","headline":"no devices are running sol yet. set one up to start your journal.","last_observation":null,"cta":{"text":"set one up →","href":"/app/network/"},"issues":[]})
    }
}

pub fn observer_state(capture: &Value) -> &'static str {
    match capture.get("status").and_then(Value::as_str) {
        Some("active") => "active",
        Some("no_observers") => "no_observers",
        _ => "unknown",
    }
}
fn backlog_issues(source: &BacklogSource, now: DateTime<Utc>) -> Vec<Value> {
    if source.validity != BacklogValidity::Valid {
        return vec![unknown_backlog()];
    }
    let Some(backlog) = &source.backlog else {
        return vec![unknown_backlog()];
    };
    let mut issues = Vec::new();
    if backlog.get("degraded").and_then(Value::as_bool) == Some(true) {
        issues.push(unknown_backlog());
    }
    match source.generated_at.as_deref().and_then(parse_time) { Some(generated) if now - generated <= Duration::hours(BACKLOG_FRESHNESS_MAX_AGE_HOURS) => {}, Some(generated) => issues.push(json!({"text":format!("i can't tell if your journal is caught up; the last update was {} ago.", age(now - generated)),"severity":"amber","href":"/app/health"})), None => issues.push(json!({"text":"i can't tell if your journal is caught up; the last update age is unknown.","severity":"amber","href":"/app/health"})), };
    if backlog
        .get("stuck_days")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        > 0
    {
        let text = backlog
            .get("days")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .and_then(|row| row.get("reason"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("a journal day needs a hand.");
        issues.push(json!({"text":text,"severity":"red","href":"/app/health"}));
    }
    issues
}
fn unknown_backlog() -> Value {
    json!({"text":"i can't tell if your journal is caught up right now.","severity":"amber","href":"/app/health"})
}
fn capture_issue(capture: &Value) -> Option<Value> {
    match capture.get("status").and_then(Value::as_str) {
        Some("degraded") => Some(
            json!({"text":format_degraded_capture_line(capture).expect("degraded"),"severity":"red","href":"/app/health"}),
        ),
        Some("offline") => Some(
            json!({"text":"sol hasn't added anything to your journal recently.","severity":"red","href":"/app/health"}),
        ),
        Some("stale") => Some(
            json!({"text":"sol on one of your devices has not added anything to your journal recently.","severity":"amber","href":"/app/health"}),
        ),
        _ => None,
    }
}
fn pipeline_issue(pipeline: &Value) -> Option<Value> {
    if !pipeline.is_object() || pipeline.as_object().is_some_and(|row| row.is_empty()) {
        return None;
    }
    let text = pipeline
        .get("headline")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("processing is behind");
    Some(
        json!({"text":text,"severity":"amber","href":if pipeline.get("suggested_action").and_then(Value::as_str) == Some("open_support") { "/app/support" } else { "/app/health#focus=recent-errors&day=today" }}),
    )
}
fn brain_issue(brain: &Value) -> Option<Value> {
    let state = brain.get("state").and_then(Value::as_str)?;
    if state == "ready"
        || state == "checking"
        || (state == "blocked" && brain.get("progressing").and_then(Value::as_bool) == Some(true))
    {
        return None;
    }
    let text = brain.get("headline").and_then(Value::as_str)?.trim();
    if text.is_empty()
        || !matches!(state, "blocked" | "unhealthy" | "unknown")
            && !brain.get("action").is_some_and(Value::is_object)
    {
        return None;
    }
    Some(
        json!({"text":text,"severity":"amber","href":brain.pointer("/action/href").and_then(Value::as_str).unwrap_or("/app/health/#brain")}),
    )
}
fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
        .or_else(|| {
            value
                .parse::<chrono::NaiveDateTime>()
                .ok()
                .map(|time| time.and_utc())
        })
}
fn age(delta: Duration) -> String {
    let seconds = delta.num_seconds().max(0);
    let hours = seconds / 3600;
    if hours >= 1 {
        format!("{hours} hour{}", if hours == 1 { "" } else { "s" })
    } else {
        let minutes = (seconds / 60).max(1);
        format!("{minutes} minute{}", if minutes == 1 { "" } else { "s" })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn injected_inputs_produce_active_and_unavailable_verdicts() {
        let backlog = BacklogSource {
            backlog: Some(json!({"stuck_days":0}).as_object().unwrap().clone()),
            validity: BacklogValidity::Valid,
            generated_at: Some("2026-05-14T12:00:00+00:00".to_owned()),
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 14, 15, 30, 0).unwrap();
        assert_eq!(
            build_health_glance(
                &json!({"status":"active"}),
                &json!({}),
                Some("29 seconds ago"),
                &backlog,
                &Value::Null,
                now
            )["verdict"],
            "ok"
        );
        assert_eq!(
            build_health_glance(
                &json!({"status":"offline"}),
                &json!({}),
                None,
                &backlog,
                &Value::Null,
                now
            )["verdict"],
            "attention"
        );
    }
}
