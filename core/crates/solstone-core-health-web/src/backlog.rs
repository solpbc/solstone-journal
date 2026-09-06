// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::backlog_reasons;
use serde_json::{Map, Value, json};

const CANT_TELL: &str = "still checking where your journal stands.";
const CAUGHT_UP: &str = "your journal's all caught up.";

pub fn load(root: &std::path::Path) -> Option<Map<String, Value>> {
    serde_json::from_slice::<Value>(&std::fs::read(root.join("stats.json")).ok()?)
        .ok()?
        .as_object()?
        .get("backlog")?
        .as_object()
        .cloned()
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

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

pub fn verdict(backlog: Option<&Map<String, Value>>) -> String {
    let Some(backlog) = backlog else {
        return CANT_TELL.to_owned();
    };
    if backlog.get("degraded") == Some(&Value::Bool(true)) {
        return CANT_TELL.to_owned();
    }
    let pending = count(backlog.get("pending_days"));
    let stuck = count(backlog.get("stuck_days"));
    if pending == 0.0 && stuck == 0.0 {
        return CAUGHT_UP.to_owned();
    }
    if stuck > 0.0 && pending == 0.0 {
        return if stuck == 1.0 {
            "caught up except 1 day that needs a hand.".to_owned()
        } else {
            format!("caught up except {} days that need a hand.", number(stuck))
        };
    }
    if stuck == 0.0 {
        return if pending == 1.0 {
            "1 day is still catching up.".to_owned()
        } else {
            format!("{} days are still catching up.", number(pending))
        };
    }
    let stuck = if stuck == 1.0 {
        "1 day needs a hand".to_owned()
    } else {
        format!("{} days need a hand", number(stuck))
    };
    let pending = if pending == 1.0 {
        "1 more day is still catching up".to_owned()
    } else {
        format!("{} more days are still catching up", number(pending))
    };
    format!("{stuck}. {pending}.")
}

/// How many days are still catching up, so the surface can name the one day
/// rather than saying "1 day".
pub fn pending_days(backlog: Option<&Map<String, Value>>) -> f64 {
    let Some(backlog) = backlog else {
        return 0.0;
    };
    if backlog.get("degraded") == Some(&Value::Bool(true)) {
        return 0.0;
    }
    count(backlog.get("pending_days"))
}

/// The oldest day still catching up, as its `YYYYMMDD` key. The surface formats
/// it through the shared date helper; this never returns a rendered label.
pub fn oldest_pending_day(backlog: Option<&Map<String, Value>>) -> Value {
    let Some(backlog) = backlog else {
        return Value::Null;
    };
    if backlog.get("degraded") == Some(&Value::Bool(true)) {
        return Value::Null;
    }
    backlog
        .get("oldest_pending_day")
        .filter(|value| value.as_str().is_some_and(|day| !day.is_empty()))
        .cloned()
        .unwrap_or(Value::Null)
}

fn reason(day: &Map<String, Value>) -> &'static str {
    let marker = day
        .get("reason_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| day.get("reason").and_then(Value::as_str));
    match marker {
        Some("catchup_backoff") => return "waiting to retry automatically. no action needed yet",
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
    json!({"bucket_heading":"days that need a hand","bucket_description":"some days retry automatically; others need your help. each day shows its current status.","day_badge":"stuck","action_process_now":"process now","action_redo_scratch":"redo from scratch","confirm_redo_scratch":"redo this whole day from scratch? this re-does the parts already finished, so it'll take longer. the day you see now won't change until it's done.","queued_feedback":"queued, working on it now"})
}

#[cfg(test)]
mod tests {
    use super::{count, oldest_pending_day, pending_days, stuck_rows, verdict};
    use serde_json::{Value, json};

    #[test]
    fn the_pending_day_is_carried_as_a_key_and_withheld_when_undecided() {
        let backlog = json!({"pending_days":1,"stuck_days":0,"oldest_pending_day":"20260904"});
        assert_eq!(pending_days(backlog.as_object()), 1.0);
        assert_eq!(oldest_pending_day(backlog.as_object()), json!("20260904"));

        // A degraded read knows nothing, so it names no day and counts no days.
        let degraded = json!({"pending_days":3,"oldest_pending_day":"20260904","degraded":true});
        assert_eq!(pending_days(degraded.as_object()), 0.0);
        assert_eq!(oldest_pending_day(degraded.as_object()), Value::Null);

        for absent in [json!({"pending_days":2}), json!({"oldest_pending_day":""})] {
            assert_eq!(oldest_pending_day(absent.as_object()), Value::Null);
        }
        assert_eq!(oldest_pending_day(None), Value::Null);
        assert_eq!(pending_days(None), 0.0);
    }

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
    fn verdict_covers_each_numeric_arm_and_degraded() {
        assert_eq!(verdict(None), "still checking where your journal stands.");
        assert_eq!(
            verdict(json!({"pending_days":1,"stuck_days":0}).as_object()),
            "1 day is still catching up."
        );
        assert_eq!(
            verdict(json!({"pending_days":0,"stuck_days":2}).as_object()),
            "caught up except 2 days that need a hand."
        );
        assert_eq!(
            verdict(json!({"pending_days":2,"stuck_days":1}).as_object()),
            "1 day needs a hand. 2 more days are still catching up."
        );
        assert_eq!(
            verdict(json!({"pending_days":2,"stuck_days":3,"degraded":true}).as_object()),
            "still checking where your journal stands."
        );
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
}
