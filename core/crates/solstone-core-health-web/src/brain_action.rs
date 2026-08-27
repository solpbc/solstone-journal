// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI-specific brain-health presentation for the journal-data Health API.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use solstone_core_brain::{inspect_brain_state, present_brain_inspection};

/// Build the historical `{snapshot, lines}` health-report brain payload.
///
/// A failed brain inspection is a degraded report field, not a failed report.
pub(crate) fn build_cli_brain_health(journal_root: &std::path::Path, now: DateTime<Utc>) -> Value {
    let Ok(config) = solstone_core_thinking::read_config(journal_root) else {
        return fallback();
    };
    let inspection = inspect_brain_state(journal_root, &config, now);
    let presentation = present_brain_inspection(&inspection, now);
    let projection = inspection.projection;
    let failing_component = presentation.failing_component;
    let progressing = projection.reason_code.as_deref() == Some("brain_check_in_progress")
        || (projection.reason_code.as_deref() == Some("local_runtime_not_ready")
            && projection.runtime_transition_in_progress);
    let action = resolve_cli_brain_action(
        &projection.aggregate_state,
        projection.reason_code.as_deref(),
        projection.active_lane.as_deref(),
        failing_component.as_deref(),
        progressing,
    );
    let headline = headline(&projection.aggregate_state);
    let snapshot = json!({
        "state": projection.aggregate_state,
        "headline": headline,
        "reason_code": projection.reason_code,
        "reason_text": presentation.reason_text,
        "failing_component": failing_component,
        "action": action,
        "identity": {
            "lane": projection.active_lane,
            "provider": projection.active_provider,
            "model": projection.active_model,
        },
        "evidence": {
            "observed_at": presentation.evidence.observed_at,
            "age_seconds": presentation.evidence.age_seconds,
            "age_text": presentation.evidence.age_text,
        },
        "components": components(inspection.record.as_ref()),
        "progressing": progressing,
    });
    let lines = render_lines(&snapshot);
    json!({"snapshot":snapshot,"lines":lines})
}

/// Resolve the action for the native CLI surface, which differs from both Home
/// and Support portal action contracts.
pub(crate) fn resolve_cli_brain_action(
    state: &str,
    reason_code: Option<&str>,
    active_lane: Option<&str>,
    failing_component: Option<&str>,
    progressing: bool,
) -> Value {
    if matches!(state, "ready" | "checking") || (state == "blocked" && progressing) {
        return Value::Null;
    }
    if matches!(state, "blocked" | "unhealthy") {
        if bundled_runtime_issue(active_lane, reason_code, failing_component) {
            return json!({"label":"open local setup","href":"/app/thinking/#local-setup"});
        }
        return json!({"label":"open thinking","href":"/app/thinking/#main"});
    }
    if state == "unknown" && reason_code == Some("configuration_invalid") {
        return json!({"label":"open thinking","href":"/app/thinking/#main"});
    }
    if state == "unknown" {
        return json!({"label":"check again","command":"journal brain refresh"});
    }
    Value::Null
}

fn bundled_runtime_issue(
    active_lane: Option<&str>,
    reason_code: Option<&str>,
    failing_component: Option<&str>,
) -> bool {
    active_lane == Some("bundled")
        && (matches!(
            reason_code,
            Some(
                "gpu_unavailable"
                    | "local_runtime_not_ready"
                    | "local_artifact_not_ready"
                    | "local_server_unhealthy"
                    | "local_runtime_state_invalid"
                    | "local_runtime_state_unavailable"
                    | "local_runtime_state_stale"
                    | "local_runtime_fingerprint_mismatch"
            )
        ) || (reason_code == Some("probe_internal_error")
            && failing_component == Some("lane_prerequisites")))
}

fn components(record: Option<&Value>) -> Value {
    let evidence = record
        .and_then(|value| value.get("evidence"))
        .and_then(Value::as_object);
    let mut result = Map::new();
    for name in ["generate", "cogitate"] {
        let component = evidence.and_then(|value| value.get(name));
        let reason = component
            .and_then(|value| value.get("reason_code"))
            .and_then(Value::as_str);
        result.insert(
            name.to_owned(),
            json!({
                "status": component.and_then(|value| value.get("status")).cloned().unwrap_or(Value::Null),
                "reason_code": reason,
                "reason_text": reason.map(|value| value.replace('_', " ")).unwrap_or_else(|| {
                    if component.and_then(|value| value.get("status")).and_then(Value::as_str) == Some("ok") { "ok".to_owned() } else { "unknown".to_owned() }
                }),
                "observed_at": component.and_then(|value| value.get("observed_at")).cloned().unwrap_or(Value::Null),
            }),
        );
    }
    Value::Object(result)
}

fn headline(state: &str) -> &'static str {
    match state {
        "ready" => "sol can think",
        "checking" => "checking how sol thinks",
        "blocked" => "sol needs a way to think",
        "unhealthy" => "sol's thinking needs attention",
        _ => "thinking status unavailable",
    }
}

fn render_lines(snapshot: &Value) -> Vec<String> {
    let headline = snapshot
        .get("headline")
        .and_then(Value::as_str)
        .unwrap_or("thinking status unavailable");
    let mut lines = vec!["Brain Health".to_owned(), format!("  {headline}")];
    let identity = snapshot.get("identity").and_then(Value::as_object);
    let lane = identity
        .and_then(|value| value.get("lane"))
        .and_then(Value::as_str);
    let provider = identity
        .and_then(|value| value.get("provider"))
        .and_then(Value::as_str);
    let model = identity
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str);
    let reason = snapshot
        .get("reason_text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let component = snapshot.get("failing_component").and_then(Value::as_str);
    let suffix = component
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    match (lane, provider, model) {
        (Some(lane), Some(provider), Some(model))
            if snapshot.get("state").and_then(Value::as_str) == Some("ready") =>
        {
            let age = snapshot
                .get("evidence")
                .and_then(|value| value.get("age_text"))
                .and_then(Value::as_str);
            lines.push(match age {
                Some(age) => format!("  {lane} {provider}/{model}, checked {age} ago"),
                None => format!("  {lane} {provider}/{model}"),
            });
        }
        (Some(lane), Some(provider), Some(model)) => {
            lines.push(format!("  {lane} {provider}/{model} — {reason}{suffix}"));
        }
        _ if lane.is_some() || provider.is_some() || model.is_some() => {
            lines.push(format!("  {reason}{suffix}"));
        }
        _ => {}
    }
    if let Some(action) = snapshot.get("action").and_then(Value::as_object) {
        let label = action
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = action
            .get("href")
            .or_else(|| action.get("command"))
            .and_then(Value::as_str);
        lines.push(match target {
            Some(target) => format!("  → {label}: {target}"),
            None => format!("  → {label}"),
        });
    }
    lines
}

fn fallback() -> Value {
    json!({
        "snapshot":{"state":"unknown","headline":"thinking status unavailable","reason_code":"brain_record_unavailable"},
        "lines":["Brain Health","  thinking status unavailable"]
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use tempfile::TempDir;

    use super::{build_cli_brain_health, resolve_cli_brain_action};

    #[test]
    fn cli_actions_match_the_surface_contract() {
        assert_eq!(
            resolve_cli_brain_action("unknown", None, None, None, false),
            json!({"label":"check again","command":"journal brain refresh"})
        );
        assert_eq!(
            resolve_cli_brain_action("unknown", Some("configuration_invalid"), None, None, false),
            json!({"label":"open thinking","href":"/app/thinking/#main"})
        );
        assert_eq!(
            resolve_cli_brain_action(
                "blocked",
                Some("gpu_unavailable"),
                Some("bundled"),
                None,
                false
            ),
            json!({"label":"open local setup","href":"/app/thinking/#local-setup"})
        );
        assert_eq!(
            resolve_cli_brain_action("blocked", None, None, None, true),
            json!(null)
        );
    }

    #[test]
    fn ready_and_checking_have_no_action() {
        assert_eq!(
            resolve_cli_brain_action("ready", None, None, None, false),
            json!(null)
        );
        assert_eq!(
            resolve_cli_brain_action("checking", None, None, None, false),
            json!(null)
        );
    }

    #[test]
    fn blocked_non_local_problem_opens_main_thinking() {
        assert_eq!(
            resolve_cli_brain_action(
                "blocked",
                Some("provider_unavailable"),
                Some("cloud"),
                None,
                false
            ),
            json!({"label":"open thinking","href":"/app/thinking/#main"})
        );
    }

    #[test]
    fn unhealthy_local_runtime_problem_opens_local_setup() {
        assert_eq!(
            resolve_cli_brain_action(
                "unhealthy",
                Some("local_server_unhealthy"),
                Some("bundled"),
                None,
                false
            ),
            json!({"label":"open local setup","href":"/app/thinking/#local-setup"})
        );
    }

    #[test]
    fn unhealthy_non_local_problem_opens_main_thinking() {
        assert_eq!(
            resolve_cli_brain_action(
                "unhealthy",
                Some("provider_unavailable"),
                Some("cloud"),
                None,
                false
            ),
            json!({"label":"open thinking","href":"/app/thinking/#main"})
        );
    }

    #[test]
    fn lane_prerequisites_probe_failure_is_a_local_runtime_issue_when_unhealthy() {
        assert_eq!(
            resolve_cli_brain_action(
                "unhealthy",
                Some("probe_internal_error"),
                Some("bundled"),
                Some("lane_prerequisites"),
                false
            ),
            json!({"label":"open local setup","href":"/app/thinking/#local-setup"})
        );
    }

    #[test]
    fn unknown_probe_failure_keeps_the_reference_retry_action() {
        assert_eq!(
            resolve_cli_brain_action(
                "unknown",
                Some("probe_internal_error"),
                Some("bundled"),
                Some("lane_prerequisites"),
                false
            ),
            json!({"label":"check again","command":"journal brain refresh"})
        );
    }

    #[test]
    fn brain_config_read_failure_returns_safe_snapshot_and_lines() {
        let temporary = TempDir::new_in("/var/tmp").unwrap();
        let config = temporary.path().join("config/journal.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(config, "{").unwrap();
        let value = build_cli_brain_health(
            temporary.path(),
            Utc.with_ymd_and_hms(2026, 4, 10, 12, 0, 0).unwrap(),
        );
        assert_eq!(value["snapshot"]["state"], "unknown");
        assert_eq!(value["snapshot"]["reason_code"], "brain_record_unavailable");
        assert_eq!(
            value["lines"],
            json!(["Brain Health", "  thinking status unavailable"])
        );
    }
}
