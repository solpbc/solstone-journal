// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::fixture::local_contract;
use crate::inspect::BrainInspection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainEvidencePresentation {
    pub observed_at: Option<String>,
    pub age_seconds: Option<i64>,
    pub age_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainPresentation {
    pub headline: String,
    /// The brain is blocked only while something finishes on its own: a check,
    /// the confidential hardware-check install, or a local runtime transition.
    pub progressing: bool,
    pub reason_text: String,
    pub failing_component: Option<String>,
    pub evidence: BrainEvidencePresentation,
}

/// Render the stable owner-facing words for an inspected brain state.
pub fn present_brain_inspection(
    inspection: &BrainInspection,
    now: DateTime<Utc>,
) -> BrainPresentation {
    let projection = &inspection.projection;
    let reason = projection.reason_code.as_deref();
    let progressing = matches!(
        reason,
        Some("brain_check_in_progress" | "nvattest_install_in_progress")
    ) || (reason == Some("local_runtime_not_ready")
        && projection.runtime_transition_in_progress);
    let (mut failing_component, observed_at) = evidence_view(inspection.record.as_ref());
    if failing_component.is_none() {
        failing_component = component_for_reason(reason);
    }
    let (age_seconds, age_text) = brain_age(now, observed_at.as_deref());
    BrainPresentation {
        headline: headline(&projection.aggregate_state, reason, progressing).to_owned(),
        progressing,
        reason_text: match (reason, refused_register(inspection.record.as_ref())) {
            (Some("attestation_rejected"), Some(register)) => refused_register_text(register),
            _ => brain_reason_text(reason),
        },
        failing_component,
        evidence: BrainEvidencePresentation {
            observed_at,
            age_seconds,
            age_text,
        },
    }
}

/// Confidential processing can't verify the service on a platform with no
/// hardware check, so it isn't offered there. Windows is the only shipped
/// journal without one, so the words name it.
const NOT_ON_PLATFORM: &str = "confidential processing isn't on windows yet";

/// The owner headline for a brain state. Some `blocked` reasons have nothing
/// for the owner to set up: the work waits for the service, or for something
/// already under way, so the headline says so instead. Two confidential
/// hardware-check failures read like the other failed checks.
fn headline(state: &str, reason: Option<&str>, progressing: bool) -> &'static str {
    match (state, reason) {
        ("blocked", Some("attestation_not_verified")) => {
            "can't reach confidential processing right now"
        }
        ("blocked", Some("nvattest_install_in_progress")) => "checking confidential processing",
        ("blocked", Some("local_runtime_not_ready")) if progressing => {
            "setting up local processing"
        }
        ("blocked", Some("nvattest_platform_unsupported")) => NOT_ON_PLATFORM,
        ("blocked", Some("nvattest_unavailable")) => "processing needs attention",
        ("blocked", Some("confidential_access_ended")) => {
            "confidential processing isn't active for this sign-in"
        }
        ("ready", _) => "processing is ready",
        ("checking", _) => "checking how processing runs",
        ("blocked", _) => "processing needs a setup",
        ("unhealthy", _) => "processing needs attention",
        _ => "thinking status unavailable",
    }
}

pub fn processing_headline_for_reason(reason: &str) -> Option<&'static str> {
    let aggregate = local_contract()
        .brain_state
        .reason_to_aggregate
        .get(reason)?;
    Some(headline(aggregate, Some(reason), false))
}

/// Owner words for a brain reason code.
pub fn brain_reason_text(reason: Option<&str>) -> String {
    match reason {
        None => "ok".to_owned(),
        Some("thinking_engine_not_chosen") => "no model chosen".to_owned(),
        Some("configuration_invalid") => "configuration invalid".to_owned(),
        Some("stale_expected_fingerprint") => "stale expected fingerprint".to_owned(),
        Some("lost_fence") => "refresh fence lost".to_owned(),
        Some("busy") => "check already running".to_owned(),
        Some("attestation_not_verified") => "couldn't reach the service to verify it".to_owned(),
        Some("nvattest_install_in_progress") => "getting the hardware check ready".to_owned(),
        Some("nvattest_platform_unsupported") => NOT_ON_PLATFORM.to_owned(),
        Some("nvattest_unavailable") => {
            "something the hardware check needs isn't installed".to_owned()
        }
        Some("nvattest_install_failed") => {
            "couldn't install what the hardware check needs".to_owned()
        }
        Some("nvattest_integrity_failed") => {
            "the hardware check's files failed an integrity check".to_owned()
        }
        Some("chatgpt_not_eligible") => "this ChatGPT account isn't eligible".to_owned(),
        Some("chatgpt_sign_in_required") => "signed out of ChatGPT".to_owned(),
        Some("chatgpt_usage_limit") => "ChatGPT usage limit reached".to_owned(),
        Some("confidential_access_ended") => {
            "confidential processing isn't active for this sign-in".to_owned()
        }
        Some(reason) => reason.replace('_', " "),
    }
}

/// The one register a refused confidential service failed on, when the
/// refusal named one.
fn refused_register(record: Option<&Value>) -> Option<&str> {
    record?
        .get("evidence")?
        .get("lane_prerequisites")?
        .get("diagnostic")?
        .get("register")?
        .as_str()
}

/// Owner words for a service refused on one register: what the journal
/// checked, nothing more.
fn refused_register_text(register: &str) -> String {
    format!("the service's measurement {register} doesn't match its published value")
}

fn evidence_view(record: Option<&Value>) -> (Option<String>, Option<String>) {
    let Some(evidence) = record
        .and_then(|record| record.get("evidence"))
        .and_then(Value::as_object)
    else {
        return (None, None);
    };
    let mut ready = None;
    for name in &local_contract().brain_state.component_order {
        let Some(component) = evidence.get(name).and_then(Value::as_object) else {
            continue;
        };
        let observed_at = component
            .get("observed_at")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if component.get("status").and_then(Value::as_str) != Some("ok") {
            return (Some(name.clone()), observed_at);
        }
        if ready.is_none() {
            ready = observed_at;
        }
    }
    (None, ready)
}

fn component_for_reason(reason: Option<&str>) -> Option<String> {
    let reason = reason?;
    local_contract()
        .brain_state
        .evidence_reason_codes
        .iter()
        .find_map(|(component, reasons)| {
            reasons
                .iter()
                .any(|candidate| candidate == reason)
                .then(|| component.clone())
        })
}

fn brain_age(now: DateTime<Utc>, observed_at: Option<&str>) -> (Option<i64>, Option<String>) {
    let Some(observed) = observed_at
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
    else {
        return (None, None);
    };
    let seconds = (now - observed).num_seconds().max(0);
    let text = if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 172_800 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    };
    (Some(seconds), Some(text))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::{brain_age, present_brain_inspection};
    use crate::{BrainInspection, BrainProjection, InspectionStatus};

    #[test]
    fn projects_python_headline_reason_component_and_age_words() {
        let inspection = BrainInspection {
            status: InspectionStatus::Ok,
            projection: BrainProjection {
                aggregate_state: "unhealthy".into(),
                reason_code: Some("configuration_invalid".into()),
                active_lane: None,
                active_provider: None,
                active_model: None,
                fingerprint_sha256: None,
                runtime_transition_in_progress: false,
            },
            error: None,
            record: Some(
                json!({"evidence":{"generate":{"status":"failed","observed_at":"2026-01-01T00:00:00Z"}}}),
            ),
        };
        let view = present_brain_inspection(
            &inspection,
            chrono::Utc.with_ymd_and_hms(2026, 1, 1, 1, 1, 0).unwrap(),
        );
        assert_eq!(view.headline, "processing needs attention");
        assert_eq!(view.reason_text, "configuration invalid");
        assert_eq!(view.failing_component.as_deref(), Some("generate"));
        assert_eq!(view.evidence.age_text.as_deref(), Some("1h"));
    }

    fn view_of(state: &str, reason: &str, transition: bool) -> super::BrainPresentation {
        let inspection = BrainInspection {
            status: InspectionStatus::Ok,
            projection: BrainProjection {
                aggregate_state: state.into(),
                reason_code: Some(reason.into()),
                active_lane: None,
                active_provider: None,
                active_model: None,
                fingerprint_sha256: None,
                runtime_transition_in_progress: transition,
            },
            error: None,
            record: None,
        };
        present_brain_inspection(
            &inspection,
            chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        )
    }

    #[test]
    fn a_blocked_lane_that_is_only_waiting_is_not_told_to_set_up() {
        let setup = view_of("blocked", "thinking_engine_not_chosen", false);
        assert!(!setup.progressing);
        for reason in ["provider_key_missing", "endpoint_configuration_incomplete"] {
            assert_eq!(
                view_of("blocked", reason, false).headline,
                setup.headline,
                "{reason}"
            );
        }
        assert_eq!(
            view_of("blocked", "local_runtime_not_ready", false).headline,
            setup.headline
        );

        let unreachable = view_of("blocked", "attestation_not_verified", false);
        let installing = view_of("blocked", "nvattest_install_in_progress", false);
        let local = view_of("blocked", "local_runtime_not_ready", true);
        assert!(!unreachable.progressing);
        assert!(installing.progressing);
        assert!(local.progressing);
        let headlines = [&unreachable.headline, &installing.headline, &local.headline];
        for (index, headline) in headlines.iter().enumerate() {
            assert_ne!(**headline, setup.headline);
            assert!(headlines[index + 1..].iter().all(|other| other != headline));
        }
        for (reason, waiting) in [
            ("attestation_not_verified", &unreachable),
            ("nvattest_install_in_progress", &installing),
        ] {
            assert_ne!(waiting.reason_text, reason.replace('_', " "), "{reason}");
        }
    }

    #[test]
    fn a_failed_hardware_check_reads_like_the_other_failed_checks() {
        let failed = view_of("unhealthy", "attestation_rejected", false).headline;
        let view = view_of("blocked", "nvattest_unavailable", false);
        assert_eq!(view.headline, failed);
        assert!(!view.progressing);
        for reason in [
            "nvattest_platform_unsupported",
            "nvattest_unavailable",
            "nvattest_install_failed",
            "nvattest_integrity_failed",
        ] {
            let text = view_of("blocked", reason, false).reason_text;
            assert!(!text.contains("nvattest"), "{reason}: {text}");
        }
    }

    #[test]
    fn a_platform_with_no_hardware_check_says_so_on_every_brain_surface() {
        let view = view_of("blocked", "nvattest_platform_unsupported", false);
        assert_eq!(view.headline, super::NOT_ON_PLATFORM);
        assert_eq!(view.reason_text, view.headline);
        assert!(!view.progressing);
        assert_eq!(
            super::processing_headline_for_reason("nvattest_platform_unsupported"),
            Some(view.headline.as_str())
        );
        assert_ne!(
            view.headline,
            view_of("unhealthy", "attestation_rejected", false).headline
        );
    }

    #[test]
    fn brain_age_uses_the_shared_second_minute_hour_and_day_boundaries() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 1, 4, 0, 0, 0).unwrap();
        for (seconds, expected) in [
            (30, "30s"),
            (59, "59s"),
            (60, "1m"),
            (300, "5m"),
            (3_599, "59m"),
            (3_600, "1h"),
            (47 * 3_600, "47h"),
            (48 * 3_600, "2d"),
            (71 * 3_600, "2d"),
            (72 * 3_600, "3d"),
        ] {
            let observed = now - chrono::Duration::seconds(seconds);
            let (_, text) = brain_age(now, Some(&observed.to_rfc3339()));
            assert_eq!(text.as_deref(), Some(expected));
        }
        let future = now + chrono::Duration::seconds(30);
        assert_eq!(
            brain_age(now, Some(&future.to_rfc3339())),
            (Some(0), Some("0s".to_owned()))
        );
    }
}
