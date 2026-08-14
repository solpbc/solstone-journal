// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub fn category(reason: Option<&str>) -> &'static str {
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
        Some(
            "local_server_unhealthy"
            | "local_endpoint_unreachable"
            | "provider_key_invalid"
            | "provider_quota_exceeded"
            | "provider_unavailable",
        ) => "provider",
        Some("provider_request_rejected") => "request",
        Some(
            "local_endpoint_contract_failed"
            | "network_unreachable"
            | "provider_response_invalid"
            | "chat_pipeline_unavailable"
            | "chat_timeout"
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
    use super::category;

    #[test]
    fn provider_taxonomy_keeps_startup_distinct_but_renderable() {
        assert_eq!(category(Some("local_model_loading")), "startup");
        assert_eq!(category(Some("provider_key_missing")), "setup");
        assert_eq!(category(Some("provider_unavailable")), "provider");
        assert_eq!(category(Some("provider_request_rejected")), "request");
        assert_eq!(
            category(Some("local_artifact_proof_unavailable")),
            "runtime"
        );
        assert_eq!(category(Some("unknown")), "generic");
    }
}
