// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only local-model projections.

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_brain::inspect_runtime_health;
use solstone_core_local::install::{
    readiness::{inspect_local, inspect_mlx},
    status::{is_in_flight, read_status},
};
use solstone_core_sense::memory::{MemoryProbe, SystemMemoryProbe};

pub const DEFAULT_MODEL: &str = "local/qwen3.5-4b";

pub fn accepted_model(model: Option<&str>) -> Option<&str> {
    let candidate = model
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_MODEL);
    (candidate == DEFAULT_MODEL).then_some(candidate)
}

pub fn invalid_model(model: &str) -> Value {
    json!({"error":"I couldn't use one of those values.","reason_code":"invalid_request_value","detail":format!("Unknown local model: {model}. Must be one of: {DEFAULT_MODEL}")})
}

pub fn models() -> Value {
    json!([{"name":DEFAULT_MODEL,"label":"qwen 3.5 4B VLM — 8 GB","min_ram_gb":8,"size_bytes":2_740_937_888_u64}])
}

pub fn availability(journal: &Path, model: &str) -> Value {
    let input = Map::from_iter([
        (
            String::from("journal"),
            Value::String(journal.display().to_string()),
        ),
        (String::from("model_id"), Value::String(model.to_owned())),
    ]);
    let readiness = if cfg!(target_os = "macos") {
        inspect_mlx(input)
    } else {
        inspect_local(input)
    };
    let host = &readiness["host"];
    let artifacts = &readiness["artifacts"];
    let platform_supported = host["platform_supported"].as_bool().unwrap_or(false);
    let binary_present = artifacts
        .get("binary_installed")
        .or_else(|| host.get("package_available"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let model_present = artifacts["model_installed"].as_bool().unwrap_or(false);
    let memory = SystemMemoryProbe;
    let total_memory_gb = memory.total_bytes().map(bytes_to_gb);
    let available_memory_gb = memory.available_bytes().map(bytes_to_gb);
    let (available, reason) = if !platform_supported {
        (
            false,
            "local thinking needs supported hardware on this computer.",
        )
    } else if !binary_present {
        (false, "local runtime is not installed")
    } else if !model_present {
        (false, "local model files are not installed")
    } else {
        (true, "")
    };
    json!({"model":model,"platform_supported":platform_supported,"total_memory_gb":total_memory_gb,"available_memory_gb":available_memory_gb,"min_ram_gb":8,"binary_present":binary_present,"model_present":model_present,"available":available,"reason":reason,"warning":"","download_bytes":3_413_361_504_u64})
}

pub fn bootstrap_status(journal: &Path, _model: &str) -> Value {
    match read_status(journal, "local") {
        Ok(status) => {
            json!({"name":status.provider,"install_state":status.install_state,"last_transition_at":status.last_transition_at,"last_progress_at":status.last_progress_at,"progress_bytes_received":if is_in_flight(&status.install_state) { status.progress_bytes_received } else { None },"progress_bytes_total":if is_in_flight(&status.install_state) { status.progress_bytes_total } else { None },"install_error":status.install_error})
        }
        Err(_) => {
            json!({"name":"local","install_state":"idle","last_transition_at":null,"last_progress_at":null,"progress_bytes_received":null,"progress_bytes_total":null,"install_error":null})
        }
    }
}

pub fn runtime(journal: &Path) -> Value {
    let inspection = inspect_runtime_health(journal);
    if inspection.status != "ok" {
        return json!({"status":inspection.status,"phase":if inspection.status == "corrupt" {"state-corrupt"} else {"state-unavailable"},"reason_code":inspection.reason_code,"health_revision":null,"desired_fingerprint_sha256":null,"retry_revision":null,"retry_pending":false,"can_retry":false,"poll":false,"updated_at":null});
    }
    let record = inspection.record.unwrap_or(Value::Null);
    let phase = record["phase"].as_str().unwrap_or("stopped");
    let poll = matches!(
        phase,
        "observing"
            | "artifact-not-ready"
            | "host-blocked"
            | "starting"
            | "warming"
            | "backoff"
            | "retry-requested"
            | "stop-deferred"
            | "stopping"
    );
    json!({"status":"ok","phase":phase,"reason_code":record["reason_code"],"health_revision":record["revision"],"desired_fingerprint_sha256":record["desired_fingerprint_sha256"],"retry_revision":0,"retry_pending":false,"can_retry":phase=="failed" && !record["desired_fingerprint_sha256"].is_null(),"poll":poll,"updated_at":record["updated_at"]})
}

fn bytes_to_gb(bytes: u64) -> f64 {
    ((bytes as f64 / 1024_f64.powi(3)) * 10.0).round() / 10.0
}
