// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[cfg(test)]
use solstone_core_assets::{Platform, resolve};

#[cfg(test)]
use super::archive;

pub const LLAMA_SERVER_PINS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "b11429",
        "llama-b11429-bin-macos-arm64.tar.gz",
        "740288ec6887be94280a5dfa25b5e23a78285cab104519e6c7e218904ee82459",
        "llama-server",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "b11429",
        "llama-b11429-bin-ubuntu-vulkan-x64.tar.gz",
        "632c4e98feba2b94407a2130e3133e0c3aefb0ea1ab41337e926d8bfafdd0b74",
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
        "https://updates.solstone.app/runtimes/llama-cuda13/b11429/llama-b11429-bin-linux-cuda13-amd64-sol1.tar.gz",
        "a9d8c0a4ece9f9dce7d8e634dd55f943ba39b93b339462dd645202db34aafbbd",
        591752886,
    ),
    (
        "aarch64-unknown-linux-gnu",
        "https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-arm64-sol1.tar.gz",
        "6de68319db40e8c0eb45dc4bd3a45a16971dbdc128f2b621b19bef5dae87d064",
        654508507,
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequiredMember {
    Regular(&'static str),
    Link {
        name: &'static str,
        target: &'static str,
    },
}

pub const VULKAN_LINUX_X64_B11429_REQUIRED: &[RequiredMember] = &[
    RequiredMember::Regular("LICENSE"),
    RequiredMember::Link {
        name: "libggml-base.so",
        target: "libggml-base.so.0",
    },
    RequiredMember::Link {
        name: "libggml-base.so.0",
        target: "libggml-base.so.0.26.0",
    },
    RequiredMember::Regular("libggml-base.so.0.26.0"),
    RequiredMember::Regular("libggml-cpu-alderlake.so"),
    RequiredMember::Regular("libggml-cpu-cannonlake.so"),
    RequiredMember::Regular("libggml-cpu-cascadelake.so"),
    RequiredMember::Regular("libggml-cpu-cooperlake.so"),
    RequiredMember::Regular("libggml-cpu-haswell.so"),
    RequiredMember::Regular("libggml-cpu-icelake.so"),
    RequiredMember::Regular("libggml-cpu-ivybridge.so"),
    RequiredMember::Regular("libggml-cpu-piledriver.so"),
    RequiredMember::Regular("libggml-cpu-sandybridge.so"),
    RequiredMember::Regular("libggml-cpu-sapphirerapids.so"),
    RequiredMember::Regular("libggml-cpu-skylakex.so"),
    RequiredMember::Regular("libggml-cpu-sse42.so"),
    RequiredMember::Regular("libggml-cpu-x64.so"),
    RequiredMember::Regular("libggml-rpc.so"),
    RequiredMember::Regular("libggml-vulkan.so"),
    RequiredMember::Link {
        name: "libggml.so",
        target: "libggml.so.0",
    },
    RequiredMember::Link {
        name: "libggml.so.0",
        target: "libggml.so.0.26.0",
    },
    RequiredMember::Regular("libggml.so.0.26.0"),
    RequiredMember::Link {
        name: "libllama-common.so",
        target: "libllama-common.so.0",
    },
    RequiredMember::Link {
        name: "libllama-common.so.0",
        target: "libllama-common.so.0.6.0",
    },
    RequiredMember::Regular("libllama-common.so.0.6.0"),
    RequiredMember::Regular("libllama-server-impl.so"),
    RequiredMember::Link {
        name: "libllama.so",
        target: "libllama.so.0",
    },
    RequiredMember::Link {
        name: "libllama.so.0",
        target: "libllama.so.0.6.0",
    },
    RequiredMember::Regular("libllama.so.0.6.0"),
    RequiredMember::Link {
        name: "libmtmd.so",
        target: "libmtmd.so.0",
    },
    RequiredMember::Link {
        name: "libmtmd.so.0",
        target: "libmtmd.so.0.6.0",
    },
    RequiredMember::Regular("libmtmd.so.0.6.0"),
    RequiredMember::Regular("llama-server"),
];

pub const METAL_MACOS_ARM64_B11429_REQUIRED: &[RequiredMember] = &[
    RequiredMember::Regular("LICENSE"),
    RequiredMember::Regular("libggml-base.0.26.0.dylib"),
    RequiredMember::Link {
        name: "libggml-base.0.dylib",
        target: "libggml-base.0.26.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-base.dylib",
        target: "libggml-base.0.dylib",
    },
    RequiredMember::Regular("libggml-blas.0.26.0.dylib"),
    RequiredMember::Link {
        name: "libggml-blas.0.dylib",
        target: "libggml-blas.0.26.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-blas.dylib",
        target: "libggml-blas.0.dylib",
    },
    RequiredMember::Regular("libggml-cpu.0.26.0.dylib"),
    RequiredMember::Link {
        name: "libggml-cpu.0.dylib",
        target: "libggml-cpu.0.26.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-cpu.dylib",
        target: "libggml-cpu.0.dylib",
    },
    RequiredMember::Regular("libggml-metal.0.26.0.dylib"),
    RequiredMember::Link {
        name: "libggml-metal.0.dylib",
        target: "libggml-metal.0.26.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-metal.dylib",
        target: "libggml-metal.0.dylib",
    },
    RequiredMember::Regular("libggml-rpc.0.26.0.dylib"),
    RequiredMember::Link {
        name: "libggml-rpc.0.dylib",
        target: "libggml-rpc.0.26.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-rpc.dylib",
        target: "libggml-rpc.0.dylib",
    },
    RequiredMember::Regular("libggml.0.26.0.dylib"),
    RequiredMember::Link {
        name: "libggml.0.dylib",
        target: "libggml.0.26.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml.dylib",
        target: "libggml.0.dylib",
    },
    RequiredMember::Regular("libllama-common.0.6.0.dylib"),
    RequiredMember::Link {
        name: "libllama-common.0.dylib",
        target: "libllama-common.0.6.0.dylib",
    },
    RequiredMember::Link {
        name: "libllama-common.dylib",
        target: "libllama-common.0.dylib",
    },
    RequiredMember::Regular("libllama-server-impl.dylib"),
    RequiredMember::Regular("libllama.0.6.0.dylib"),
    RequiredMember::Link {
        name: "libllama.0.dylib",
        target: "libllama.0.6.0.dylib",
    },
    RequiredMember::Link {
        name: "libllama.dylib",
        target: "libllama.0.dylib",
    },
    RequiredMember::Regular("libmtmd.0.6.0.dylib"),
    RequiredMember::Link {
        name: "libmtmd.0.dylib",
        target: "libmtmd.0.6.0.dylib",
    },
    RequiredMember::Link {
        name: "libmtmd.dylib",
        target: "libmtmd.0.dylib",
    },
    RequiredMember::Regular("llama-server"),
];

pub const B10068_REQUIRED: &[RequiredMember] = &[RequiredMember::Regular("llama-server")];

pub fn required_members_for(
    release_tag: &str,
    artifact_key: &str,
) -> Option<&'static [RequiredMember]> {
    match (release_tag, artifact_key) {
        ("b11429", "x86_64-unknown-linux-gnu") => Some(VULKAN_LINUX_X64_B11429_REQUIRED),
        ("b11429", "aarch64-apple-darwin") => Some(METAL_MACOS_ARM64_B11429_REQUIRED),
        ("b10068", _) => Some(B10068_REQUIRED),
        _ => None,
    }
}
// Parakeet pins mirror LLAMA_SERVER_PINS's (artifact_key, release_tag,
// filename, sha256, binary_name) shape, split by backend the same way
// LLAMA_SERVER_PINS (vulkan) and CUDA_ARTIFACTS (cuda) are split -- one
// array per backend, keyed by arch -- rather than adding a backend column
// to a single array.
pub const PARAKEET_VULKAN_PINS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "x86_64-unknown-linux-gnu",
        "v0.5.0",
        "parakeet-v0.5.0-bin-linux-vulkan-x64.tar.gz",
        "36c8d4b93594ec18928c9c76b02e04b2d738e859deda8b5e3944bb34fc0646eb",
        "parakeet-server",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "v0.5.0",
        "parakeet-v0.5.0-bin-linux-vulkan-arm64.tar.gz",
        "b95483070eb87ed144b9f39826a69fb67ea516c68aacc4fcf13a121a746ad7e4",
        "parakeet-server",
    ),
];
pub const PARAKEET_CPU_PINS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "x86_64-unknown-linux-gnu",
        "v0.5.0",
        "parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz",
        "636a9fc48ac023096037790f9b77d7e5043b200dd6399ec0438bd648c35d79b9",
        "parakeet-server",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "v0.5.0",
        "parakeet-v0.5.0-bin-linux-cpu-arm64.tar.gz",
        "a7c9064c64b84f6b041252d5d2334d4a47693636e9c7c6ab2c535fcef11cf88b",
        "parakeet-server",
    ),
];
/// One model, shared by every arch/backend: (repo, filename, revision, sha256, size_bytes).
pub const PARAKEET_MODEL: (&str, &str, &str, &str, u64) = (
    "mudler/parakeet-cpp-gguf",
    "tdt-0.6b-v3-q8_0.gguf",
    "bf0af9f425fa01809cadec671b3cb672709d13e9",
    "4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757",
    940663680,
);

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
/// The unit is the mirrored origin namespace, not a backend assertion about
/// the archive selected for this platform.
pub fn vulkan_identity(key: &str) -> Option<Value> {
    vulkan_pin(key).map(|(release_tag, filename, sha256, binary_name)| json!({"unit":"llama-server-vulkan","artifact_key":key,"release_tag":release_tag,"filename":filename,"sha256":sha256,"binary_name":binary_name}))
}
pub fn cuda_identity(key: &str) -> Option<Value> {
    let (url, sha256, size_bytes) = cuda_pin(key)?;
    let arch = cuda_runtime_arch(key)?;
    let wanted_files = cuda_wanted_files(arch)?;
    if arch == "amd64" {
        // The signed publisher revision is not the runtime source.
        Some(json!({
            "unit": "llama-server-cuda",
            "artifact_key": key,
            "url": url,
            "sha256": sha256,
            "size_bytes": size_bytes,
            "release_tag": "b11429",
            "llama_cpp_revision": "d81235049384534c167caea52b85a694f6103d14",
            "cuda_toolkit": "13.4.1",
            "repack_revision": "sol1",
            "inputs": [
                {
                    "role": "engine",
                    "filename": "llama-b11429-bin-ubuntu-cuda-13.4-x64.tar.gz",
                    "sha256": "8082b7eaa74a714c9fecca19128f751c8e32da763ee8096b8ad1e824da7621d3",
                    "size_bytes": 152519318,
                    "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11429/"
                },
                {
                    "role": "cudart",
                    "filename": "cudart-llama-b11429-bin-ubuntu-cuda-13.4-x64.tar.gz",
                    "sha256": "93d18648d815b2bd624d83d82f653e1db97afb478f02064305fe3cf570040a6d",
                    "size_bytes": 440236630,
                    "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11429/"
                }
            ],
            "arch": arch,
            "binary_name": "llama-server",
            "wanted_files": wanted_files
        }))
    } else {
        Some(json!({
            "unit": "llama-server-cuda",
            "artifact_key": key,
            "url": url,
            "sha256": sha256,
            "size_bytes": size_bytes,
            "release_tag": "b10068",
            "upstream_image_digest": "sha256:5bd5290bd35cfde893d0dcbd9811723c16d89575927d537b5f21becbfbab2f63",
            "llama_cpp_revision": "571d0d540df04f25298d0e159e520d9fc62ed121",
            "repack_revision": "sol1",
            "arch": arch,
            "binary_name": "llama-server",
            "wanted_files": wanted_files
        }))
    }
}
pub fn model_identity(model_id: &str) -> Option<Value> {
    (model_id == "local/qwen3.5-4b").then(|| json!({"unit":"local-model","model_id":"local/qwen3.5-4b","repo":"unsloth/Qwen3.5-4B-GGUF","revision":"main","filename":"Qwen3.5-4B-Q4_K_M.gguf","sha256":"00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4","mmproj_filename":"mmproj-F16.gguf","mmproj_sha256":"cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864"}))
}

pub fn parakeet_vulkan_pin(
    key: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    PARAKEET_VULKAN_PINS
        .iter()
        .find(|pin| pin.0 == key)
        .map(|pin| (pin.1, pin.2, pin.3, pin.4))
}
pub fn parakeet_cpu_pin(
    key: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    PARAKEET_CPU_PINS
        .iter()
        .find(|pin| pin.0 == key)
        .map(|pin| (pin.1, pin.2, pin.3, pin.4))
}
pub fn parakeet_backend_pin(
    key: &str,
    backend: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    match backend {
        "vulkan" => parakeet_vulkan_pin(key),
        "cpu" => parakeet_cpu_pin(key),
        _ => None,
    }
}
/// The RECORDED identity of the parakeet model, which is not the same thing as
/// what gets FETCHED.
///
/// ⛔ `size_bytes` is deliberately absent. It is a fetch property -- the catalog
/// carries it and `download_verified` refuses a length mismatch against it --
/// and it has never been part of the identity the reference writes beside an
/// installed model. `prove_manifest` compares pin identity by exact
/// canonicalized-JSON equality, so a sixth key here makes every manifest an
/// owner already has on disk read `manifest_pin_mismatch` and re-fetch the
/// model. Measured 2026-08-14: a manifest written by the reference proved
/// `missing-or-mismatched` on this one key alone. `model_identity` above is the
/// shape to match -- it agrees with its reference key-for-key and carries no
/// size either.
pub fn parakeet_model_identity() -> Value {
    let (repo, filename, revision, sha256, _size_bytes) = PARAKEET_MODEL;
    json!({"unit":"parakeet-model","repo":repo,"filename":filename,"revision":revision,"sha256":sha256})
}
pub fn parakeet_backend_identity(key: &str, backend: &str) -> Option<Value> {
    let (release_tag, filename, sha256, binary_name) = parakeet_backend_pin(key, backend)?;
    Some(
        json!({"unit":"parakeet-server","artifact_key":key,"backend":backend,"release_tag":release_tag,"filename":filename,"sha256":sha256,"binary_name":binary_name}),
    )
}

#[cfg(test)]
pub(crate) fn origin_url_for_arch_key(unit: &str, arch_key: &str) -> Option<String> {
    let platform = match arch_key {
        "aarch64-apple-darwin" => Platform::MacosArm64,
        "x86_64-unknown-linux-gnu" => Platform::LinuxX64,
        "aarch64-unknown-linux-gnu" => Platform::LinuxArm64,
        _ => return None,
    };
    resolve(unit, Some(platform), None)
        .into_iter()
        .find(|artifact| artifact.artifact_key == Some(arch_key))
        .map(|artifact| {
            archive::origin_url(
                archive::PRODUCTION_DOWNLOAD_POLICY.origin_base_url,
                artifact.origin_key,
            )
        })
}

pub fn cache_root(journal: &Path) -> PathBuf {
    journal.join("cache/providers/local")
}
pub fn parakeet_cache_root(journal: &Path) -> PathBuf {
    journal.join("cache/providers/parakeet")
}
pub fn platform_key() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => format!("{arch}-apple-darwin"),
        "linux" => format!("{arch}-unknown-linux-gnu"),
        other => format!("{arch}-{other}"),
    }
}

/// Unlike `platform_key`, parakeet-cpp has no macOS/other-OS fallback shape
/// to format -- it is a closed lookup over exactly the two supported Linux
/// arches, and anything else is a hard error. Takes `os_name`/`arch`
/// explicitly (never reads `std::env::consts` itself) so callers -- and
/// tests -- supply the platform rather than this function probing the real
/// host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedParakeetPlatform {
    pub os_name: String,
    pub arch: String,
}
impl std::fmt::Display for UnsupportedParakeetPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "parakeet-cpp is unsupported on {}/{}",
            self.os_name, self.arch
        )
    }
}
impl std::error::Error for UnsupportedParakeetPlatform {}

pub fn parakeet_artifact_key(
    os_name: &str,
    arch: &str,
) -> Result<String, UnsupportedParakeetPlatform> {
    let unsupported = || UnsupportedParakeetPlatform {
        os_name: os_name.to_owned(),
        arch: arch.to_owned(),
    };
    if os_name != "linux" {
        return Err(unsupported());
    }
    match arch.to_lowercase().as_str() {
        "amd64" | "x64" | "x86_64" => Ok("x86_64-unknown-linux-gnu".to_owned()),
        "arm64" | "aarch64" => Ok("aarch64-unknown-linux-gnu".to_owned()),
        _ => Err(unsupported()),
    }
}
/// Convenience wrapper over `parakeet_artifact_key` for the current host.
/// Never call this from a test -- pass explicit os_name/arch instead so the
/// assertion is about the mapping, not about the build machine.
pub fn parakeet_host_artifact_key() -> Result<String, UnsupportedParakeetPlatform> {
    parakeet_artifact_key(std::env::consts::OS, std::env::consts::ARCH)
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
    json!({"llama_server_pins": LLAMA_SERVER_PINS.iter().map(|p| json!({"artifact_key":p.0,"release_tag":p.1,"filename":p.2,"sha256":p.3,"binary_name":p.4})).collect::<Vec<_>>(), "cuda_server_pin":{"cuda_version":13,"embedded_arch_set":["sm_86","sm_89","sm_120a","sm_121a"],"binary_name":"llama-server","device_flag_value":"CUDA0","visible_devices_env":"CUDA_VISIBLE_DEVICES","shared_wanted_files":CUDA_SHARED_WANTED_FILES,"cpu_wanted_files_by_arch":{"amd64":CUDA_AMD64_WANTED_FILES,"arm64":CUDA_ARM64_WANTED_FILES},"artifacts":CUDA_ARTIFACTS.iter().map(|p| cuda_identity(p.0).unwrap()).collect::<Vec<_>>()}})
}

/// Mirrors `paths()`, keyed the way Parakeet's own cache tree is laid out
/// (`journal/cache/providers/parakeet/bin/<key>/<backend>/<release_tag>/parakeet-server`,
/// `.../models/<repo>/<revision>/<filename>`), not Local's.
pub fn parakeet_paths(journal: &Path, key: &str) -> Value {
    let root = parakeet_cache_root(journal);
    let (repo, filename, revision, ..) = PARAKEET_MODEL;
    let model_path = root
        .join("models")
        .join(repo.replace('/', "__"))
        .join(revision)
        .join(filename);
    json!({
        "artifact_key": key,
        "cache_root": root,
        "binary_path_vulkan": parakeet_vulkan_pin(key).map(|(release_tag, _, _, binary_name)| {
            root.join("bin").join(key).join("vulkan").join(release_tag).join(binary_name)
        }),
        "binary_path_cpu": parakeet_cpu_pin(key).map(|(release_tag, _, _, binary_name)| {
            root.join("bin").join(key).join("cpu").join(release_tag).join(binary_name)
        }),
        "model_path": model_path,
    })
}
pub fn parakeet_pins_json() -> Value {
    json!({
        "parakeet_vulkan_pins": PARAKEET_VULKAN_PINS.iter().map(|p| json!({"artifact_key":p.0,"release_tag":p.1,"filename":p.2,"sha256":p.3,"binary_name":p.4})).collect::<Vec<_>>(),
        "parakeet_cpu_pins": PARAKEET_CPU_PINS.iter().map(|p| json!({"artifact_key":p.0,"release_tag":p.1,"filename":p.2,"sha256":p.3,"binary_name":p.4})).collect::<Vec<_>>(),
        "parakeet_model": parakeet_model_identity(),
    })
}
