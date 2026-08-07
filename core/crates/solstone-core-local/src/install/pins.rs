// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const LLAMA_SERVER_PINS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "b10068",
        "llama-b10068-bin-macos-arm64.tar.gz",
        "13aa2d40c76ad1dcb8ebeec5f0d2814bf3b2f84a66935c7d4dc6f7cca8e38d68",
        "llama-server",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "b10068",
        "llama-b10068-bin-ubuntu-vulkan-x64.tar.gz",
        "713641920dce6c8efb953ebc9ffa309977e200cec5e182e6ad0e8b086203cdc3",
        "llama-server",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "b10068",
        "llama-b10068-bin-ubuntu-vulkan-arm64.tar.gz",
        "c3c49e6e124a574165ca28317be021b1a12a2ea06977e3eb7daee3eb443eb186",
        "llama-server",
    ),
];
pub const CUDA_ARTIFACTS: &[(&str, &str, &str, u64)] = &[
    (
        "x86_64-unknown-linux-gnu",
        "https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz",
        "3727630e6ac79953f5c652fddcfd7100da98c55d773c0aec115a55f40f3aafea",
        550238443,
    ),
    (
        "aarch64-unknown-linux-gnu",
        "https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-arm64-sol1.tar.gz",
        "6de68319db40e8c0eb45dc4bd3a45a16971dbdc128f2b621b19bef5dae87d064",
        654508507,
    ),
];
pub const MLX_MODELS: &[(&str, &str, &str, u64)] = &[
    (
        "qwen3.5:9b",
        "mlx-community/Qwen3.5-9B-MLX-8bit",
        "84f7c2deea248d8df56240f88102def51c7ed5d6",
        10453446077,
    ),
    (
        "gemma-4-26b-a4b-it-mlx-4bit",
        "mlx-community/gemma-4-26b-a4b-it-4bit",
        "efbeee6e582ebfd06abc9d65e90839c4b5d2116b",
        15641241224,
    ),
];

pub const CUDA_SHARED_WANTED_FILES: &[&str] = &[
    "llama-server",
    "libllama-server-impl.so",
    "libllama-common.so.0",
    "libmtmd.so.0",
    "libllama.so.0",
    "libggml.so.0",
    "libggml-base.so.0",
    "libggml-cuda.so",
    "libcudart.so.13",
    "libcublas.so.13",
    "libcublasLt.so.13",
];
pub const CUDA_AMD64_WANTED_FILES: &[&str] = &[
    "libggml-cpu-x64.so",
    "libggml-cpu-sse42.so",
    "libggml-cpu-sandybridge.so",
    "libggml-cpu-ivybridge.so",
    "libggml-cpu-piledriver.so",
    "libggml-cpu-haswell.so",
    "libggml-cpu-skylakex.so",
    "libggml-cpu-cannonlake.so",
    "libggml-cpu-cascadelake.so",
    "libggml-cpu-icelake.so",
    "libggml-cpu-cooperlake.so",
    "libggml-cpu-zen4.so",
    "libggml-cpu-alderlake.so",
    "libggml-cpu-sapphirerapids.so",
];
pub const CUDA_ARM64_WANTED_FILES: &[&str] = &[
    "libggml-cpu-armv8.0_1.so",
    "libggml-cpu-armv8.2_1.so",
    "libggml-cpu-armv8.2_2.so",
    "libggml-cpu-armv8.2_3.so",
    "libggml-cpu-armv8.6_1.so",
    "libggml-cpu-armv8.6_2.so",
    "libggml-cpu-armv9.2_1.so",
    "libggml-cpu-armv9.2_2.so",
];

pub fn cuda_runtime_arch(key: &str) -> Option<&'static str> {
    if key.starts_with("x86_64-") {
        Some("amd64")
    } else if key.starts_with("aarch64-") {
        Some("arm64")
    } else {
        None
    }
}
pub fn cuda_wanted_files(arch: &str) -> Option<Vec<String>> {
    let cpu = match arch {
        "amd64" => CUDA_AMD64_WANTED_FILES,
        "arm64" => CUDA_ARM64_WANTED_FILES,
        _ => return None,
    };
    Some(
        CUDA_SHARED_WANTED_FILES
            .iter()
            .chain(cpu)
            .map(|value| (*value).to_owned())
            .collect(),
    )
}
pub fn vulkan_pin(key: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    LLAMA_SERVER_PINS
        .iter()
        .find(|pin| pin.0 == key)
        .map(|pin| (pin.1, pin.2, pin.3, pin.4))
}
pub fn cuda_pin(key: &str) -> Option<(&'static str, &'static str, u64)> {
    CUDA_ARTIFACTS
        .iter()
        .find(|pin| pin.0 == key)
        .map(|pin| (pin.1, pin.2, pin.3))
}
pub fn vulkan_identity(key: &str) -> Option<Value> {
    vulkan_pin(key).map(|(release_tag, filename, sha256, binary_name)| json!({"unit":"llama-server-vulkan","artifact_key":key,"release_tag":release_tag,"filename":filename,"sha256":sha256,"binary_name":binary_name}))
}
pub fn cuda_identity(key: &str) -> Option<Value> {
    let (url, sha256, size_bytes) = cuda_pin(key)?;
    let arch = cuda_runtime_arch(key)?;
    let wanted_files = cuda_wanted_files(arch)?;
    Some(
        json!({"unit":"llama-server-cuda","artifact_key":key,"url":url,"sha256":sha256,"size_bytes":size_bytes,"release_tag":"b10068","upstream_image_digest":"sha256:5bd5290bd35cfde893d0dcbd9811723c16d89575927d537b5f21becbfbab2f63","llama_cpp_revision":"571d0d540df04f25298d0e159e520d9fc62ed121","repack_revision":"sol1","arch":arch,"binary_name":"llama-server","wanted_files":wanted_files}),
    )
}
pub fn model_identity(model_id: &str) -> Option<Value> {
    (model_id == "local/qwen3.5-4b").then(|| json!({"unit":"local-model","model_id":"local/qwen3.5-4b","repo":"unsloth/Qwen3.5-4B-GGUF","revision":"main","filename":"Qwen3.5-4B-Q4_K_M.gguf","sha256":"00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4","mmproj_filename":"mmproj-F16.gguf","mmproj_sha256":"cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864"}))
}

pub fn cache_root(journal: &Path) -> PathBuf {
    journal.join("cache/providers/local")
}
pub fn platform_key() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => format!("{arch}-apple-darwin"),
        "linux" => format!("{arch}-unknown-linux-gnu"),
        other => format!("{arch}-{other}"),
    }
}
pub fn paths(journal: &Path, key: &str, model_id: Option<&str>) -> Value {
    let vulkan = LLAMA_SERVER_PINS.iter().find(|pin| pin.0 == key);
    let cuda = CUDA_ARTIFACTS.iter().find(|pin| pin.0 == key);
    let root = cache_root(journal);
    json!({
        "artifact_key": key,
        "cache_root": root,
        "binary_path": vulkan.map(|pin| root.join("bin").join(key).join(pin.1).join(pin.4)),
        "cuda_binary_path": cuda.map(|pin| root.join("cuda").join(key).join(pin.2).join("llama-server")),
        "model_dir": model_id.map(|id| root.join("models").join(id.replace('/', "__"))),
    })
}
pub fn pins_json() -> Value {
    json!({"llama_server_pins": LLAMA_SERVER_PINS.iter().map(|p| json!({"artifact_key":p.0,"release_tag":p.1,"filename":p.2,"sha256":p.3,"binary_name":p.4})).collect::<Vec<_>>(), "cuda_server_pin":{"cuda_version":13,"embedded_arch_set":["sm_86","sm_89","sm_120a","sm_121a"],"binary_name":"llama-server","device_flag_value":"CUDA0","visible_devices_env":"CUDA_VISIBLE_DEVICES","shared_wanted_files":CUDA_SHARED_WANTED_FILES,"cpu_wanted_files_by_arch":{"amd64":CUDA_AMD64_WANTED_FILES,"arm64":CUDA_ARM64_WANTED_FILES},"artifacts":CUDA_ARTIFACTS.iter().map(|p| cuda_identity(p.0).unwrap()).collect::<Vec<_>>()}, "mlx_models":MLX_MODELS.iter().map(|p| json!({"name":p.0,"repo":p.1,"revision":p.2,"size_bytes":p.3})).collect::<Vec<_>>(), "mlx_soft_token_budget":1120})
}
