// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use hmac::{Hmac, Mac};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_brain::{CanonicalInput, canonical_json};

use crate::nvidia::{
    ArtifactTrust, Backend, NvidiaProbe, hardware_backend_rejection, select_local_backend,
};

type HmacSha256 = Hmac<Sha256>;

pub fn canonical(value: Value) -> Result<String, String> {
    canonical_json(&CanonicalInput::Json(value)).map_err(|error| error.to_string())
}

pub fn sha256(text: &str) -> String {
    hex(Sha256::digest(text.as_bytes()))
}

pub fn hmac_sha256(key: &[u8], text: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts arbitrary keys");
    mac.update(text.as_bytes());
    hex(mac.finalize().into_bytes())
}

pub fn local_fingerprint(mut input: Map<String, Value>) -> Result<Value, String> {
    let probe: NvidiaProbe = input
        .remove("nvidia_probe")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(crate::probe_nvidia_gpu);
    let cuda_trust: ArtifactTrust = input
        .remove("cuda_trust")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or(ArtifactTrust::Unavailable);
    let backend = input
        .remove("backend")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .map(|value| match value.as_str() {
            "cuda" => Ok(Backend::Cuda),
            "vulkan" => Ok(Backend::Vulkan),
            _ => Err("invalid backend".to_owned()),
        })
        .transpose()?
        .unwrap_or_else(|| {
            select_local_backend(
                &probe,
                &crate::CUDA_EMBEDDED_ARCH_SET,
                crate::CUDA_MIN_DRIVER_VERSION,
                cuda_trust,
                false,
            )
            .backend
        });
    if let Some(reason) = hardware_backend_rejection(
        &probe,
        &crate::CUDA_EMBEDDED_ARCH_SET,
        crate::CUDA_MIN_DRIVER_VERSION,
    ) && backend == Backend::Cuda
    {
        return Err(reason.reason);
    }
    input.insert("provider".to_owned(), Value::String("local".to_owned()));
    input.insert(
        "backend".to_owned(),
        Value::String(
            match backend {
                Backend::Cuda => "cuda",
                Backend::Vulkan => "vulkan",
            }
            .to_owned(),
        ),
    );
    let canonical = canonical(Value::Object(input))?;
    Ok(
        json!({"target_fingerprint_json": canonical, "target_fingerprint_sha256": sha256(&canonical)}),
    )
}

pub fn mlx_fingerprint(mut input: Map<String, Value>) -> Result<Value, String> {
    input.insert("provider".to_owned(), Value::String("local".to_owned()));
    let canonical = canonical(Value::Object(input))?;
    Ok(
        json!({"target_fingerprint_json": canonical, "target_fingerprint_sha256": sha256(&canonical)}),
    )
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
