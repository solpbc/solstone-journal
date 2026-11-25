// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure health-glance projection.

use jiff::Timestamp;
use serde_json::{Value, json};

use crate::formatting::{cardinal_quantity, render_count};
use crate::model::{BacklogSource, BacklogValidity};
use crate::needs_you::format_degraded_capture_line_in;
use solstone_core_system_health::NotYet;
use solstone_core_window_catalog::{ENGLISH, Value as CatalogValue, render, strings, text};

const UNAVAILABLE_HEADLINE_ID: &str = "journal.window.home.health.unavailable.headline";
const EMPTY_REGISTRY_HEADLINE_ID: &str = "journal.window.home.status.headline.ready.empty";
const QUIET_HEADLINE_ID: &str = "journal.window.home.status.headline.quiet";
const AWAITING_FIRST_HEADLINE_ID: &str = "journal.window.home.health.awaiting.first.headline";
const RESIDUE_HEADLINE_ID: &str = "journal.window.home.health.residue.headline";
const NO_ELIGIBLE_HEADLINE_ID: &str = "journal.window.home.health.no.eligible.headline";
// Day 0 with a device already delivering: things are arriving, and the summary
// that could prove the rest hasn't had its first chance yet. Calm, never green.
const NOT_YET_ACTIVE_HEADLINE_ID: &str = "journal.window.home.health.not.yet.active.headline";

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
    last_observe: Option<&str>,
    backlog: &BacklogSource,
    brain: &Value,
    now: Timestamp,
    transcription_issue: Option<&Value>,
) -> Value {
    build_health_glance_in(
        capture,
        last_observe,
        backlog,
        brain,
        now,
        transcription_issue,
        ENGLISH,
    )
}

pub fn build_health_glance_in(
    capture: &Value,
    last_observe: Option<&str>,
    backlog: &BacklogSource,
    brain: &Value,
    now: Timestamp,
    transcription_issue: Option<&Value>,
    locale: &str,
) -> Value {
    build_health_glance_with_index_in(
        capture,
        last_observe,
        backlog,
        brain,
        now,
        transcription_issue,
        None,
        locale,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_health_glance_with_index_in(
    capture: &Value,
    last_observe: Option<&str>,
    backlog: &BacklogSource,
    brain: &Value,
    now: Timestamp,
    transcription_issue: Option<&Value>,
    search_index_issue: Option<&Value>,
    locale: &str,
) -> Value {
    let mut glance = derive_glance(
        capture,
        last_observe,
        backlog,
        brain,
        now,
        transcription_issue,
        search_index_issue,
        locale,
    );
    let not_yet = match backlog.validity {
        BacklogValidity::NotYet(not_yet) => Some(not_yet),
        // No way to think chosen is a choice still ahead, never a fault: it
        // rides as the note even when a summary from earlier exists.
        _ if engine_not_chosen(brain) => Some(NotYet::AwaitingEngine(backlog.saved_lane)),
        _ => None,
    };
    if let Some(not_yet) = not_yet {
        // Not counted, never hidden: the note rides every verdict. Green is
        // earned by a summary, so an `ok` the note sits beside becomes calm.
        if glance["verdict"] == "ok" {
            glance["verdict"] = json!("calm");
            glance["severity"] = json!("neutral");
            glance["headline"] = json!(text(locale, NOT_YET_ACTIVE_HEADLINE_ID));
        }
        glance["note"] = json!({"text": not_yet.text_in(locale), "href": not_yet.href()});
    }
    glance
}

#[allow(clippy::too_many_arguments)]
fn derive_glance(
    capture: &Value,
    last_observe: Option<&str>,
    backlog: &BacklogSource,
    brain: &Value,
    now: Timestamp,
    transcription_issue: Option<&Value>,
    search_index_issue: Option<&Value>,
    locale: &str,
) -> Value {
    let mut issues = backlog_issues(backlog, now, locale);
    if let Some(issue) = capture_issue(capture, locale) {
        issues.push(issue);
    }
    if let Some(issue) = brain_issue(brain) {
        issues.push(issue);
    } else if let Some(issue) = transcription_issue {
        issues.push(issue.clone());
    }
    if let Some(issue) = search_index_issue {
        issues.push(issue.clone());
    }
    if !issues.is_empty() {
        let severity = if issues.iter().any(|issue| issue["severity"] == "red") {
            "red"
        } else {
            "amber"
        };
        let count = issues.len();
        let headline = render_count(
            locale,
            "journal.window.home.health.attention.count",
            count as i64,
        );
        return json!({"verdict":"attention","severity":severity,"headline":headline,"last_observation":null,"cta":null,"issues":issues});
    }
    let disposition = capture_disposition(capture);
    if matches!(disposition, CaptureDisposition::Unavailable) {
        return unavailable_json(locale);
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
            json!({"verdict":"calm","severity":"neutral","headline":text(locale, QUIET_HEADLINE_ID),"last_observation":last_observe,"cta":null,"issues":[]})
        }
        CaptureDisposition::Active => {
            json!({"verdict":"ok","severity":"green","headline":text(locale, "journal.window.home.health.ready"),"last_observation":last_observe,"cta":null,"issues":[]})
        }
        CaptureDisposition::Calm(kind) => calm_json(kind, locale),
        CaptureDisposition::Unavailable => unavailable_json(locale),
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

/// The brain reason for a journal with no way to think chosen yet.
fn engine_not_chosen(brain: &Value) -> bool {
    brain.get("reason_code").and_then(Value::as_str) == Some("thinking_engine_not_chosen")
}

fn unavailable_json(locale: &str) -> Value {
    json!({"verdict":"unavailable","severity":"amber","headline":text(locale, UNAVAILABLE_HEADLINE_ID),"last_observation":null,"cta":null,"issues":[]})
}

fn calm_json(kind: CalmKind, locale: &str) -> Value {
    match kind {
        CalmKind::EmptyRegistry => json!({
            "verdict": "calm",
            "severity": "neutral",
            "headline": text(locale, EMPTY_REGISTRY_HEADLINE_ID),
            "last_observation": null,
            "cta": null,
            "issues": [],
        }),
        CalmKind::AwaitingFirst => json!({
            "verdict": "calm",
            "severity": "neutral",
            "headline": text(locale, AWAITING_FIRST_HEADLINE_ID),
            "last_observation": null,
            "cta": null,
            "issues": [],
        }),
        CalmKind::Residue => json!({
            "verdict": "calm",
            "severity": "neutral",
            "headline": text(locale, RESIDUE_HEADLINE_ID),
            "last_observation": null,
            "cta": null,
            "issues": [],
        }),
        CalmKind::NoEligible => json!({
            "verdict": "calm",
            "severity": "neutral",
            "headline": text(locale, NO_ELIGIBLE_HEADLINE_ID),
            "last_observation": null,
            "cta": null,
            "issues": [],
        }),
    }
}

fn backlog_issues(source: &BacklogSource, now: Timestamp, locale: &str) -> Vec<Value> {
    if matches!(source.validity, BacklogValidity::NotYet(_)) {
        return Vec::new();
    }
    if source.validity != BacklogValidity::Valid {
        return vec![unknown_backlog(locale)];
    }
    let Some(backlog) = &source.backlog else {
        return vec![unknown_backlog(locale)];
    };
    let mut issues = Vec::new();
    if backlog.get("degraded").and_then(Value::as_bool) == Some(true) {
        issues.push(unknown_backlog(locale));
    }
    match solstone_core_system_health::summary_freshness(source.generated_at.as_deref(), now) {
        solstone_core_system_health::SummaryFreshness::Fresh => {}
        solstone_core_system_health::SummaryFreshness::Stale => {
            let generated = solstone_core_system_health::parse_summary_time(
                source.generated_at.as_deref().unwrap(),
            )
            .unwrap();
            issues.push(json!({
                "text": render_count(locale, "journal.window.system.health.backlog.verdict.stale.hour", (now - generated).as_secs().max(0) / 3600),
                "severity": "amber",
                "href": "/app/health"
            }));
        }
        solstone_core_system_health::SummaryFreshness::Unknown => {
            issues.push(json!({
                "text": text(locale, "journal.window.system.health.backlog.verdict.age.unknown"),
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
            .map(str::to_owned)
            .unwrap_or_else(|| text(locale, "journal.window.home.health.backlog.day.needs.help"));
        issues.push(json!({"text":text,"severity":"red","href":"/app/health"}));
    }
    issues
}
fn unknown_backlog(locale: &str) -> Value {
    json!({"text":solstone_core_system_health::backlog_unavailable_in(locale),"severity":"amber","href":"/app/health"})
}
fn rejected_sources(client: &Value, locale: &str) -> Vec<String> {
    client
        .get("source_delivery")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, row)| row.get("state").and_then(Value::as_str) == Some("needs_attention"))
        .map(|(source, _)| crate::needs_you::source_display_name(source, locale))
        .collect()
}

fn capture_issue(capture: &Value, locale: &str) -> Option<Value> {
    // The same authority as Health: an explicit source refusal survives a
    // sibling source succeeding. Count devices, not their source rows.
    let mut affected = capture
        .get("clients")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|client| client["failing"] == true || !rejected_sources(client, locale).is_empty())
        .collect::<Vec<_>>();
    if affected.is_empty() {
        return format_degraded_capture_line_in(capture, locale).map(|text| {
            json!({
                "text": text, "severity": "red", "href": "/app/health"
            })
        });
    }
    affected.sort_by_key(|client| {
        std::cmp::Reverse(
            client
                .get("capture_elapsed_ms")
                .and_then(Value::as_i64)
                .unwrap_or(-1),
        )
    });
    let labels = affected
        .iter()
        .map(|client| {
            let name = client
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| text(locale, "journal.window.health.status.an.unnamed.device"));
            let sources = rejected_sources(client, locale);
            if sources.is_empty() {
                name.to_owned()
            } else {
                strings(
                    locale,
                    "journal.window.home.health.device.sources",
                    [("device", &name), ("sources", &sources.join(", "))],
                )
            }
        })
        .collect::<Vec<_>>();
    let count = affected.len() as i64;
    let text = render(
        locale,
        "journal.window.home.health.upload.devices",
        &[
            ("count", CatalogValue::String(&count.to_string())),
            ("devices", CatalogValue::String(&labels.join(", "))),
            ("quantity", cardinal_quantity(count)),
        ],
    );
    let severity = if affected.iter().any(|client| client["failing"] == true) {
        "red"
    } else {
        "amber"
    };
    Some(json!({"text":text, "severity":severity, "href":"/app/health/#registeredClientsCard"}))
}

fn brain_issue(brain: &Value) -> Option<Value> {
    let state = brain.get("state").and_then(Value::as_str)?;
    if engine_not_chosen(brain)
        || state == "ready"
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

pub fn search_index_issue(context: &crate::HomeContext) -> Option<Value> {
    let observations = solstone_core_system_health::read_indexing_observations(
        context.journal_root(),
        context.now_utc,
    );
    let index = solstone_core_system_health::evaluate_index_health_recent_in(
        context.journal_root(),
        &observations,
        context.locale(),
    );
    if index.state != solstone_core_system_health::IndexHealthState::Failing {
        return None;
    }
    let label = if index.text.trim().is_empty() {
        text(
            context.locale(),
            "journal.window.home.status.search.attention",
        )
    } else {
        index.text
    };
    Some(json!({
        "text": label,
        "severity": "amber",
        "href": "/app/health/#backlogVerdict",
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusBadgeSummary {
    pub issues: usize,
    pub severity: Option<&'static str>,
    pub uncertain: bool,
}

pub fn status_badge_summary(root: &std::path::Path) -> StatusBadgeSummary {
    let context = crate::HomeContext::new(root, Timestamp::now());
    status_badge_summary_at(&context)
}

pub fn status_badge_summary_at(context: &crate::HomeContext) -> StatusBadgeSummary {
    let capture_health = crate::readers::get_capture_health(context);
    let last_observe = crate::readers::last_observe_relative_seconds(context)
        .map(|seconds| crate::formatting::relative_time_ago(seconds as f64, context.locale()));
    let brain = crate::readers::build_brain_snapshot(context);
    let transcription_issue = solstone_core_thinking::read_config(context.journal_root())
        .ok()
        .and_then(|config| {
            crate::readers::transcription_processing_issue(
                context.journal_root(),
                &config,
                brain.pointer("/identity/lane").and_then(Value::as_str),
                context.locale(),
            )
        });
    let index_issue = search_index_issue(context);
    let backlog = crate::readers::load_backlog_source(context);
    let glance = build_health_glance_with_index_in(
        &capture_health,
        last_observe.as_deref(),
        &backlog,
        &brain,
        context.now_utc,
        transcription_issue.as_ref(),
        index_issue.as_ref(),
        context.locale(),
    );
    let issues_arr = glance.get("issues").and_then(Value::as_array);
    let issues = issues_arr.map_or(0, Vec::len);
    let severity = if issues == 0 {
        None
    } else if issues_arr.is_some_and(|rows| rows.iter().any(|issue| issue["severity"] == "red")) {
        Some("red")
    } else {
        Some("amber")
    };
    let uncertain = glance.get("verdict").and_then(Value::as_str) == Some("unavailable");
    StatusBadgeSummary {
        issues,
        severity,
        uncertain,
    }
}

#[cfg(test)]
mod tests {
    use jiff::SignedDuration;
    use serde_json::json;

    use super::*;
    use crate::model::{BacklogSource, BacklogValidity};

    #[test]
    fn future_generated_at_skew_handling() {
        let n = now();
        // 10 minutes in the future -> unknown age issue
        let ten_min_future = n + SignedDuration::from_mins(10);
        let backlog_future = BacklogSource {
            backlog: Some(json!({"stuck_days": 0}).as_object().unwrap().clone()),
            validity: BacklogValidity::Valid,
            generated_at: Some(solstone_core_time::utc_rfc3339(ten_min_future)),
            saved_lane: None,
        };
        let glance_ten = build_health_glance(
            &json!({"status": "active"}),
            Some("29 seconds ago"),
            &backlog_future,
            &Value::Null,
            n,
            None,
        );
        let issues = glance_ten["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0]["text"],
            "without a reliable time for its last status update, your journal can't tell whether it's caught up."
        );

        // 2 minutes in the future -> tolerated within 5 min skew, no age issue
        let two_min_future = n + SignedDuration::from_mins(2);
        let backlog_two = BacklogSource {
            backlog: Some(json!({"stuck_days": 0}).as_object().unwrap().clone()),
            validity: BacklogValidity::Valid,
            generated_at: Some(solstone_core_time::utc_rfc3339(two_min_future)),
            saved_lane: None,
        };
        let glance_two = build_health_glance(
            &json!({"status": "active"}),
            Some("29 seconds ago"),
            &backlog_two,
            &Value::Null,
            n,
            None,
        );
        let issues_two = glance_two["issues"].as_array().unwrap();
        assert_eq!(issues_two.len(), 0);
    }

    fn fresh_backlog() -> BacklogSource {
        BacklogSource {
            backlog: Some(json!({"stuck_days":0}).as_object().unwrap().clone()),
            validity: BacklogValidity::Valid,
            generated_at: Some("2026-05-14T12:00:00+00:00".to_owned()),
            saved_lane: None,
        }
    }

    fn now() -> Timestamp {
        crate::test_time::utc_at(2026, 5, 14, 15, 30, 0)
    }

    fn glance(capture: &Value) -> Value {
        glance_at(capture, None, &Value::Null, now())
    }

    fn glance_at(
        capture: &Value,
        last_observe: Option<&str>,
        brain: &Value,
        at: Timestamp,
    ) -> Value {
        build_health_glance(capture, last_observe, &fresh_backlog(), brain, at, None)
    }

    #[test]
    fn source_refusal_counts_devices_and_clears_only_with_authoritative_evidence() {
        let mut capture = json!({"status":"active", "registry":"registry_complete", "clients":[
            {"name":"phone", "failing":false, "source_delivery":{
                "audio":{"state":"needs_attention"}, "location":{"state":"current"}}},
            {"name":"laptop", "failing":false, "source_delivery":{"screen":{"state":"current"}}}
        ]});
        let g = glance(&capture);
        assert_eq!(g["verdict"], "attention");
        assert_eq!(g["issues"].as_array().unwrap().len(), 1);
        assert!(
            g["issues"][0]["text"]
                .as_str()
                .unwrap()
                .contains("phone (audio)")
        );
        assert_eq!(g["issues"][0]["href"], "/app/health/#registeredClientsCard");
        capture["clients"][0]["source_delivery"]
            .as_object_mut()
            .unwrap()
            .remove("location");
        assert_eq!(
            glance(&capture)["verdict"],
            "attention",
            "one source is enough"
        );
        capture["clients"][1]["source_delivery"]["screen"]["state"] = json!("needs_attention");
        capture["clients"][1]["source_delivery"]["audio"] = json!({"state":"needs_attention"});
        assert!(
            glance(&capture)["issues"][0]["text"]
                .as_str()
                .unwrap()
                .contains("2 devices")
        );
        capture["clients"][0]["source_delivery"]["audio"]["state"] = json!("current");
        capture["clients"][1]["source_delivery"] = json!({"screen":{"state":"current"}});
        assert_eq!(glance(&capture)["verdict"], "ok");
        let unknown = glance(&json!({"status":"unknown", "clients":[]}));
        assert_ne!(unknown["verdict"], "ok");
        assert!(unknown["issues"].as_array().unwrap().is_empty());
    }

    fn unassessed(name: &str, reason: &str, reach: &str) -> Value {
        json!({"name": name, "reason": reason, "reach": reach})
    }

    fn client(name: &str, status: &str, reach: &str) -> Value {
        // `failing` is what home_client_row emits and what both surfaces read,
        // so the fixture carries it rather than restating the rule (F-8).
        json!({"name": name, "status": status, "reach": reach, "failing": status == "degraded"})
    }

    /// Quiet paired devices use the calm quiet headline without turning silence
    /// into an attention demand; per-device quiet ages live in the device strip.
    #[test]
    fn quiet_device_headline_stays_calm_without_requiring_attention() {
        let mut iphone = client("iPhone's iPhone", "offline", "offline");
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
        assert_eq!(glanced["headline"], text(ENGLISH, QUIET_HEADLINE_ID));
        assert_eq!(glanced["verdict"], "calm");
        assert_eq!(glanced["severity"], "neutral");
        assert!(glanced["cta"].is_null());
        assert_eq!(glanced["issues"].as_array().unwrap().len(), 0);

        let one = json!({
            "status": "offline",
            "clients": [client("suze", "offline", "offline")],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert_eq!(glance(&one)["verdict"], "calm");
        assert_eq!(glance(&one)["headline"], text(ENGLISH, QUIET_HEADLINE_ID));
    }

    #[test]
    fn injected_inputs_produce_active_and_quiet_verdicts() {
        let backlog = fresh_backlog();
        assert_eq!(
            build_health_glance(
                &json!({"status":"active"}),
                Some("29 seconds ago"),
                &backlog,
                &Value::Null,
                now(),
                None,
            )["verdict"],
            "ok"
        );
        assert_eq!(
            build_health_glance(
                &json!({"status":"offline"}),
                None,
                &backlog,
                &Value::Null,
                now(),
                None,
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
        assert!(g["cta"].is_null());
        assert_eq!(g["issues"].as_array().unwrap().len(), 0);
        assert_eq!(g["headline"], text(ENGLISH, EMPTY_REGISTRY_HEADLINE_ID));

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
            text(ENGLISH, QUIET_HEADLINE_ID)
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
            text(ENGLISH, QUIET_HEADLINE_ID)
        );
        let one_offline = json!({
            "status": "offline",
            "clients": [client("suze", "offline", "offline")],
            "unassessed": [],
            "registry": "registry_complete",
        });
        assert_eq!(
            glance(&one_offline)["headline"],
            text(ENGLISH, QUIET_HEADLINE_ID)
        );
        let mut one_offline_timed = one_offline.clone();
        one_offline_timed["clients"][0]["last_accepted_ingest_at"] = json!("2026-05-13T15:30:00Z");
        let timed = glance(&one_offline_timed);
        assert_eq!(timed["headline"], text(ENGLISH, QUIET_HEADLINE_ID));
        assert_eq!(timed["severity"], "neutral");
        assert!(timed["cta"].is_null());

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
        assert!(degraded_text.contains("rej"));

        let missing_backlog = BacklogSource {
            backlog: None,
            validity: BacklogValidity::Missing,
            generated_at: None,
            saved_lane: None,
        };
        let backlog_over_invalid = build_health_glance(
            &invalid_active,
            None,
            &missing_backlog,
            &Value::Null,
            now(),
            None,
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
        assert_eq!(running_g["verdict"], "calm");
        assert_eq!(running_g["severity"], "neutral");
        assert!(running_g["issues"].as_array().unwrap().is_empty());
        assert!(running_g["cta"].is_null());
        let running_text = running_g["headline"].as_str().unwrap();
        let asleep_text = asleep_g["headline"].as_str().unwrap();
        assert_eq!(running_text, asleep_text);
        assert_eq!(running_text, text(ENGLISH, QUIET_HEADLINE_ID));

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
        assert_eq!(glance(&mixed)["headline"], text(ENGLISH, QUIET_HEADLINE_ID));

        let corpus_shaped = json!({"status": "stale", "clients": [{"name": "laptop"}]});
        assert_eq!(
            glance(&corpus_shaped)["headline"],
            text(ENGLISH, QUIET_HEADLINE_ID)
        );
        assert!(glance(&corpus_shaped)["cta"].is_null());

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

        let later = now() + SignedDuration::from_hours(24 * 30);
        let at_clock = |at: Timestamp| {
            let backlog = BacklogSource {
                backlog: Some(json!({"stuck_days":0}).as_object().unwrap().clone()),
                validity: BacklogValidity::Valid,
                generated_at: Some(solstone_core_time::utc_rfc3339(at)),
                saved_lane: None,
            };
            build_health_glance(&awaiting, None, &backlog, &Value::Null, at, None)
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
        assert!(
            g["issues"][0]["text"]
                .as_str()
                .unwrap()
                .contains("phone (location)")
        );
    }

    #[test]
    fn single_source_active_rollup_surfaces_an_explicit_refusal() {
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
            assert_eq!(g["verdict"], "attention", "{capture}");
            assert_eq!(g["issues"].as_array().unwrap().len(), 1);
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
        assert!(
            g["issues"][0]["text"]
                .as_str()
                .unwrap()
                .contains("phone (default)")
        );
    }

    fn not_yet(kind: solstone_core_system_health::NotYet) -> BacklogSource {
        BacklogSource {
            backlog: None,
            validity: BacklogValidity::NotYet(kind),
            generated_at: None,
            saved_lane: None,
        }
    }

    fn not_yet_glance(capture: &Value, brain: &Value, backlog: &BacklogSource) -> Value {
        build_health_glance(capture, Some("2 minutes ago"), backlog, brain, now(), None)
    }

    /// Day 0: the missing summary is a note on every verdict, never an issue,
    /// and never lets a delivering device turn home green.
    #[test]
    fn first_night_is_a_calm_note_never_a_count_and_never_green() {
        use solstone_core_journal_config::SavedLane;
        use solstone_core_system_health::NotYet;
        let note = |glance: &Value| glance["note"]["text"].as_str().map(str::to_owned);

        // A device already delivering on day 0: calm, not ok.
        let active = json!({"status": "active", "clients": [client("phone", "active", "active")], "registry": "registry_complete"});
        let delivering = not_yet_glance(&active, &Value::Null, &not_yet(NotYet::FirstNight));
        assert_eq!(delivering["verdict"], "calm");
        assert_eq!(delivering["severity"], "neutral");
        assert_eq!(
            delivering["headline"],
            text(ENGLISH, NOT_YET_ACTIVE_HEADLINE_ID)
        );
        assert_eq!(delivering["last_observation"], "2 minutes ago");
        assert!(delivering["issues"].as_array().unwrap().is_empty());
        assert_eq!(
            note(&delivering).as_deref(),
            Some("you'll see whether your journal is caught up after its first night.")
        );
        assert_eq!(delivering["note"]["href"], "/app/health/#backlogVerdict");

        // No way to think yet is a choice still ahead, not a fault: the note
        // carries it, nothing is counted, and no ambiguous inline CTA is injected.
        let empty = json!({"status": "no_clients", "registry": "registry_empty"});
        let blocked = json!({"state": "blocked", "reason_code": "thinking_engine_not_chosen",
            "headline": "thinking isn't set up yet"});
        let no_engine = not_yet_glance(&empty, &blocked, &not_yet(NotYet::AwaitingEngine(None)));
        assert_eq!(no_engine["verdict"], "calm");
        assert!(no_engine["issues"].as_array().unwrap().is_empty());
        assert!(no_engine["cta"].is_null());
        assert_eq!(
            note(&no_engine).as_deref(),
            Some("your journal starts catching up once you choose where the thinking happens.")
        );
        assert_eq!(no_engine["note"]["href"], "/app/thinking/");

        for (lane, expected_href) in [
            (SavedLane::Byo, "/app/thinking/#byo-setup"),
            (SavedLane::Spp, "/app/thinking/#confidential-setup"),
            (SavedLane::Local, "/app/thinking/#local-setup"),
        ] {
            let mut backlog_with_not_yet = not_yet(NotYet::AwaitingEngine(Some(lane)));
            backlog_with_not_yet.saved_lane = None;
            let glance = not_yet_glance(&empty, &blocked, &backlog_with_not_yet);
            assert_eq!(
                note(&glance).as_deref(),
                Some(NotYet::AwaitingEngine(Some(lane)).text().as_str())
            );
            assert_eq!(glance["note"]["href"], expected_href);
        }

        // A summary from earlier exists, and still no way to think is chosen:
        // the same note, never an issue.
        let unchosen_later = not_yet_glance(&active, &blocked, &fresh_backlog());
        assert!(unchosen_later["issues"].as_array().unwrap().is_empty());
        assert_eq!(
            note(&unchosen_later).as_deref(),
            Some("your journal starts catching up once you choose where the thinking happens.")
        );
        assert_eq!(unchosen_later["verdict"], "calm");

        // Summary exists (Valid backlog) and brain reason_code == "thinking_engine_not_chosen":
        // href and text follow backlog.saved_lane
        let mut byo_backlog = fresh_backlog();
        byo_backlog.saved_lane = Some(SavedLane::Byo);
        let byo_glance = not_yet_glance(&active, &blocked, &byo_backlog);
        assert_eq!(
            note(&byo_glance).as_deref(),
            Some(NotYet::AwaitingEngine(Some(SavedLane::Byo)).text().as_str())
        );
        assert_eq!(byo_glance["note"]["href"], "/app/thinking/#byo-setup");

        // A blocked lane that needs a step is still the ask it was.
        let key_missing = json!({"state": "blocked", "reason_code": "provider_key_missing",
            "headline": "thinking isn't set up yet"});
        let asked = not_yet_glance(&empty, &key_missing, &fresh_backlog());
        assert_eq!(asked["verdict"], "attention");
        assert_eq!(asked["issues"][0]["text"], "thinking isn't set up yet");
        assert!(asked["cta"].is_null());

        // Local thinking still arriving: progressing, without an inline device CTA.
        let arriving = json!({"state": "blocked", "reason_code": "local_runtime_not_ready",
            "headline": "setting up local thinking", "progressing": true});
        let progressing = not_yet_glance(&empty, &arriving, &fresh_backlog());
        assert_eq!(progressing["verdict"], "progressing");
        assert!(progressing["cta"].is_null());

        // No devices, thinking set up: ready headline with no inline CTA.
        let first_run = not_yet_glance(&empty, &Value::Null, &not_yet(NotYet::FirstNight));
        assert_eq!(first_run["verdict"], "calm");
        assert_eq!(
            first_run["headline"],
            text(ENGLISH, EMPTY_REGISTRY_HEADLINE_ID)
        );
        assert!(first_run["cta"].is_null());
        assert_eq!(
            note(&first_run).as_deref(),
            Some("you'll see whether your journal is caught up after its first night.")
        );

        // A summary that is missing for any other reason keeps today's amber.
        let missing = BacklogSource {
            backlog: None,
            validity: BacklogValidity::Missing,
            generated_at: None,
            saved_lane: None,
        };
        let amber = not_yet_glance(&active, &Value::Null, &missing);
        assert_eq!(amber["verdict"], "attention");
        assert_eq!(
            amber["issues"][0]["text"],
            "it's unclear whether your journal is caught up right now."
        );
        assert!(amber.get("note").is_none());

        // A healthy summary: no note, and green is earned.
        let green = glance_at(&active, Some("2 minutes ago"), &Value::Null, now());
        assert_eq!(green["verdict"], "ok");
        assert!(green.get("note").is_none());
    }

    #[test]
    fn status_badge_summary_reflects_calm_and_attention_states() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        std::fs::create_dir_all(root.join("config")).expect("config dir");
        std::fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1778760000},"identity":{"timezone":"UTC"}}"#,
        )
        .expect("write config");
        let t0 = crate::test_time::utc_at(2026, 5, 14, 15, 30, 0);
        solstone_core_brain::begin_refresh(root, t0, None, None, false, None)
            .expect("write initial none brain record");
        let day0_ctx = crate::HomeContext::new(root, t0);
        let day0 = status_badge_summary_at(&day0_ctx);
        assert_eq!(day0.issues, 0);
        assert_eq!(day0.severity, None);
        assert!(!day0.uncertain);

        // Unparseable stats.json surfaces 1 amber backlog issue; stuck day surfaces red.
        std::fs::write(root.join("stats.json"), b"not json").expect("write unparseable stats");
        let amber = status_badge_summary_at(&day0_ctx);
        assert_eq!(amber.issues, 1);
        assert_eq!(amber.severity, Some("amber"));

        std::fs::write(
            root.join("stats.json"),
            br#"{"generated_at":"2026-05-14T15:25:00Z","backlog":{"stuck_days":1,"days":[{"state":"stuck","reason":"needs retry"}]}}"#,
        )
        .expect("write stuck stats");
        let red = status_badge_summary_at(&day0_ctx);
        assert_eq!(red.issues, 1);
        assert_eq!(red.severity, Some("red"));

        // Clean backlog + corrupt indexer/journal.sqlite surfaces failing search index on both glance and badge.
        std::fs::write(
            root.join("stats.json"),
            br#"{"generated_at":"2026-05-14T15:25:00Z","backlog":{"stuck_days":0,"days":[]}}"#,
        )
        .expect("write clean stats");
        assert_eq!(status_badge_summary_at(&day0_ctx).issues, 0);

        // Routine behind (searchable file without sqlite yet) stays off Home and badge.
        std::fs::create_dir_all(root.join("reflections/weekly")).expect("weekly dir");
        std::fs::write(root.join("reflections/weekly/20260510.md"), b"hello").expect("weekly md");
        assert!(search_index_issue(&day0_ctx).is_none());
        assert_eq!(status_badge_summary_at(&day0_ctx).issues, 0);

        // Corrupt sqlite file triggers IndexHealthState::Failing -> amber issue on both glance and badge.
        std::fs::create_dir_all(root.join("indexer")).expect("indexer dir");
        std::fs::write(root.join("indexer/journal.sqlite"), b"corrupt sqlite bytes")
            .expect("write corrupt sqlite");
        let idx_issue = search_index_issue(&day0_ctx).expect("failing search index issue");
        assert_eq!(idx_issue["severity"], "amber");
        assert_eq!(idx_issue["href"], "/app/health/#backlogVerdict");
        assert!(!idx_issue["text"].as_str().unwrap_or("").is_empty());
        let with_failing_index = status_badge_summary_at(&day0_ctx);
        assert_eq!(with_failing_index.issues, 1);
        assert_eq!(with_failing_index.severity, Some("amber"));
    }
}
