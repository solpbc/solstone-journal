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
    pub reason_text: String,
    pub failing_component: Option<String>,
    pub evidence: BrainEvidencePresentation,
}

/// Render the stable owner-facing words for an inspected brain state.
pub fn present_brain_inspection(
    inspection: &BrainInspection,
    now: DateTime<Utc>,
) -> BrainPresentation {
    let reason = inspection.projection.reason_code.as_deref();
    let (mut failing_component, observed_at) = evidence_view(inspection.record.as_ref());
    if failing_component.is_none() {
        failing_component = component_for_reason(reason);
    }
    let (age_seconds, age_text) = brain_age(now, observed_at.as_deref());
    BrainPresentation {
        headline: headline(&inspection.projection.aggregate_state).to_owned(),
        reason_text: brain_reason_text(reason),
        failing_component,
        evidence: BrainEvidencePresentation {
            observed_at,
            age_seconds,
            age_text,
        },
    }
}

fn headline(state: &str) -> &'static str {
    match state {
        "ready" => "processing is ready",
        "checking" => "checking how processing runs",
        "blocked" => "processing needs a setup",
        "unhealthy" => "processing needs attention",
        _ => "thinking status unavailable",
    }
}

fn brain_reason_text(reason: Option<&str>) -> String {
    match reason {
        None => "ok".to_owned(),
        Some("thinking_engine_not_chosen") => "no thinking engine chosen".to_owned(),
        Some("configuration_invalid") => "configuration invalid".to_owned(),
        Some("stale_expected_fingerprint") => "stale expected fingerprint".to_owned(),
        Some("lost_fence") => "refresh fence lost".to_owned(),
        Some("busy") => "check already running".to_owned(),
        Some(reason) => reason.replace('_', " "),
    }
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
