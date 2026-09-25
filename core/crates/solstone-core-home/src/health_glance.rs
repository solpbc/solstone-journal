// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure health-glance projection.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::formatting::relative_time;
use crate::model::{BacklogSource, BacklogValidity};
use crate::needs_you::format_degraded_capture_line;

const UNAVAILABLE_HEADLINE: &str = "your devices' status is unclear right now.";
const EMPTY_REGISTRY_HEADLINE: &str =
    "no devices are running the solstone app yet. set one up to start your journal.";
const AWAITING_FIRST_HEADLINE: &str =
    "the solstone app on one of your devices hasn't added anything to your journal yet.";
const RESIDUE_HEADLINE: &str = "nothing needs your attention right now.";
const NO_ELIGIBLE_HEADLINE: &str =
    "none of your devices have the solstone app set up to add to your journal right now.";
// An idle device is a device with no new material. Reachability is a heartbeat
// window, not something the journal measures for its own host, so the line says
// what is known — nothing was added, and when the last thing was — and stops.
// Day 0 with a device already delivering: things are arriving, and the summary
// that could prove the rest hasn't had its first chance yet. Calm, never green.
const NOT_YET_ACTIVE_HEADLINE: &str = "the solstone app is adding to your journal.";
const UNNAMED_IDLE_ISSUE: &str =
    "the solstone app on one of your devices hasn't added anything to your journal recently.";

#[derive(Clone, Copy)]
enum CalmKind {
    EmptyRegistry,
    AwaitingFirst,
    Residue,
    NoEligible,
}

#[derive(Clone, Copy)]
enum CaptureDisposition {
    Active,
    Quiet,
    Calm(CalmKind),
    Unavailable,
}

pub fn build_health_glance(
    capture: &Value,
    pipeline: &Value,
    last_observe: Option<&str>,
    backlog: &BacklogSource,
    brain: &Value,
    now: DateTime<Utc>,
) -> Value {
    let mut glance = derive_glance(capture, pipeline, last_observe, backlog, brain, now);
    if let BacklogValidity::NotYet(not_yet) = backlog.validity {
        // Not counted, never hidden: the note rides every verdict. Green is
        // earned by a summary, so an `ok` the note sits beside becomes calm.
        if glance["verdict"] == "ok" {
            glance["verdict"] = json!("calm");
            glance["severity"] = json!("neutral");
            glance["headline"] = json!(NOT_YET_ACTIVE_HEADLINE);
        }
        glance["note"] = json!({"text": not_yet.text(), "href": not_yet.href()});
    }
    glance
}

fn derive_glance(
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
        let headline = if count == 1 {
            "1 thing needs your attention".to_owned()
        } else {
            format!("{count} things need your attention")
        };
        return json!({"verdict":"attention","severity":severity,"headline":headline,"last_observation":null,"cta":null,"issues":issues});
    }
    let disposition = capture_disposition(capture);
    if matches!(disposition, CaptureDisposition::Unavailable) {
        return unavailable_json();
    }
    if brain.get("state").and_then(Value::as_str) == Some("checking") {
        return json!({"verdict":"checking","severity":"amber","headline":brain.get("headline").cloned().unwrap_or(Value::Null),"last_observation":null,"cta":null,"issues":[]});
    }
    if brain.get("state").and_then(Value::as_str) == Some("blocked")
        && brain.get("progressing").and_then(Value::as_bool) == Some(true)
    {
        return json!({"verdict":"progressing","severity":"amber","headline":brain.get("headline").cloned().unwrap_or(Value::Null),"last_observation":null,"cta":null,"issues":[]});
    }
    match disposition {
        CaptureDisposition::Quiet => {
            let summary = quiet_capture_summary(capture, now);
            json!({"verdict":"calm","severity":"neutral","headline":summary,"last_observation":last_observe,"cta":{"text":"view devices →","href":"/app/health/#registeredClientsCard"},"issues":[]})
        }
        CaptureDisposition::Active => {
            json!({"verdict":"ok","severity":"green","headline":"everything's working","last_observation":last_observe,"cta":null,"issues":[]})
        }
        CaptureDisposition::Calm(kind) => calm_json(kind),
        CaptureDisposition::Unavailable => unavailable_json(),
    }
}

pub fn client_state(capture: &Value) -> &'static str {
    match capture.get("status").and_then(Value::as_str) {
        Some("active") => "active",
        Some("no_clients") => "no_clients",
        _ => "unknown",
    }
}

fn capture_disposition(capture: &Value) -> CaptureDisposition {
    let status = capture
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if !matches!(status, "active" | "no_clients" | "stale" | "offline") {
        return CaptureDisposition::Unavailable;
    }
    if unassessed_has_reason(capture, "invalid_delivery_evidence") {
        return CaptureDisposition::Unavailable;
    }
    if capture.get("registry").and_then(Value::as_str) == Some("partial_registry")
        && (assessed_empty_or_all_active(capture) || matches!(status, "stale" | "offline"))
    {
        return CaptureDisposition::Unavailable;
    }
    if matches!(status, "stale" | "offline") {
        return CaptureDisposition::Quiet;
    }
    if status == "no_clients" {
        match capture.get("registry").and_then(Value::as_str) {
            Some("registry_empty" | "no_eligible_records" | "registry_complete") => {
                CaptureDisposition::Calm(calm_kind(capture))
            }
            _ => CaptureDisposition::Unavailable,
        }
    } else {
        CaptureDisposition::Active
    }
}

fn calm_kind(capture: &Value) -> CalmKind {
    match capture.get("registry").and_then(Value::as_str) {
        Some("registry_empty") => CalmKind::EmptyRegistry,
        Some("no_eligible_records") => CalmKind::NoEligible,
        _ => {
            if unassessed_has_reason(capture, "awaiting_first_delivery") {
                CalmKind::AwaitingFirst
            } else {
                CalmKind::Residue
            }
        }
    }
}

fn unassessed_has_reason(capture: &Value, reason: &str) -> bool {
    capture
        .get("unassessed")
        .and_then(Value::as_array)
        .is_some_and(|rows| {
            rows.iter()
                .any(|row| row.get("reason").and_then(Value::as_str) == Some(reason))
        })
}

fn assessed_empty_or_all_active(capture: &Value) -> bool {
    let Some(clients) = capture.get("clients").and_then(Value::as_array) else {
        return true;
    };
    clients.is_empty()
        || clients
            .iter()
            .all(|row| row.get("status").and_then(Value::as_str) == Some("active"))
}

fn unavailable_json() -> Value {
    json!({"verdict":"unavailable","severity":"amber","headline":UNAVAILABLE_HEADLINE,"last_observation":null,"cta":null,"issues":[]})
}

fn calm_json(kind: CalmKind) -> Value {
    match kind {
        CalmKind::EmptyRegistry => json!({
            "verdict": "calm",
            "severity": "neutral",
            "headline": EMPTY_REGISTRY_HEADLINE,
            "last_observation": null,
            "cta": {"text": "set one up →", "href": "/app/network/"},
            "issues": [],
        }),
        CalmKind::AwaitingFirst => json!({
            "verdict": "calm",
            "severity": "neutral",
            "headline": AWAITING_FIRST_HEADLINE,
            "last_observation": null,
            "cta": null,
            "issues": [],
        }),
        CalmKind::Residue => json!({
            "verdict": "calm",
            "severity": "neutral",
            "headline": RESIDUE_HEADLINE,
            "last_observation": null,
            "cta": null,
            "issues": [],
        }),
        CalmKind::NoEligible => json!({
            "verdict": "calm",
            "severity": "neutral",
            "headline": NO_ELIGIBLE_HEADLINE,
            "last_observation": null,
            "cta": null,
            "issues": [],
        }),
    }
}

fn backlog_issues(source: &BacklogSource, now: DateTime<Utc>) -> Vec<Value> {
    if matches!(source.validity, BacklogValidity::NotYet(_)) {
        return Vec::new();
    }
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
    match solstone_core_system_health::summary_freshness(source.generated_at.as_deref(), now) {
        solstone_core_system_health::SummaryFreshness::Fresh => {}
        solstone_core_system_health::SummaryFreshness::Stale => {
            let generated = solstone_core_system_health::parse_summary_time(
                source.generated_at.as_deref().unwrap(),
            )
            .unwrap();
            issues.push(json!({
                "text": format!(
                    "it's unclear whether your journal is caught up; the last update was {} ago.",
                    age(now - generated)
                ),
                "severity": "amber",
                "href": "/app/health"
            }));
        }
        solstone_core_system_health::SummaryFreshness::Unknown => {
            issues.push(json!({
                "text": "it's unclear whether your journal is caught up; the last update age is unknown.",
                "severity": "amber",
                "href": "/app/health"
            }));
        }
    }
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
    json!({"text":solstone_core_system_health::VERDICT_UNCLEAR_NOW,"severity":"amber","href":"/app/health"})
}
fn capture_issue(capture: &Value) -> Option<Value> {
    match capture.get("status").and_then(Value::as_str) {
        Some("degraded") => Some(json!({
            "text": format_degraded_capture_line(capture).expect("degraded"),
            "severity": "red",
            "href": "/app/health",
        })),

        _ => {
            let sources = crate::needs_you::named_attention_sources(capture)?;
            Some(json!({
                "text": format!(
                    "the solstone app on one of your devices is having trouble adding {sources} to your journal."
                ),
                "severity": "amber",
                "href": "/app/health",
            }))
        }
    }
}
/// Health's `describeRegisteredClient` and its `HEALTH_GLANCE_DEVICE*_SILENT`
/// strings are the single source for this sentence; home echoes them, in
/// health's order (the quietest device first). X-03.
fn quiet_capture_summary(capture: &Value, now: DateTime<Utc>) -> String {
    let idle = idle_clients(capture, now);
    match idle.as_slice() {
        [] => UNNAMED_IDLE_ISSUE.to_owned(),
        [(name, Some(age))] => format!("{name} hasn't added to your journal in {age}."),
        [(name, None)] => format!("{name} hasn't added to your journal recently."),
        rows => format!(
            "{} devices haven't added to your journal recently: {}.",
            rows.len(),
            rows.iter()
                .map(|(name, age)| match age {
                    Some(age) => format!("{name} ({age})"),
                    None => name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
// The names the owner sees decide the number the sentence is written in, so the
// devices are collected before the line is composed rather than after.
fn idle_clients(capture: &Value, now: DateTime<Utc>) -> Vec<(String, Option<String>)> {
    let mut rows = capture
        .get("clients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|client| {
            matches!(
                client.get("status").and_then(Value::as_str),
                Some("stale" | "offline")
            )
        })
        .filter_map(|client| {
            let name = client
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())?;
            Some((name.to_owned(), idle_seconds(client, now)))
        })
        .collect::<Vec<_>>();
    // Health lists the quietest device first and puts a device with no known
    // age last; home reads the same order. X-03.
    rows.sort_by(|left, right| {
        right
            .1
            .unwrap_or(-1)
            .cmp(&left.1.unwrap_or(-1))
            .then_with(|| left.0.cmp(&right.0))
    });
    rows.into_iter()
        .map(|(name, seconds)| (name, seconds.map(format_idle_age)))
        .collect()
}
fn idle_seconds(client: &Value, now: DateTime<Utc>) -> Option<i64> {
    let last = client
        .get("last_accepted_ingest_at")
        .and_then(Value::as_str)
        .and_then(parse_time)?;
    let seconds = (now - last).num_seconds();
    (seconds >= 0).then_some(seconds)
}
fn format_idle_age(seconds: i64) -> String {
    if seconds < 45 {
        "just now".to_owned()
    } else {
        relative_time(seconds as f64)
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
    use crate::model::{BacklogSource, BacklogValidity};

    #[test]
    fn future_generated_at_skew_handling() {
        let n = now();
        // 10 minutes in the future -> unknown age issue
        let ten_min_future = n + Duration::minutes(10);
        let backlog_future = BacklogSource {
            backlog: Some(json!({"stuck_days": 0}).as_object().unwrap().clone()),
            validity: BacklogValidity::Valid,
            generated_at: Some(ten_min_future.to_rfc3339()),
        };
        let glance_ten = build_health_glance(
            &json!({"status": "active"}),
            &json!({}),
            Some("29 seconds ago"),
            &backlog_future,
            &Value::Null,
            n,
        );
        let issues = glance_ten["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0]["text"],
            "it's unclear whether your journal is caught up; the last update age is unknown."
        );

        // 2 minutes in the future -> tolerated within 5 min skew, no age issue
        let two_min_future = n + Duration::minutes(2);
        let backlog_two = BacklogSource {
            backlog: Some(json!({"stuck_days": 0}).as_object().unwrap().clone()),
            validity: BacklogValidity::Valid,
            generated_at: Some(two_min_future.to_rfc3339()),
        };
        let glance_two = build_health_glance(
            &json!({"status": "active"}),
            &json!({}),
            Some("29 seconds ago"),
            &backlog_two,
            &Value::Null,
            n,
        );
        let issues_two = glance_two["issues"].as_array().unwrap();
        assert_eq!(issues_two.len(), 0);
    }

    fn fresh_backlog() -> BacklogSource {
        BacklogSource {
            backlog: Some(json!({"stuck_days":0}).as_object().unwrap().clone()),
            validity: BacklogValidity::Valid,
            generated_at: Some("2026-05-14T12:00:00+00:00".to_owned()),
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 14, 15, 30, 0).unwrap()
    }

    fn glance(capture: &Value) -> Value {
        glance_at(capture, None, &Value::Null, now())
    }

    fn glance_at(
        capture: &Value,
        last_observe: Option<&str>,
        brain: &Value,
        at: DateTime<Utc>,
    ) -> Value {
        build_health_glance(
            capture,
            &json!({}),
            last_observe,
            &fresh_backlog(),
            brain,
            at,
        )
    }

    fn unassessed(name: &str, reason: &str, reach: &str) -> Value {
        json!({"name": name, "reason": reason, "reach": reach})
    }

    fn client(name: &str, status: &str, reach: &str) -> Value {
        // `failing` is what home_client_row emits and what both surfaces read,
        // so the fixture carries it rather than restating the rule (F-8).
        json!({"name": name, "status": status, "reach": reach, "failing": status == "degraded"})
    }

    /// Home preserves Health's quiet-device names, ages and ordering as
    /// information, without turning silence into an attention demand.
    #[test]
    fn quiet_device_headline_echoes_health_without_requiring_attention() {
        let mut iphone = client("iPhone's iPhone", "offline", "offline");
        // 15:30 on 2026-05-14 less six hours, and less one day.
        iphone["last_accepted_ingest_at"] = json!("2026-05-14T09:30:00Z");
        let mut suze = client("suze", "offline", "offline");
        suze["last_accepted_ingest_at"] = json!("2026-05-13T15:30:00Z");
        let capture = json!({
            "status": "offline",
            "clients": [iphone, suze],
            "unassessed": [],
            "registry": "registry_complete",
        });
        let glanced = glance(&capture);
        assert_eq!(
            glanced["headline"],
            "2 devices haven't added to your journal recently: suze (1 day), iPhone's iPhone (6 hours)."
        );
        assert_eq!(glanced["verdict"], "calm");
        assert_eq!(glanced["severity"], "neutral");
        assert_eq!(glanced["issues"].as_array().unwrap().len(), 0);

        // A lone quiet device also stays informational.
        let one = json!({
            "status": "offline",
            "clients": [client("suze", "offline", "offline")],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert_eq!(glance(&one)["verdict"], "calm");

        // A device with no known age sorts last, the way health sorts it.
        let mut known = client("known", "stale", "active");
        known["last_accepted_ingest_at"] = json!("2026-05-14T14:30:00Z");
        let unknown_last = json!({
            "status": "stale",
            "clients": [client("unknown", "stale", "active"), known],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert_eq!(
            glance(&unknown_last)["headline"],
            "2 devices haven't added to your journal recently: known (1 hour), unknown."
        );
    }

    #[test]
    fn injected_inputs_produce_active_and_quiet_verdicts() {
        let backlog = fresh_backlog();
        assert_eq!(
            build_health_glance(
                &json!({"status":"active"}),
                &json!({}),
                Some("29 seconds ago"),
                &backlog,
                &Value::Null,
                now()
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
                now()
            )["verdict"],
            "calm"
        );
    }

    #[test]
    fn owner_signal_matrix() {
        let empty = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [],
            "registry": "registry_empty",
        });
        let awaiting = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [unassessed("phone", "awaiting_first_delivery", "active")],
            "registry": "registry_complete",
        });
        let residue_offline = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [unassessed("old", "registration_residue", "offline")],
            "registry": "registry_complete",
        });
        let residue_stale = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [unassessed("old", "registration_residue", "stale")],
            "registry": "registry_complete",
        });
        let no_eligible = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [],
            "registry": "no_eligible_records",
        });

        let g = glance(&empty);
        assert_eq!(g["verdict"], "calm");
        assert_eq!(g["severity"], "neutral");
        assert_eq!(g["cta"]["href"], "/app/network/");
        assert_eq!(g["issues"].as_array().unwrap().len(), 0);
        assert!(g["headline"].as_str().unwrap().contains("set one up"));

        for capture in [&awaiting, &residue_offline, &residue_stale, &no_eligible] {
            let g = glance(capture);
            assert_eq!(g["verdict"], "calm");
            assert_eq!(g["severity"], "neutral");
            assert!(g["cta"].is_null());
            assert_eq!(g["issues"].as_array().unwrap().len(), 0);
        }
        let awaiting_g = glance(&awaiting);
        assert!(
            awaiting_g["headline"]
                .as_str()
                .unwrap()
                .contains("hasn't added")
        );
        assert!(
            !awaiting_g["headline"]
                .as_str()
                .unwrap()
                .contains("no devices are running")
        );
        assert!(
            !glance(&residue_offline)["headline"]
                .as_str()
                .unwrap()
                .contains("device")
        );
        assert!(
            !glance(&residue_stale)["headline"]
                .as_str()
                .unwrap()
                .contains("device")
        );
        assert!(
            glance(&no_eligible)["headline"]
                .as_str()
                .unwrap()
                .contains("solstone app")
        );

        let invalid_active = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [unassessed("bad", "invalid_delivery_evidence", "active")],
            "registry": "registry_complete",
        });
        let invalid_offline = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [unassessed("bad", "invalid_delivery_evidence", "offline")],
            "registry": "registry_complete",
        });
        let unknown = json!({
            "status": "unknown",
            "clients": [],
            "unassessed": [],
            "registry": "registry_unknown",
        });
        let partial_empty = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [],
            "registry": "partial_registry",
        });
        let partial_all_active = json!({
            "status": "active",
            "clients": [client("peer", "active", "active")],
            "unassessed": [],
            "registry": "partial_registry",
        });
        let invalid_beside_active = json!({
            "status": "active",
            "clients": [client("peer", "active", "active")],
            "unassessed": [unassessed("bad", "invalid_delivery_evidence", "active")],
            "registry": "registry_complete",
        });
        let missing_registry = json!({
            "status": "no_clients",
            "clients": [],
            "unassessed": [],
        });

        let invalid_g = glance(&invalid_active);
        assert_eq!(invalid_g["verdict"], "unavailable");
        assert_eq!(invalid_g["severity"], "amber");
        assert!(invalid_g["cta"].is_null());
        assert_eq!(invalid_g["issues"].as_array().unwrap().len(), 0);
        assert!(
            invalid_g["headline"]
                .as_str()
                .unwrap()
                .contains("status is unclear")
        );
        assert_eq!(glance(&invalid_offline)["verdict"], invalid_g["verdict"]);
        assert_eq!(glance(&invalid_offline)["severity"], invalid_g["severity"]);
        assert_eq!(glance(&invalid_offline)["headline"], invalid_g["headline"]);
        assert_eq!(glance(&invalid_offline)["cta"], invalid_g["cta"]);
        assert_eq!(glance(&invalid_offline)["issues"], invalid_g["issues"]);
        for capture in [
            &unknown,
            &partial_empty,
            &partial_all_active,
            &invalid_beside_active,
            &missing_registry,
        ] {
            let g = glance(capture);
            assert_eq!(g["verdict"], "unavailable", "{capture}");
            assert_eq!(g["severity"], "amber");
            assert!(g["last_observation"].is_null());
        }

        // Two offline devices in one row previously read as one device called
        // "iPhone's iPhone, suze", followed by singular pronouns, and the line
        // asserted a reachability failure the journal never measured.
        let two_offline = json!({
            "status": "offline",
            "clients": [
                client("iPhone's iPhone", "offline", "offline"),
                client("suze", "offline", "offline"),
            ],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert_eq!(
            glance(&two_offline)["headline"],
            "2 devices haven't added to your journal recently: iPhone's iPhone, suze."
        );
        assert_eq!(glance(&two_offline)["severity"], "neutral");
        let three_stale_running = json!({
            "status": "stale",
            "clients": [
                client("desk", "stale", "active"),
                client("laptop", "stale", "active"),
                client("suze", "stale", "active"),
            ],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert_eq!(
            glance(&three_stale_running)["headline"],
            "3 devices haven't added to your journal recently: desk, laptop, suze."
        );
        // One device is named on its own, with the age of the last thing it
        // added when the record carries one.
        let one_offline = json!({
            "status": "offline",
            "clients": [client("suze", "offline", "offline")],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert_eq!(
            glance(&one_offline)["headline"],
            "suze hasn't added to your journal recently."
        );
        let mut one_offline_timed = one_offline.clone();
        one_offline_timed["clients"][0]["last_accepted_ingest_at"] = json!("2026-05-13T15:30:00Z");
        let timed = glance(&one_offline_timed);
        assert_eq!(
            timed["headline"],
            "suze hasn't added to your journal in 1 day."
        );
        assert_eq!(timed["severity"], "neutral");
        assert_eq!(timed["cta"]["href"], "/app/health/#registeredClientsCard");

        let stale_with_invalid = json!({
            "status": "stale",
            "clients": [client("phone", "stale", "offline")],
            "unassessed": [unassessed("bad", "invalid_delivery_evidence", "active")],
            "registry": "registry_complete",
        });
        let stale_g = glance(&stale_with_invalid);
        assert_eq!(stale_g["verdict"], "unavailable");
        assert_eq!(stale_g["severity"], "amber");
        assert!(stale_g["issues"].as_array().unwrap().is_empty());

        let offline_partial = json!({
            "status": "offline",
            "clients": [client("phone", "offline", "offline")],
            "unassessed": [],
            "registry": "partial_registry",
        });
        let offline_g = glance(&offline_partial);
        assert_eq!(offline_g["verdict"], "unavailable");
        assert_eq!(offline_g["severity"], "amber");
        assert!(offline_g["issues"].as_array().unwrap().is_empty());

        let degraded_invalid = json!({
            "status": "degraded",
            "clients": [client("rej", "degraded", "active")],
            "unassessed": [unassessed("bad", "invalid_delivery_evidence", "active")],
            "registry": "registry_complete",
        });
        let degraded_g = glance(&degraded_invalid);
        assert_eq!(degraded_g["verdict"], "attention");
        assert_eq!(degraded_g["severity"], "red");
        let degraded_text = degraded_g["issues"][0]["text"].as_str().unwrap();
        // X-02: the red device line names the device, the way health's banner
        // does. "one of your devices" was the only one of the three writers
        // that hid it.
        assert_eq!(degraded_text, "rej isn't reaching your journal.");

        let missing_backlog = BacklogSource {
            backlog: None,
            validity: BacklogValidity::Missing,
            generated_at: None,
        };
        let backlog_over_invalid = build_health_glance(
            &invalid_active,
            &json!({}),
            None,
            &missing_backlog,
            &Value::Null,
            now(),
        );
        assert_eq!(backlog_over_invalid["verdict"], "attention");
        assert!(
            backlog_over_invalid["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["text"]
                    .as_str()
                    .unwrap()
                    .contains("it's unclear whether your journal is caught up"))
        );

        let active_awaiting = json!({
            "status": "active",
            "clients": [client("phone", "active", "active")],
            "unassessed": [unassessed("new", "awaiting_first_delivery", "active")],
            "registry": "registry_complete",
        });
        let active_residue = json!({
            "status": "active",
            "clients": [client("phone", "active", "active")],
            "unassessed": [unassessed("old", "registration_residue", "offline")],
            "registry": "registry_complete",
        });
        let active_g = glance_at(
            &active_awaiting,
            Some("29 seconds ago"),
            &Value::Null,
            now(),
        );
        assert_eq!(active_g["verdict"], "ok");
        assert_eq!(active_g["severity"], "green");
        assert_eq!(active_g["last_observation"], "29 seconds ago");
        assert!(active_g["cta"].is_null());
        let residue_active =
            glance_at(&active_residue, Some("29 seconds ago"), &Value::Null, now());
        assert_eq!(residue_active["verdict"], "ok");
        assert_eq!(residue_active["severity"], "green");

        let mut running = json!({
            "status": "stale",
            "clients": [client("phone", "stale", "active")],
            "unassessed": [],
            "registry": "registry_complete",
        });
        let mut asleep = running.clone();
        asleep["clients"][0]["reach"] = json!("offline");
        let running_g = glance(&running);
        let asleep_g = glance(&asleep);
        assert_eq!(running_g["verdict"], asleep_g["verdict"]);
        assert_eq!(running_g["severity"], asleep_g["severity"]);
        assert_eq!(running_g["headline"], asleep_g["headline"]);
        assert_eq!(
            running_g["issues"].as_array().unwrap().len(),
            asleep_g["issues"].as_array().unwrap().len()
        );
        assert_eq!(running_g["cta"]["href"], asleep_g["cta"]["href"]);
        assert_eq!(running_g["verdict"], "calm");
        assert_eq!(running_g["severity"], "neutral");
        assert!(running_g["issues"].as_array().unwrap().is_empty());
        // Reach is a heartbeat window, not delivery. It no longer changes a
        // word of the line either way.
        let running_text = running_g["headline"].as_str().unwrap();
        let asleep_text = asleep_g["headline"].as_str().unwrap();
        assert_eq!(running_text, asleep_text);
        assert_eq!(running_text, "phone hasn't added to your journal recently.");

        running["clients"][0]["reach"] = json!("stale");
        assert_eq!(glance(&running)["headline"], running_text);

        let mixed = json!({
            "status": "stale",
            "clients": [
                client("alpha", "stale", "active"),
                client("bravo", "stale", "offline"),
            ],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert_eq!(
            glance(&mixed)["headline"],
            "2 devices haven't added to your journal recently: alpha, bravo."
        );

        let corpus_shaped = json!({"status": "stale", "clients": [{"name": "laptop"}]});
        assert_eq!(glance(&corpus_shaped)["headline"], UNNAMED_IDLE_ISSUE);
        assert_eq!(
            glance(&corpus_shaped)["cta"]["href"],
            "/app/health/#registeredClientsCard"
        );

        let checking = glance_at(
            &empty,
            None,
            &json!({"state": "checking", "headline": "checking thinking"}),
            now(),
        );
        assert_eq!(checking["verdict"], "checking");
        assert_eq!(checking["severity"], "amber");

        let progressing = glance_at(
            &empty,
            None,
            &json!({"state": "blocked", "headline": "installing", "progressing": true}),
            now(),
        );
        assert_eq!(progressing["verdict"], "progressing");
        assert_eq!(progressing["severity"], "amber");

        let later = now() + Duration::days(30);
        let at_clock = |at: DateTime<Utc>| {
            let backlog = BacklogSource {
                backlog: Some(json!({"stuck_days":0}).as_object().unwrap().clone()),
                validity: BacklogValidity::Valid,
                generated_at: Some(at.to_rfc3339()),
            };
            build_health_glance(&awaiting, &json!({}), None, &backlog, &Value::Null, at)
        };
        let first = at_clock(now());
        let second = at_clock(later);
        assert_eq!(first["verdict"], "calm", "{first}");
        assert_eq!(second["verdict"], "calm", "{second}");
        assert_eq!(first["severity"], "neutral");
        assert_eq!(second["severity"], "neutral");
        assert!(first["cta"].is_null());
        assert!(second["cta"].is_null());
    }

    #[test]
    fn active_rollup_still_surfaces_a_named_source_that_needs_attention() {
        let capture = json!({
            "status": "active",
            "clients": [{
                "name": "phone",
                "status": "active",
                "reach": "active",
                "source_delivery": {
                    "audio": {"state": "current", "elapsed_ms": 1000},
                    "location": {"state": "needs_attention", "elapsed_ms": 700000}
                }
            }],
            "unassessed": [],
            "registry": "registry_complete",
        });
        let g = glance(&capture);
        assert_eq!(g["verdict"], "attention");
        assert_eq!(g["severity"], "amber");
        assert_eq!(
            g["issues"][0]["text"],
            "the solstone app on one of your devices is having trouble adding location to your journal."
        );
    }

    #[test]
    fn single_source_active_rollup_does_not_invent_a_source_issue() {
        for source_delivery in [
            json!({"audio": {"state": "needs_attention"}}),
            json!({"": {"state": "needs_attention"}}),
        ] {
            let capture = json!({
                "status": "active",
                "clients": [{
                    "name": "phone",
                    "status": "active",
                    "reach": "active",
                    "source_delivery": source_delivery
                }],
                "unassessed": [],
                "registry": "registry_complete",
            });
            let g = glance(&capture);
            assert_eq!(g["verdict"], "ok", "{capture}");
            assert_eq!(g["issues"].as_array().unwrap().len(), 0);
        }
    }

    #[test]
    fn empty_source_is_named_default_on_a_multi_source_active_rollup() {
        let capture = json!({
            "status": "active",
            "clients": [{
                "name": "phone",
                "status": "active",
                "reach": "active",
                "source_delivery": {
                    "audio": {"state": "current", "elapsed_ms": 1000},
                    "": {"state": "needs_attention", "elapsed_ms": 700000}
                }
            }],
            "unassessed": [],
            "registry": "registry_complete",
        });
        let g = glance(&capture);
        assert_eq!(g["verdict"], "attention");
        assert_eq!(
            g["issues"][0]["text"],
            "the solstone app on one of your devices is having trouble adding default to your journal."
        );
    }

    fn not_yet(kind: solstone_core_system_health::NotYet) -> BacklogSource {
        BacklogSource {
            backlog: None,
            validity: BacklogValidity::NotYet(kind),
            generated_at: None,
        }
    }

    fn not_yet_glance(capture: &Value, brain: &Value, backlog: &BacklogSource) -> Value {
        build_health_glance(
            capture,
            &json!({}),
            Some("2 minutes ago"),
            backlog,
            brain,
            now(),
        )
    }

    /// Day 0: the missing summary is a note on every verdict, never an issue,
    /// and never lets a delivering device turn home green.
    #[test]
    fn first_night_is_a_calm_note_never_a_count_and_never_green() {
        use solstone_core_system_health::{NOT_YET_ENGINE, NOT_YET_FIRST_NIGHT, NotYet};
        let note = |glance: &Value| glance["note"]["text"].as_str().map(str::to_owned);

        // A device already delivering on day 0: calm, not ok.
        let active = json!({"status": "active", "clients": [client("phone", "active", "active")], "registry": "registry_complete"});
        let delivering = not_yet_glance(&active, &Value::Null, &not_yet(NotYet::FirstNight));
        assert_eq!(delivering["verdict"], "calm");
        assert_eq!(delivering["severity"], "neutral");
        assert_eq!(delivering["headline"], NOT_YET_ACTIVE_HEADLINE);
        assert_eq!(delivering["last_observation"], "2 minutes ago");
        assert!(delivering["issues"].as_array().unwrap().is_empty());
        assert_eq!(note(&delivering).as_deref(), Some(NOT_YET_FIRST_NIGHT));
        assert_eq!(delivering["note"]["href"], "/app/health/#backlogVerdict");

        // No way to think yet: the one real ask is the only count.
        let empty = json!({"status": "no_clients", "registry": "registry_empty"});
        let blocked = json!({"state": "blocked", "headline": "processing needs a setup"});
        let no_engine = not_yet_glance(&empty, &blocked, &not_yet(NotYet::AwaitingEngine));
        assert_eq!(no_engine["verdict"], "attention");
        assert_eq!(no_engine["headline"], "1 thing needs your attention");
        assert_eq!(no_engine["issues"][0]["text"], "processing needs a setup");
        assert_eq!(note(&no_engine).as_deref(), Some(NOT_YET_ENGINE));
        assert_eq!(no_engine["note"]["href"], "/app/thinking/");

        // No devices, thinking set up: the first-run invite comes back.
        let first_run = not_yet_glance(&empty, &Value::Null, &not_yet(NotYet::FirstNight));
        assert_eq!(first_run["verdict"], "calm");
        assert_eq!(first_run["headline"], EMPTY_REGISTRY_HEADLINE);
        assert_eq!(first_run["cta"]["text"], "set one up →");
        assert_eq!(note(&first_run).as_deref(), Some(NOT_YET_FIRST_NIGHT));

        // A summary that is missing for any other reason keeps today's amber.
        let missing = BacklogSource {
            backlog: None,
            validity: BacklogValidity::Missing,
            generated_at: None,
        };
        let amber = not_yet_glance(&active, &Value::Null, &missing);
        assert_eq!(amber["verdict"], "attention");
        assert_eq!(
            amber["issues"][0]["text"],
            solstone_core_system_health::VERDICT_UNCLEAR_NOW
        );
        assert!(amber.get("note").is_none());

        // A healthy summary: no note, and green is earned.
        let green = glance_at(&active, Some("2 minutes ago"), &Value::Null, now());
        assert_eq!(green["verdict"], "ok");
        assert!(green.get("note").is_none());
    }
}
