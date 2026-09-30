// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

/// The one owner-facing reason for a backlog day, shared by the health page
/// and the stats page so the same day never reads two ways.
pub fn backlog_day_reason_copy(day: &Map<String, Value>) -> &'static str {
    let marker = day_reason_marker(day);
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
    match category(marker) {
        "setup" => "a setting's missing. check your journal's setup",
        "key" => "your AI provider turned down your key. check it on the thinking page",
        "provider" | "startup" => "the AI provider was unreachable. try again",
        "request" => {
            "the AI provider refused a request. retrying won't help; this is a defect to report."
        }
        _ => "a processing step keeps failing. try again",
    }
}

fn day_reason_marker(day: &Map<String, Value>) -> Option<&str> {
    // A day's own repair or media state can require a different action from
    // the coded failure of another unit on the same day.
    let reason = day.get("reason").and_then(Value::as_str);
    if reason == Some("corrupt_raw") {
        return reason;
    }
    match day.get("segment_repair_status").and_then(Value::as_str) {
        Some("stuck") => return Some("segment_repair_stuck"),
        Some("unknown") => return Some("segment_repair_unknown"),
        _ => {}
    }
    if reason == Some("catchup_backoff") {
        return reason;
    }
    day.get("reason_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or(reason)
}

fn category(reason: Option<&str>) -> &'static str {
    match reason {
        Some("local_model_installing" | "local_model_loading" | "local_model_not_ready") => {
            "startup"
        }
        Some(
            "thinking_engine_not_chosen"
            | "provider_key_missing"
            | "ram_insufficient"
            | "gpu_unavailable"
            | "gpu_probe_failed"
            | "local_model_missing"
            | "model_missing"
            | "binary_missing"
            | "install_busy"
            | "unsupported_platform"
            | "host_unfit"
            | "unsupported_model"
            | "sha256_mismatch"
            | "archive_path_traversal"
            | "cuda_runtime_incomplete"
            | "model_not_found",
        ) => "setup",
        Some("local_artifact_proof_unavailable") => "runtime",
        // Trying again cannot fix a refused key; the owner has to replace it.
        Some("provider_key_invalid") => "key",
        Some(
            "local_server_unhealthy"
            | "local_endpoint_unreachable"
            | "provider_quota_exceeded"
            | "provider_unavailable",
        ) => "provider",
        Some("provider_request_rejected") => "request",
        Some(
            "local_endpoint_contract_failed"
            | "network_unreachable"
            | "provider_response_invalid"
            | "pipeline_unavailable"
            | "brain_refresh_timeout"
            | "local_queue_timeout"
            | "local_capacity_exhausted"
            | "context_window_exceeded"
            | "context_budget_exceeded"
            | "incomplete_json_length"
            | "incomplete_text_length"
            | "max_turns_exhausted"
            | "no_output"
            | "non_responsive"
            | "token_budget_exceeded"
            | "wall_clock_exceeded"
            | "unknown",
        ) => "generic",
        _ => "generic",
    }
}

#[cfg(test)]
mod tests {
    use super::{category, day_reason_marker};
    use serde_json::json;

    #[test]
    fn day_state_instructions_precede_another_units_failure_code() {
        for code in [
            "provider_unavailable",
            "provider_request_rejected",
            "no_output",
        ] {
            for (reason, repair, expected) in [
                ("corrupt_raw", None, "corrupt_raw"),
                ("corrupt_raw", Some("stuck"), "corrupt_raw"),
                ("failing_step", Some("stuck"), "segment_repair_stuck"),
                ("failing_step", Some("unknown"), "segment_repair_unknown"),
                ("catchup_backoff", None, "catchup_backoff"),
            ] {
                let day =
                    json!({"reason": reason, "reason_code": code, "segment_repair_status": repair});
                assert_eq!(day_reason_marker(day.as_object().unwrap()), Some(expected));
            }
        }
        for repair in ["progressing", "degraded", "finished", ""] {
            let day = json!({"reason":"failing_step", "reason_code":"provider_request_rejected", "segment_repair_status":repair});
            assert_eq!(
                day_reason_marker(day.as_object().unwrap()),
                Some("provider_request_rejected")
            );
        }
        let day = json!({"reason":"failing_step", "reason_code":"provider_request_rejected"});
        assert_eq!(
            day_reason_marker(day.as_object().unwrap()),
            Some("provider_request_rejected")
        );
        let day = json!({"reason":"segment_repair_stuck", "reason_code":""});
        assert_eq!(
            day_reason_marker(day.as_object().unwrap()),
            Some("segment_repair_stuck")
        );
        assert_eq!(day_reason_marker(json!({}).as_object().unwrap()), None);
    }

    #[test]
    fn provider_taxonomy_keeps_startup_distinct_but_renderable() {
        assert_eq!(category(Some("local_model_loading")), "startup");
        assert_eq!(category(Some("provider_key_missing")), "setup");
        assert_eq!(category(Some("provider_unavailable")), "provider");
        assert_eq!(category(Some("provider_key_invalid")), "key");
        assert_eq!(category(Some("provider_request_rejected")), "request");
        assert_eq!(
            category(Some("local_artifact_proof_unavailable")),
            "runtime"
        );
        assert_eq!(category(Some("unknown")), "generic");
    }
}
