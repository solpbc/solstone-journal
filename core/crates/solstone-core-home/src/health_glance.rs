// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure health-glance projection.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::model::{BacklogSource, BacklogValidity};
use crate::needs_you::format_degraded_capture_line;

pub const BACKLOG_FRESHNESS_MAX_AGE_HOURS: i64 = 36;

const UNAVAILABLE_HEADLINE: &str = "i don't know the status of your devices right now.";
const EMPTY_REGISTRY_HEADLINE: &str =
    "no devices are running the solstone app yet. set one up to start your journal.";
const AWAITING_FIRST_HEADLINE: &str =
    "the solstone app on one of your devices hasn't added anything to your journal yet.";
const RESIDUE_HEADLINE: &str = "nothing needs your attention right now.";
const NO_ELIGIBLE_HEADLINE: &str =
    "none of your devices have the solstone app set up to add to your journal right now.";
const RUNNING_REACH_SENTENCE: &str =
    "the app is still running, but it isn't adding to your journal.";
const ASLEEP_REACH_SENTENCE: &str = "the device appears offline and may just be asleep.";
const STALE_ISSUE: &str =
    "the solstone app on one of your devices has not added anything to your journal recently.";
const OFFLINE_ISSUE: &str = "the solstone app hasn't added anything to your journal recently.";

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
    let status = client_state(capture);
    if status != "active" && status != "no_clients" {
        return CaptureDisposition::Unavailable;
    }
    if unassessed_has_reason(capture, "invalid_delivery_evidence") {
        return CaptureDisposition::Unavailable;
    }
    if capture.get("registry").and_then(Value::as_str) == Some("partial_registry")
        && assessed_empty_or_all_active(capture)
    {
        return CaptureDisposition::Unavailable;
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

fn affected_reach_sentence(capture: &Value) -> Option<&'static str> {
    let clients = capture.get("clients").and_then(Value::as_array)?;
    let affected: Vec<&Value> = clients
        .iter()
        .filter(|row| {
            matches!(
                row.get("status").and_then(Value::as_str),
                Some("stale" | "offline")
            )
        })
        .collect();
    if affected.is_empty() {
        return None;
    }
    if affected.iter().any(|row| {
        matches!(
            row.get("reach").and_then(Value::as_str),
            Some("active" | "stale")
        )
    }) {
        Some(RUNNING_REACH_SENTENCE)
    } else {
        Some(ASLEEP_REACH_SENTENCE)
    }
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
    let (base, severity) = match capture.get("status").and_then(Value::as_str) {
        Some("degraded") => (
            format_degraded_capture_line(capture).expect("degraded"),
            "red",
        ),
        Some("offline") => (OFFLINE_ISSUE.to_owned(), "red"),
        Some("stale") => (STALE_ISSUE.to_owned(), "amber"),
        _ => return None,
    };
    let text = match capture.get("status").and_then(Value::as_str) {
        Some("stale" | "offline") => match affected_reach_sentence(capture) {
            Some(sentence) => format!("{base} {sentence}"),
            None => base,
        },
        _ => base,
    };
    Some(json!({"text":text,"severity":severity,"href":"/app/health"}))
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
        json!({"name": name, "status": status, "reach": reach})
    }

    #[test]
    fn injected_inputs_produce_active_and_unavailable_verdicts() {
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
            "attention"
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
                .contains("i don't know the status")
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

        let stale_with_invalid = json!({
            "status": "stale",
            "clients": [client("phone", "stale", "offline")],
            "unassessed": [unassessed("bad", "invalid_delivery_evidence", "active")],
            "registry": "registry_complete",
        });
        let stale_g = glance(&stale_with_invalid);
        assert_eq!(stale_g["verdict"], "attention");
        assert_eq!(stale_g["severity"], "amber");
        assert_eq!(stale_g["issues"].as_array().unwrap().len(), 1);
        assert_eq!(stale_g["issues"][0]["href"], "/app/health");
        assert!(
            stale_g["issues"][0]["text"]
                .as_str()
                .unwrap()
                .contains("has not added anything to your journal recently")
        );

        let offline_partial = json!({
            "status": "offline",
            "clients": [client("phone", "offline", "offline")],
            "unassessed": [],
            "registry": "partial_registry",
        });
        let offline_g = glance(&offline_partial);
        assert_eq!(offline_g["verdict"], "attention");
        assert_eq!(offline_g["severity"], "red");
        assert_eq!(offline_g["issues"].as_array().unwrap().len(), 1);
        assert!(
            offline_g["issues"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with(OFFLINE_ISSUE)
        );

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
        assert!(degraded_text.contains("having trouble adding"));
        assert!(!degraded_text.contains("still running"));
        assert!(!degraded_text.contains("asleep"));

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
                    .contains("i can't tell if your journal is caught up"))
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
        assert_eq!(
            running_g["issues"][0]["href"],
            asleep_g["issues"][0]["href"]
        );
        assert_eq!(running_g["verdict"], "attention");
        assert_eq!(running_g["severity"], "amber");
        assert_eq!(running_g["headline"], "1 thing needs your attention");
        let running_text = running_g["issues"][0]["text"].as_str().unwrap();
        let asleep_text = asleep_g["issues"][0]["text"].as_str().unwrap();
        assert!(running_text.contains("still running"));
        assert!(running_text.contains("isn't adding"));
        assert!(asleep_text.contains("asleep"));
        assert!(!running_text.contains("reach"));
        assert!(!asleep_text.contains("contact"));
        assert!(!running_text.contains("heartbeat"));

        running["clients"][0]["reach"] = json!("stale");
        let stale_reach_g = glance(&running);
        assert!(
            stale_reach_g["issues"][0]["text"]
                .as_str()
                .unwrap()
                .contains("still running")
        );

        let mixed = json!({
            "status": "stale",
            "clients": [
                client("alpha", "stale", "active"),
                client("bravo", "stale", "offline"),
            ],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert!(
            glance(&mixed)["issues"][0]["text"]
                .as_str()
                .unwrap()
                .contains("still running")
        );

        let corpus_shaped = json!({"status": "stale", "clients": [{"name": "laptop"}]});
        assert_eq!(glance(&corpus_shaped)["issues"][0]["text"], STALE_ISSUE);

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
}
