// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::json;
use solstone_core_local::install::{manifest, pins};

pub fn install(journal: &Path, model_marker: &str) {
    let cache = pins::cache_root(journal);
    let key = "aarch64-apple-darwin";
    let (release, _, _, _) = pins::vulkan_pin(key).expect("Darwin runtime pin");
    let runtime = cache.join("bin").join(key).join(release);
    let model = cache.join("models/local__qwen3.5-4b");
    std::fs::create_dir_all(&runtime).expect("runtime directory");
    std::fs::create_dir_all(&model).expect("model directory");
    let binary_path = runtime.join("llama-server");
    std::fs::copy(crate::fixture_binary::path(), &binary_path).expect("fixture runtime");
    let model_path = model.join("Qwen3.5-4B-Q4_K_M.gguf");
    std::fs::write(&model_path, model_marker).expect("fixture model");
    let projector_path = model.join("mmproj-F16.gguf");
    std::fs::write(&projector_path, b"projector").expect("fixture projector");
    let runtime_manifest = manifest::build_manifest(
        "local",
        "llama-server-vulkan",
        "test",
        json!({"pin_identity":pins::vulkan_identity(key).unwrap()}),
        manifest::runtime_inventory(&runtime, &[]).unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(
        &manifest::artifact_manifest_path(&runtime),
        &runtime_manifest,
    )
    .unwrap();
    let model_manifest = manifest::build_manifest(
        "local",
        "local-model",
        "test",
        json!({"pin_identity":pins::model_identity("local/qwen3.5-4b").unwrap()}),
        manifest::inventory_for_tree(&model, "model").unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(&manifest::artifact_manifest_path(&model), &model_manifest).unwrap();
}
