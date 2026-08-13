// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only local-model projections.

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_brain::inspect_runtime_health;
use solstone_core_local::install::{
    lease::is_held,
    readiness::{inspect_local, inspect_mlx},
    status::{is_in_flight, read_status},
};
use solstone_core_sense::memory::{MemoryProbe, SystemMemoryProbe};

pub const DEFAULT_MODEL: &str = "local/qwen3.5-4b";
const MLX_MODEL: &str = "qwen3.5:9b";
const MLX_MODEL_SIZE_BYTES: u64 = 10_453_446_077;
const MLX_MIN_RAM_GB: u64 = 13;

pub fn default_model() -> &'static str {
    if cfg!(target_os = "macos") {
        MLX_MODEL
    } else {
        DEFAULT_MODEL
    }
}

pub fn accepted_model(model: Option<&str>) -> Option<&'static str> {
    let candidate = model
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_MODEL);
    if cfg!(target_os = "macos") {
        matches!(candidate, DEFAULT_MODEL | MLX_MODEL).then_some(MLX_MODEL)
    } else {
        (candidate == DEFAULT_MODEL).then_some(DEFAULT_MODEL)
    }
}

pub fn invalid_model(model: &str) -> Value {
    json!({"error":"I couldn't use one of those values.","reason_code":"invalid_request_value","detail":format!("Unknown local model: {model}. Must be one of: {}", default_model())})
}

pub fn models() -> Value {
    if cfg!(target_os = "macos") {
        return json!([{"name":MLX_MODEL,"label":"qwen 3.5 9B VLM — 13 GB","min_ram_gb":MLX_MIN_RAM_GB,"size_bytes":MLX_MODEL_SIZE_BYTES}]);
    }
    json!([{"name":DEFAULT_MODEL,"label":"qwen 3.5 4B VLM — 8 GB","min_ram_gb":8,"size_bytes":2_740_937_888_u64}])
}

pub fn availability(journal: &Path, model: &str) -> Value {
    let mut input = Map::from_iter([
        (
            String::from("journal"),
            Value::String(journal.display().to_string()),
        ),
        (String::from("model_id"), Value::String(model.to_owned())),
    ]);
    let readiness = if cfg!(target_os = "macos") {
        // `inspect_mlx` requires `mlx_vlm_importable`; Rust has no reader for
        // that Python-package observation, so binary_present/available remain
        // unavailable until one exists.
        input.insert(
            "platform_supported".to_owned(),
            Value::Bool(cfg!(target_arch = "aarch64")),
        );
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
    let min_ram_gb = if cfg!(target_os = "macos") {
        MLX_MIN_RAM_GB
    } else {
        8
    };
    let download_bytes = if cfg!(target_os = "macos") {
        MLX_MODEL_SIZE_BYTES
    } else {
        3_413_361_504_u64
    };
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
    json!({"model":model,"platform_supported":platform_supported,"total_memory_gb":total_memory_gb,"available_memory_gb":available_memory_gb,"min_ram_gb":min_ram_gb,"binary_present":binary_present,"model_present":model_present,"available":available,"reason":reason,"warning":"","download_bytes":download_bytes})
}

pub fn bootstrap_status(journal: &Path, _model: &str) -> Value {
    match read_status(journal, "local") {
        Ok(mut status) => {
            if is_in_flight(&status.install_state) && matches!(is_held(journal, "local"), Ok(false))
            {
                status.install_state = "failed".to_owned();
                status.install_error = Some("install_interrupted".to_owned());
            }
            json!({"name":status.provider,"install_state":status.install_state,"last_transition_at":status.last_transition_at,"last_progress_at":status.last_progress_at,"progress_bytes_received":if is_in_flight(&status.install_state) { status.progress_bytes_received } else { None },"progress_bytes_total":if is_in_flight(&status.install_state) { status.progress_bytes_total } else { None },"install_error":status.install_error})
        }
        Err(_) => {
            json!({"name":"local","install_state":"idle","last_transition_at":null,"last_progress_at":null,"progress_bytes_received":null,"progress_bytes_total":null,"install_error":null})
        }
    }
}

pub fn runtime(journal: &Path) -> Value {
    // Runtime retry tokens have only a private parser in solstone-core-system;
    // this projection needs a public read-only inspector to expose their state.
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
