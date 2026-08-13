// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Brain-health projections used by the Thinking read surface.

use std::path::Path;

use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_brain::{BrainInspection, inspect_brain_state, present_brain_inspection};

pub fn presentation(journal: &Path, config: &Map<String, Value>, spp_configured: bool) -> Value {
    let now = Utc::now();
    let inspection = inspect_brain_state(journal, config, now);
    let view = present_brain_inspection(&inspection, now);
    let record = inspection.record.as_ref();
    let projection = &inspection.projection;
    let reason = projection.reason_code.as_deref();
    let components = json!({
        "generate": component(record, "generate"),
        "cogitate": component(record, "cogitate"),
    });
    let brain = json!({
        "state": projection.aggregate_state,
        "headline": view.headline,
        "reason_code": projection.reason_code,
        "reason_text": view.reason_text,
        "failing_component": view.failing_component,
        "action": action(&projection.aggregate_state, reason, projection.active_lane.as_deref(), view.failing_component.as_deref()),
        "identity": {"lane": projection.active_lane, "provider": projection.active_provider, "model": projection.active_model},
        "evidence": {"observed_at": view.evidence.observed_at, "age_seconds": view.evidence.age_seconds, "age_text": view.evidence.age_text},
        "components": components,
        "progressing": reason == Some("brain_check_in_progress") || (reason == Some("local_runtime_not_ready") && projection.runtime_transition_in_progress),
    });
    json!({
        "brain": brain,
        "spp_active": projection.active_lane.as_deref() == Some("spp"),
        "spp_readiness": spp_readiness(&inspection),
        "confidential_attestation": confidential_attestation(&inspection, spp_configured),
    })
}

fn component(record: Option<&Value>, name: &str) -> Value {
    let value = record
        .and_then(|record| record.get("evidence"))
        .and_then(Value::as_object)
        .and_then(|evidence| evidence.get(name));
    let reason = value
        .and_then(|item| item.get("reason_code"))
        .and_then(Value::as_str);
    json!({
        "status": value.and_then(|item| item.get("status")).cloned().unwrap_or(Value::Null),
        "reason_code": reason,
        "reason_text": reason.map(reason_text).unwrap_or_else(|| if value.and_then(|item| item.get("status")).and_then(Value::as_str) == Some("ok") { "ok".to_owned() } else { "unknown".to_owned() }),
        "observed_at": value.and_then(|item| item.get("observed_at")).cloned().unwrap_or(Value::Null),
    })
}

fn reason_text(reason: &str) -> String {
    match reason {
        "thinking_engine_not_chosen" => "no thinking engine chosen".to_owned(),
        "configuration_invalid" => "configuration invalid".to_owned(),
        "stale_expected_fingerprint" => "stale expected fingerprint".to_owned(),
        "lost_fence" => "refresh fence lost".to_owned(),
        "busy" => "check already running".to_owned(),
        _ => reason.replace('_', " "),
    }
}

fn action(state: &str, reason: Option<&str>, lane: Option<&str>, failing: Option<&str>) -> Value {
    let bundled_runtime = lane == Some("bundled")
        && (matches!(
            reason,
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
        ) || (reason == Some("probe_internal_error") && failing == Some("lane_prerequisites")));
    if state == "ready"
        || state == "checking"
        || (state == "blocked" && reason == Some("brain_check_in_progress"))
    {
        return Value::Null;
    }
    if matches!(state, "blocked" | "unhealthy") {
        return if bundled_runtime {
            json!({"label":"open local setup","href":"/app/thinking/#local-setup"})
        } else {
            json!({"label":"open thinking","href":"/app/thinking/#main"})
        };
    }
    if state == "unknown" && reason == Some("configuration_invalid") {
        return json!({"label":"open thinking","href":"/app/thinking/#main"});
    }
    if state == "unknown" {
        return json!({"label":"check again","refresh":true});
    }
    Value::Null
}

fn usable_spp_component<'a>(inspection: &'a BrainInspection, name: &str) -> Option<&'a Value> {
    let projection = &inspection.projection;
    if projection.active_lane.as_deref() != Some("spp")
        || projection_only(projection.reason_code.as_deref())
    {
        return None;
    }
    inspection.record.as_ref()?.get("evidence")?.get(name)
}

fn spp_readiness(inspection: &BrainInspection) -> Value {
    let generate = usable_spp_component(inspection, "generate");
    let cogitate = usable_spp_component(inspection, "cogitate");
    let generate_ready = generate
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        == Some("ok");
    let cogitate_ready = cogitate
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        == Some("ok");
    let mut issues = Vec::new();
    if inspection.projection.aggregate_state != "ready" {
        add_issue(&mut issues, inspection.projection.reason_code.as_deref());
    }
    for value in [generate, cogitate] {
        if value
            .and_then(|item| item.get("status"))
            .and_then(Value::as_str)
            != Some("ok")
        {
            add_issue(
                &mut issues,
                value
                    .and_then(|item| item.get("reason_code"))
                    .and_then(Value::as_str),
            );
        }
    }
    if (!generate_ready || !cogitate_ready) && issues.is_empty() {
        issues.push("brain_record_invalid".to_owned());
    }
    json!({"generate_ready":generate_ready,"cogitate_ready":cogitate_ready,"issues":issues})
}

fn add_issue(issues: &mut Vec<String>, issue: Option<&str>) {
    if let Some(issue) = issue
        && !issues.iter().any(|candidate| candidate == issue)
    {
        issues.push(issue.to_owned());
    }
}

pub fn confidential_attestation(inspection: &BrainInspection, spp_configured: bool) -> Value {
    if !spp_configured {
        return json!({"state":"off","reason":"confidential_not_configured","observed_at":null,"expires_at":null});
    }
    let projection = &inspection.projection;
    if projection.active_lane.as_deref() != Some("spp") {
        return json!({"state":"inactive","reason":"confidential_not_active","observed_at":null,"expires_at":null});
    }
    if projection.aggregate_state == "checking" {
        return json!({"state":"verifying","reason":"brain_check_in_progress","observed_at":null,"expires_at":null});
    }
    if projection_only(projection.reason_code.as_deref()) {
        return json!({"state":"stale","reason":projection.reason_code,"observed_at":null,"expires_at":null});
    }
    let Some(component) = usable_spp_component(inspection, "lane_prerequisites") else {
        return json!({"state":"stale","reason":projection.reason_code.clone().unwrap_or_else(|| "brain_record_invalid".to_owned()),"observed_at":null,"expires_at":null});
    };
    let reason = component.get("reason_code").and_then(Value::as_str);
    let observed = component.get("observed_at").cloned().unwrap_or(Value::Null);
    let expires = component.get("expires_at").cloned().unwrap_or(Value::Null);
    let state = if component.get("status").and_then(Value::as_str) == Some("ok") {
        "verified"
    } else if reason == Some("attestation_rejected")
        || matches!(
            reason,
            Some(
                "nvattest_platform_unsupported"
                    | "nvattest_unavailable"
                    | "nvattest_install_failed"
                    | "nvattest_integrity_failed"
            )
        )
    {
        "failed"
    } else if reason == Some("nvattest_install_in_progress") {
        "verifying"
    } else if reason == Some("attestation_not_verified") {
        "unreachable"
    } else if reason == Some("attestation_expired") {
        "stale"
    } else {
        return json!({"state":"stale","reason":reason.or(projection.reason_code.as_deref()).unwrap_or("brain_record_invalid"),"observed_at":null,"expires_at":null});
    };
    json!({"state":state,"reason":reason,"observed_at":observed,"expires_at":expires})
}

fn projection_only(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some(
            "brain_record_missing"
                | "brain_record_invalid"
                | "brain_record_unavailable"
                | "configuration_invalid"
                | "fingerprint_key_unavailable"
                | "brain_check_interrupted"
                | "stale_expected_fingerprint"
                | "lost_fence"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test names and pins one public attestation return branch.
    fn inspection(
        lane: &str,
        state: &str,
        reason: Option<&str>,
        component: Option<Value>,
    ) -> BrainInspection {
        BrainInspection {
            status: solstone_core_brain::InspectionStatus::Ok,
            projection: solstone_core_brain::BrainProjection {
                aggregate_state: state.into(),
                reason_code: reason.map(str::to_owned),
                active_lane: Some(lane.into()),
                active_provider: None,
                active_model: None,
                fingerprint_sha256: None,
                runtime_transition_in_progress: false,
            },
            error: None,
            record: component.map(|component| json!({"evidence":{"lane_prerequisites":component}})),
        }
    }
    fn assert_branch(
        value: Value,
        state: &str,
        reason: Option<&str>,
        observed: bool,
        expires: bool,
    ) {
        assert_eq!(value["state"], state);
        assert_eq!(
            value["reason"],
            reason.map(Value::from).unwrap_or(Value::Null)
        );
        assert_eq!(!value["observed_at"].is_null(), observed);
        assert_eq!(!value["expires_at"].is_null(), expires);
    }
    #[test]
    fn attestation_off_branch() {
        assert_branch(
            confidential_attestation(&inspection("spp", "ready", None, None), false),
            "off",
            Some("confidential_not_configured"),
            false,
            false,
        );
    }
    #[test]
    fn attestation_inactive_branch() {
        assert_branch(
            confidential_attestation(&inspection("byo-cloud", "ready", None, None), true),
            "inactive",
            Some("confidential_not_active"),
            false,
            false,
        );
    }
    #[test]
    fn attestation_checking_branch() {
        assert_branch(
            confidential_attestation(&inspection("spp", "checking", None, None), true),
            "verifying",
            Some("brain_check_in_progress"),
            false,
            false,
        );
    }
    #[test]
    fn attestation_projection_stale_branch() {
        assert_branch(
            confidential_attestation(
                &inspection("spp", "unknown", Some("brain_record_invalid"), None),
                true,
            ),
            "stale",
            Some("brain_record_invalid"),
            false,
            false,
        );
    }
    #[test]
    fn attestation_missing_prerequisites_branch() {
        assert_branch(
            confidential_attestation(&inspection("spp", "unhealthy", Some("x"), None), true),
            "stale",
            Some("x"),
            false,
            false,
        );
    }
    #[test]
    fn attestation_verified_branch() {
        assert_branch(
            confidential_attestation(
                &inspection(
                    "spp",
                    "ready",
                    None,
                    Some(json!({"status":"ok","observed_at":"o","expires_at":"e"})),
                ),
                true,
            ),
            "verified",
            None,
            true,
            true,
        );
    }
    #[test]
    fn attestation_rejected_branch() {
        assert_branch(
            confidential_attestation(
                &inspection(
                    "spp",
                    "unhealthy",
                    None,
                    Some(
                        json!({"status":"failed","reason_code":"attestation_rejected","observed_at":"o","expires_at":"e"}),
                    ),
                ),
                true,
            ),
            "failed",
            Some("attestation_rejected"),
            true,
            true,
        );
    }
    #[test]
    fn attestation_installing_branch() {
        assert_branch(
            confidential_attestation(
                &inspection(
                    "spp",
                    "unhealthy",
                    None,
                    Some(
                        json!({"status":"failed","reason_code":"nvattest_install_in_progress","observed_at":"o","expires_at":"e"}),
                    ),
                ),
                true,
            ),
            "verifying",
            Some("nvattest_install_in_progress"),
            true,
            true,
        );
    }
    #[test]
    fn attestation_install_failure_branch() {
        assert_branch(
            confidential_attestation(
                &inspection(
                    "spp",
                    "unhealthy",
                    None,
                    Some(
                        json!({"status":"failed","reason_code":"nvattest_unavailable","observed_at":"o","expires_at":"e"}),
                    ),
                ),
                true,
            ),
            "failed",
            Some("nvattest_unavailable"),
            true,
            true,
        );
    }
    #[test]
    fn attestation_unreachable_branch() {
        assert_branch(
            confidential_attestation(
                &inspection(
                    "spp",
                    "unhealthy",
                    None,
                    Some(
                        json!({"status":"failed","reason_code":"attestation_not_verified","observed_at":"o","expires_at":"e"}),
                    ),
                ),
                true,
            ),
            "unreachable",
            Some("attestation_not_verified"),
            true,
            true,
        );
    }
    #[test]
    fn attestation_expired_branch() {
        assert_branch(
            confidential_attestation(
                &inspection(
                    "spp",
                    "unhealthy",
                    None,
                    Some(
                        json!({"status":"failed","reason_code":"attestation_expired","observed_at":"o","expires_at":"e"}),
                    ),
                ),
                true,
            ),
            "stale",
            Some("attestation_expired"),
            true,
            true,
        );
    }
    #[test]
    fn attestation_fallback_branch() {
        assert_branch(
            confidential_attestation(
                &inspection(
                    "spp",
                    "unhealthy",
                    None,
                    Some(json!({"status":"failed","reason_code":"other"})),
                ),
                true,
            ),
            "stale",
            Some("other"),
            false,
            false,
        );
    }
}
