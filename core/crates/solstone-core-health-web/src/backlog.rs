// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::backlog_reasons;
use serde_json::{Map, Value, json};

pub fn load(root: &std::path::Path) -> (Option<String>, Option<Map<String, Value>>) {
    let Ok(bytes) = std::fs::read(root.join("stats.json")) else {
        return (None, None);
    };
    let Some(obj) = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.as_object().cloned())
    else {
        return (None, None);
    };
    let generated_at = obj
        .get("generated_at")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let backlog = obj.get("backlog").and_then(Value::as_object).cloned();
    (generated_at, backlog)
}

pub fn count(value: Option<&Value>) -> f64 {
    let Some(value) = value else {
        return 0.0;
    };
    let value = match value {
        Value::Bool(value) => Some(f64::from(*value)),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    };
    value
        .filter(|n: &f64| n.is_finite() && *n > 0.0)
        .unwrap_or(0.0)
}

fn reason(day: &Map<String, Value>) -> &'static str {
    let marker = day
        .get("reason_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| day.get("reason").and_then(Value::as_str));
    match marker {
        Some("catchup_backoff") => return "waiting to retry automatically. no action needed yet",
        Some("context_preserved_overflow" | "context_fitted_overflow") => {
            return "this day's remaining content still will not fit the on-device model after trimming. it will keep retrying";
        }
        Some("segment_repair_progressing") => return "repairing itself. check back soon",
        Some("segment_repair_degraded") => {
            return "repair is having trouble keeping up. may need a hand";
        }
        Some("segment_repair_stuck") => return "repair has stalled. try again",
        Some("segment_repair_unknown") => return "repair status is unclear right now",
        _ => {}
    }
    if marker == Some("corrupt_raw") {
        return "original raw media is missing or damaged. re-import it";
    }
    match backlog_reasons::category(marker) {
        "setup" => "a setting's missing. check your journal's setup",
        "provider" | "startup" => "the AI provider was unreachable. try again",
        "request" => {
            "the AI provider refused a request. retrying won't help; this is a defect to report."
        }
        _ => "a processing step keeps failing. try again",
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
}

pub fn stuck_rows(backlog: Option<&Map<String, Value>>) -> Vec<Value> {
    let Some(backlog) = backlog else {
        return Vec::new();
    };
    if backlog.get("degraded") == Some(&Value::Bool(true)) {
        return Vec::new();
    }
    let errors = backlog.get("errors").and_then(Value::as_array);
    backlog
        .get("days")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|day| {
            let error = day.get("error").filter(|value| truthy(value)).or_else(|| {
                errors
                    .and_then(|errors| errors.iter().find(|item| item.get("day") == day.get("day")))
            });
            if day.get("state").and_then(Value::as_str) != Some("stuck") && error.is_none() {
                return None;
            }
            let depth = count(day.get("segments")) + count(day.get("units"));
            let mut row = Map::new();
            row.insert(
                "day".to_owned(),
                day.get("day").cloned().unwrap_or(Value::Null),
            );
            row.insert("reason".to_owned(), Value::String(reason(day).to_owned()));
            row.insert(
                "depth".to_owned(),
                if depth > 0.0 {
                    if depth.fract() == 0.0 {
                        json!(depth as i64)
                    } else {
                        json!(depth)
                    }
                } else {
                    Value::Null
                },
            );
            for key in ["reason_code", "provider", "model"] {
                if day.get(key).is_some_and(truthy) {
                    row.insert(key.to_owned(), day[key].clone());
                }
            }
            Some(Value::Object(row))
        })
        .collect()
}

pub fn copy() -> Value {
    json!({
        "bucket_heading": "days that need a hand",
        "bucket_description": "some days retry automatically; others need your help. each day shows its current status.",
        "day_badge": "stuck",
        "action_process_now": "process now",
        "action_redo_scratch": "redo from scratch",
        "confirm_redo_scratch": "redo this whole day from scratch? this re-does the parts already finished, so it'll take longer. the day you see now won't change until it's done.",
        "queued_feedback": "queued, working on it now",
        "unfinished_template_one": solstone_core_system_health::UNFINISHED_TEMPLATE_ONE,
        "unfinished_template_many_one_day": solstone_core_system_health::UNFINISHED_TEMPLATE_MANY_ONE_DAY,
        "unfinished_template_many_days": solstone_core_system_health::UNFINISHED_TEMPLATE_MANY_DAYS,
    })
}

#[cfg(test)]
mod tests {
    use super::{count, stuck_rows};
    use serde_json::json;

    #[test]
    fn retry_and_repair_states_keep_their_distinct_recovery_guidance() {
        for (code, expected) in [
            (
                "catchup_backoff",
                "waiting to retry automatically. no action needed yet",
            ),
            (
                "segment_repair_progressing",
                "repairing itself. check back soon",
            ),
            (
                "segment_repair_degraded",
                "repair is having trouble keeping up. may need a hand",
            ),
            ("segment_repair_stuck", "repair has stalled. try again"),
            (
                "segment_repair_unknown",
                "repair status is unclear right now",
            ),
        ] {
            let backlog = json!({"days":[{"day":"20260904","state":"stuck","reason_code":code}]});
            let rows = stuck_rows(backlog.as_object());
            assert_eq!(rows[0]["reason"], expected, "{code}");
            assert_eq!(rows[0]["reason_code"], code);
        }
    }

    #[test]
    fn count_matches_python_coercions() {
        assert_eq!(count(None), 0.0);
        assert_eq!(count(Some(&json!("nope"))), 0.0);
        assert_eq!(count(Some(&json!(f64::INFINITY))), 0.0);
        assert_eq!(count(Some(&json!(-2))), 0.0);
        assert_eq!(count(Some(&json!(true))), 1.0);
        assert_eq!(count(Some(&json!(false))), 0.0);
        assert_eq!(count(Some(&json!(2.5))), 2.5);
        assert_eq!(count(Some(&json!("3"))), 3.0);
    }

    #[test]
    fn rows_map_startup_and_provider_to_the_same_copy() {
        let rows = stuck_rows(json!({"days":[{"day":"20240101","state":"stuck","reason_code":"local_model_loading","segments":1},{"day":"20240102","state":"stuck","reason_code":"provider_unavailable","units":2},{"day":"20240103","state":"stuck","reason_code":"provider_request_rejected"}]}).as_object());
        assert_eq!(
            rows[0]["reason"],
            "the AI provider was unreachable. try again"
        );
        assert_eq!(
            rows[1]["reason"],
            "the AI provider was unreachable. try again"
        );
        assert_eq!(
            rows[2]["reason"],
            "the AI provider refused a request. retrying won't help; this is a defect to report."
        );
        let generic = stuck_rows(
            json!({"days":[{"day":"20240104","state":"stuck","reason_code":"local_artifact_proof_unavailable"},{"day":"20240105","state":"stuck","reason_code":"not_in_taxonomy"}]}).as_object(),
        );
        assert_eq!(
            generic[0]["reason"],
            "a processing step keeps failing. try again"
        );
        assert_eq!(
            generic[1]["reason"],
            "a processing step keeps failing. try again"
        );
    }

    #[test]
    fn falsey_day_errors_fall_through_to_backlog_errors() {
        for error in [json!(false), json!("")] {
            let without_fallback = json!({"days":[{"day":"20240101","error":error.clone()}]});
            assert!(stuck_rows(without_fallback.as_object()).is_empty());

            let with_fallback =
                json!({"days":[{"day":"20240101","error":error}],"errors":[{"day":"20240101"}]});
            assert_eq!(stuck_rows(with_fallback.as_object()).len(), 1);
        }
    }

    #[test]
    fn context_overflow_stuck_rows_render_specific_retry_sentence() {
        let expected = "this day's remaining content still will not fit the on-device model after trimming. it will keep retrying";
        let generic = "a processing step keeps failing. try again";
        for code in ["context_preserved_overflow", "context_fitted_overflow"] {
            let backlog = json!({"days":[{"day":"20260904","state":"stuck","reason_code":code}]});
            let rows = stuck_rows(backlog.as_object());
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["reason"], expected, "{code}");
            assert_ne!(rows[0]["reason"], generic, "{code}");
            assert_eq!(rows[0]["reason_code"], code);
        }
    }
}
