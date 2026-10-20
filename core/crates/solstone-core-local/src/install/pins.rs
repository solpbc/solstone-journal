// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use solstone_core_assets::Platform;
#[cfg(test)]
use solstone_core_assets::resolve;

pub const LLAMA_SERVER_PINS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "b11669",
        "llama-b11669-bin-macos-arm64.tar.gz",
        "dbed55493d72bc1ba2babc8770d669c2289b5bc4ee9fb2a4dfc158d45b7d13fd",
        "llama-server",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "b11669",
        "llama-b11669-bin-ubuntu-vulkan-x64.tar.gz",
        "e8709e5ba6c1b093dea7f3de57f4aba842f9a188fba6b465782d65d6fcfa4d9f",
        "llama-server",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "b11669",
        "llama-b11669-bin-ubuntu-vulkan-arm64.tar.gz",
        "d283522e479c0d532e3a04d715dd852ffb2d95925caa6d41ef2ba53a65e46a51",
        "llama-server",
    ),
];
pub const CUDA_ARTIFACTS: &[(&str, &str, &str, u64)] = &[
    (
        "x86_64-unknown-linux-gnu",
        "https://updates.solstone.app/runtimes/llama-cuda13/b11669/llama-b11669-bin-linux-cuda13-amd64-sol1.tar.gz",
        "2288ff1960305206f11495c3407caf9f196bc8510ca9016c9bc3b5054f9db80d",
        592895506,
    ),
    (
        "aarch64-unknown-linux-gnu",
        "https://updates.solstone.app/runtimes/llama-cuda13/b11669/llama-b11669-bin-linux-cuda13-arm64-sol1.tar.gz",
        "e7cee5a124adb5d93f2ada5bb26ddab123f07ae0d4cef5ec8fb922d63e939509",
        700854480,
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

pub const VULKAN_LINUX_ARM64_B11429_REQUIRED: &[RequiredMember] = &[
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
    RequiredMember::Regular("libggml-cpu-armv8.0_1.so"),
    RequiredMember::Regular("libggml-cpu-armv8.2_1.so"),
    RequiredMember::Regular("libggml-cpu-armv8.2_2.so"),
    RequiredMember::Regular("libggml-cpu-armv8.2_3.so"),
    RequiredMember::Regular("libggml-cpu-armv8.6_1.so"),
    RequiredMember::Regular("libggml-cpu-armv8.6_2.so"),
    RequiredMember::Regular("libggml-cpu-armv9.2_1.so"),
    RequiredMember::Regular("libggml-cpu-armv9.2_2.so"),
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

pub const VULKAN_LINUX_ARM64_B11669_REQUIRED: &[RequiredMember] = &[
    RequiredMember::Regular("LICENSE"),
    RequiredMember::Link {
        name: "libggml-base.so",
        target: "libggml-base.so.0",
    },
    RequiredMember::Link {
        name: "libggml-base.so.0",
        target: "libggml-base.so.0.27.0",
    },
    RequiredMember::Regular("libggml-base.so.0.27.0"),
    RequiredMember::Regular("libggml-cpu-armv8.0_1.so"),
    RequiredMember::Regular("libggml-cpu-armv8.2_1.so"),
    RequiredMember::Regular("libggml-cpu-armv8.2_2.so"),
    RequiredMember::Regular("libggml-cpu-armv8.2_3.so"),
    RequiredMember::Regular("libggml-cpu-armv8.6_1.so"),
    RequiredMember::Regular("libggml-cpu-armv8.6_2.so"),
    RequiredMember::Regular("libggml-cpu-armv9.2_1.so"),
    RequiredMember::Regular("libggml-cpu-armv9.2_2.so"),
    RequiredMember::Regular("libggml-rpc.so"),
    RequiredMember::Regular("libggml-vulkan.so"),
    RequiredMember::Link {
        name: "libggml.so",
        target: "libggml.so.0",
    },
    RequiredMember::Link {
        name: "libggml.so.0",
        target: "libggml.so.0.27.0",
    },
    RequiredMember::Regular("libggml.so.0.27.0"),
    RequiredMember::Link {
        name: "libllama-common.so",
        target: "libllama-common.so.0",
    },
    RequiredMember::Link {
        name: "libllama-common.so.0",
        target: "libllama-common.so.0.7.0",
    },
    RequiredMember::Regular("libllama-common.so.0.7.0"),
    RequiredMember::Regular("libllama-server-impl.so"),
    RequiredMember::Link {
        name: "libllama.so",
        target: "libllama.so.0",
    },
    RequiredMember::Link {
        name: "libllama.so.0",
        target: "libllama.so.0.7.0",
    },
    RequiredMember::Regular("libllama.so.0.7.0"),
    RequiredMember::Link {
        name: "libmtmd.so",
        target: "libmtmd.so.0",
    },
    RequiredMember::Link {
        name: "libmtmd.so.0",
        target: "libmtmd.so.0.7.0",
    },
    RequiredMember::Regular("libmtmd.so.0.7.0"),
    RequiredMember::Regular("llama-server"),
];

pub const VULKAN_LINUX_X64_B11669_REQUIRED: &[RequiredMember] = &[
    RequiredMember::Regular("LICENSE"),
    RequiredMember::Link {
        name: "libggml-base.so",
        target: "libggml-base.so.0",
    },
    RequiredMember::Link {
        name: "libggml-base.so.0",
        target: "libggml-base.so.0.27.0",
    },
    RequiredMember::Regular("libggml-base.so.0.27.0"),
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
        target: "libggml.so.0.27.0",
    },
    RequiredMember::Regular("libggml.so.0.27.0"),
    RequiredMember::Link {
        name: "libllama-common.so",
        target: "libllama-common.so.0",
    },
    RequiredMember::Link {
        name: "libllama-common.so.0",
        target: "libllama-common.so.0.7.0",
    },
    RequiredMember::Regular("libllama-common.so.0.7.0"),
    RequiredMember::Regular("libllama-server-impl.so"),
    RequiredMember::Link {
        name: "libllama.so",
        target: "libllama.so.0",
    },
    RequiredMember::Link {
        name: "libllama.so.0",
        target: "libllama.so.0.7.0",
    },
    RequiredMember::Regular("libllama.so.0.7.0"),
    RequiredMember::Link {
        name: "libmtmd.so",
        target: "libmtmd.so.0",
    },
    RequiredMember::Link {
        name: "libmtmd.so.0",
        target: "libmtmd.so.0.7.0",
    },
    RequiredMember::Regular("libmtmd.so.0.7.0"),
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

pub const METAL_MACOS_ARM64_B11669_REQUIRED: &[RequiredMember] = &[
    RequiredMember::Regular("LICENSE"),
    RequiredMember::Regular("libggml-base.0.27.0.dylib"),
    RequiredMember::Link {
        name: "libggml-base.0.dylib",
        target: "libggml-base.0.27.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-base.dylib",
        target: "libggml-base.0.dylib",
    },
    RequiredMember::Regular("libggml-blas.0.27.0.dylib"),
    RequiredMember::Link {
        name: "libggml-blas.0.dylib",
        target: "libggml-blas.0.27.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-blas.dylib",
        target: "libggml-blas.0.dylib",
    },
    RequiredMember::Regular("libggml-cpu.0.27.0.dylib"),
    RequiredMember::Link {
        name: "libggml-cpu.0.dylib",
        target: "libggml-cpu.0.27.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-cpu.dylib",
        target: "libggml-cpu.0.dylib",
    },
    RequiredMember::Regular("libggml-metal.0.27.0.dylib"),
    RequiredMember::Link {
        name: "libggml-metal.0.dylib",
        target: "libggml-metal.0.27.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-metal.dylib",
        target: "libggml-metal.0.dylib",
    },
    RequiredMember::Regular("libggml-rpc.0.27.0.dylib"),
    RequiredMember::Link {
        name: "libggml-rpc.0.dylib",
        target: "libggml-rpc.0.27.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml-rpc.dylib",
        target: "libggml-rpc.0.dylib",
    },
    RequiredMember::Regular("libggml.0.27.0.dylib"),
    RequiredMember::Link {
        name: "libggml.0.dylib",
        target: "libggml.0.27.0.dylib",
    },
    RequiredMember::Link {
        name: "libggml.dylib",
        target: "libggml.0.dylib",
    },
    RequiredMember::Regular("libllama-common.0.7.0.dylib"),
    RequiredMember::Link {
        name: "libllama-common.0.dylib",
        target: "libllama-common.0.7.0.dylib",
    },
    RequiredMember::Link {
        name: "libllama-common.dylib",
        target: "libllama-common.0.dylib",
    },
    RequiredMember::Regular("libllama-server-impl.dylib"),
    RequiredMember::Regular("libllama.0.7.0.dylib"),
    RequiredMember::Link {
        name: "libllama.0.dylib",
        target: "libllama.0.7.0.dylib",
    },
    RequiredMember::Link {
        name: "libllama.dylib",
        target: "libllama.0.dylib",
    },
    RequiredMember::Regular("libmtmd.0.7.0.dylib"),
    RequiredMember::Link {
        name: "libmtmd.0.dylib",
        target: "libmtmd.0.7.0.dylib",
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
        ("b11429", "aarch64-unknown-linux-gnu") => Some(VULKAN_LINUX_ARM64_B11429_REQUIRED),
        ("b11429", "aarch64-apple-darwin") => Some(METAL_MACOS_ARM64_B11429_REQUIRED),
        ("b11669", "x86_64-unknown-linux-gnu") => Some(VULKAN_LINUX_X64_B11669_REQUIRED),
        ("b11669", "aarch64-unknown-linux-gnu") => Some(VULKAN_LINUX_ARM64_B11669_REQUIRED),
        ("b11669", "aarch64-apple-darwin") => Some(METAL_MACOS_ARM64_B11669_REQUIRED),
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
        "v0.6.1",
        "parakeet-v0.6.1-bin-linux-vulkan-x64.tar.gz",
        "881fd99d531a4dcfc26119a4969aec61b8390ba84e9d641716411d4c1db2de3a",
        "parakeet-server",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "v0.6.1",
        "parakeet-v0.6.1-bin-linux-vulkan-arm64.tar.gz",
        "96bc0a9ac524ea875f7260fbaed65632d55cd249e251d0f8a69d9df13dc30c9c",
        "parakeet-server",
    ),
];
pub const PARAKEET_CPU_PINS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "x86_64-unknown-linux-gnu",
        "v0.6.1",
        "parakeet-v0.6.1-bin-linux-cpu-x64.tar.gz",
        "cce60d122ab72e1068cd0d164e54a21655a0b83f1b9c21befc20124f5a972c10",
        "parakeet-server",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "v0.6.1",
        "parakeet-v0.6.1-bin-linux-cpu-arm64.tar.gz",
        "85b6dafce8a984d0971d94865da5e2e50e111604fb6e7030f5b599dc2616c8db",
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
pub const CUDA_RELEASE_REVISIONS: &[(&str, &str)] = &[
    ("b11429", "d81235049384534c167caea52b85a694f6103d14"),
    ("b11669", "620d2225cc436ab0b001e3200e590e3b286ef9ab"),
];

pub fn cuda_identity(key: &str) -> Option<Value> {
    let (url, sha256, size_bytes) = cuda_pin(key)?;
    let arch = cuda_runtime_arch(key)?;
    let wanted_files = cuda_wanted_files(arch)?;
    let release_tag = url.rsplit('/').nth(1)?;
    let llama_cpp_revision = CUDA_RELEASE_REVISIONS
        .iter()
        .find(|(release, _)| *release == release_tag)?
        .1;
    let inputs = match arch {
        "amd64" => json!([
            {
                "role": "engine",
                "filename": "llama-b11669-bin-ubuntu-cuda-13.4-x64.tar.gz",
                "sha256": "80878473d27278dbe9c38fb7e704f292a51e889d3c28a1e863137cdee1e2ba4e",
                "size_bytes": 153679828,
                "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11669/"
            },
            {
                "role": "cudart",
                "filename": "cudart-llama-b11669-bin-ubuntu-cuda-13.4-x64.tar.gz",
                "sha256": "1108bd9300a281e8eabb18d3fa028d4240ecd983f9f90dbbbc3297671016d697",
                "size_bytes": 440236547,
                "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11669/"
            }
        ]),
        "arm64" => json!([
            {
                "role": "engine",
                "filename": "llama-b11669-bin-ubuntu-cuda-13.4-arm64.tar.gz",
                "sha256": "19902edd0c02942ac736c2dea5e104ff219a196a5cd07f125a7adcd44c103e6b",
                "size_bytes": 149286186,
                "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11669/"
            },
            {
                "role": "cudart",
                "filename": "cudart-llama-b11669-bin-ubuntu-cuda-13.4-arm64.tar.gz",
                "sha256": "6c81453d16a574eef3273662a7a8ceafa3537699a5b2ffec3fc853eb1a08f67c",
                "size_bytes": 552521396,
                "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11669/"
            }
        ]),
        _ => return None,
    };
    // The signed publisher revision is not the runtime source.
    Some(json!({
        "unit": "llama-server-cuda",
        "artifact_key": key,
        "url": url,
        "sha256": sha256,
        "size_bytes": size_bytes,
        "release_tag": release_tag,
        "llama_cpp_revision": llama_cpp_revision,
        "cuda_toolkit": "13.4.1",
        "repack_revision": "sol1",
        "inputs": inputs,
        "arch": arch,
        "binary_name": "llama-server",
        "wanted_files": wanted_files
    }))
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
        .map(|artifact| format!("https://updates.solstone.app/{}", artifact.origin_key))
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

/// Complete model/projector identity; a projector pin move creates a new sibling.
pub fn model_generation(
    model_id: &str,
) -> Option<solstone_core_artifact_download::generation::Generation> {
    use solstone_core_artifact_download::generation::{Generation, Member};
    let identity = model_identity(model_id)?;
    let mut files = Vec::new();
    for (name_key, sha_key) in [("filename", "sha256"), ("mmproj_filename", "mmproj_sha256")] {
        let name = identity[name_key].as_str()?;
        let sha = identity[sha_key].as_str()?;
        let asset = solstone_core_assets::resolve("local-model", None, None)
            .into_iter()
            .find(|a| a.filename == name && a.sha256 == sha)?;
        files.push(Member {
            file: name.into(),
            sha256: sha.into(),
            size_bytes: asset.size_bytes,
        });
    }
    let mut generation = Generation::new("local-model", files).ok()?;
    generation.identity = identity;
    Some(generation)
}

/// Reuse a matching legacy install in place. New pin sets live beside it,
/// never inside a tree recursively inventoried by an older release.
pub fn model_directory(journal: &Path, model_id: &str) -> PathBuf {
    let root = cache_root(journal).join("models");
    let legacy = root.join(model_id.replace('/', "__"));
    let Some(identity) = model_identity(model_id) else {
        return legacy;
    };
    let legacy_manifest = super::manifest::artifact_manifest_path(&legacy);
    let matching = std::fs::read(&legacy_manifest)
        .ok()
        .filter(|bytes| bytes.len() <= 65536)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|record| record["source"]["pin_identity"] == identity);
    if matching {
        return legacy;
    }
    model_generation(model_id)
        .map(|g| g.directory(&root))
        .unwrap_or(legacy)
}

pub fn model_fetch_directory(journal: &Path, model_id: &str) -> Option<PathBuf> {
    model_generation(model_id)?;
    Some(cache_root(journal).join(".fetch").join("models"))
}

/// A pinned member reusable without allocating another model copy.
pub(crate) fn reusable_model_member(
    journal: &Path,
    model_id: &str,
    member: &solstone_core_artifact_download::generation::Member,
) -> Option<PathBuf> {
    let root = cache_root(journal).join("models");
    let flat = root.join(model_id.replace('/', "__"));
    if solstone_core_artifact_download::generation::Generation::new(
        "local-model",
        vec![member.clone()],
    )
    .ok()?
    .verify(&flat)
    {
        return Some(flat.join(&member.file));
    }
    solstone_core_artifact_download::generation::reusable_member(&root, member)
}

pub(crate) fn remaining_model_member_bytes(
    journal: &Path,
    model_id: &str,
    member: &solstone_core_artifact_download::generation::Member,
    installed: &Path,
    staging: &Path,
) -> u64 {
    if reusable_model_member(journal, model_id, member).is_some() {
        return 0;
    }
    let credit = solstone_core_artifact_download::retained_prefix_bytes(
        &installed.join(&member.file),
        &member.sha256,
        member.size_bytes,
    )
    .max(solstone_core_artifact_download::retained_prefix_bytes(
        &staging.join(&member.file),
        &member.sha256,
        member.size_bytes,
    ));
    member.size_bytes.saturating_sub(credit)
}

pub fn hold_model_use(directory: &Path) -> std::io::Result<solstone_core_journal_io::FileLock> {
    solstone_core_artifact_download::generation::hold_use(directory)
}

pub fn collect_model_generations(journal: &Path) {
    let Ok(Some(_lease)) = super::lease::acquire(journal, "local") else {
        return;
    };
    collect_model_generations_under_install_lease(journal);
}

/// Called only while the successful installer owns the existing local lease.
pub(crate) fn collect_model_generations_under_install_lease(journal: &Path) {
    let Some(current) = model_generation("local/qwen3.5-4b") else {
        return;
    };
    let root = cache_root(journal).join("models");
    let legacy = model_directory(journal, "local/qwen3.5-4b");
    let target = current.directory(&root);
    if legacy != target
        && !current.is_sealed(&target)
        && current.verify(&legacy)
        && let Ok(_hold) = solstone_core_artifact_download::generation::hold_write(&target)
        && let Err(error) = current.back_from(&legacy, &target)
    {
        log::warn!("could not retain verified model generation: {error}");
    }
    if let Err(error) = super::model_cache::collect_legacy(journal, &current) {
        log::warn!("could not clean obsolete legacy model: {error}");
    }
    if let Err(error) = solstone_core_artifact_download::generation::collect(
        &root,
        "local-model",
        std::slice::from_ref(&current),
        true,
    ) {
        log::warn!("could not clean obsolete model generations: {error}");
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
        "model_dir": model_id.map(|id| model_directory(journal,id)),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingDownloadRow {
    pub unit: &'static str,
    pub platform: Option<Platform>,
    pub filename: Option<&'static str>,
    pub packaged: bool,
}

fn platform_from_key(key: &str) -> Option<Platform> {
    match key {
        "aarch64-apple-darwin" => Some(Platform::MacosArm64),
        "x86_64-unknown-linux-gnu" => Some(Platform::LinuxX64),
        "aarch64-unknown-linux-gnu" => Some(Platform::LinuxArm64),
        _ => None,
    }
}

pub fn thinking_download_rows(
    backend: crate::Backend,
    platform_key: &str,
) -> Option<Vec<ThinkingDownloadRow>> {
    let model_id = model_identity("local/qwen3.5-4b")?;
    let filename = model_id.get("filename")?.as_str()?;
    let mmproj_filename = model_id.get("mmproj_filename")?.as_str()?;
    let model_rows = vec![
        ThinkingDownloadRow {
            unit: "local-model",
            platform: None,
            filename: Some(
                super::select_artifact("local-model", None, None, None, Some(filename))
                    .ok()?
                    .filename,
            ),
            packaged: false,
        },
        ThinkingDownloadRow {
            unit: "local-model",
            platform: None,
            filename: Some(
                super::select_artifact("local-model", None, None, None, Some(mmproj_filename))
                    .ok()?
                    .filename,
            ),
            packaged: false,
        },
    ];

    if platform_key == "x86_64-windows" {
        let mut rows = vec![ThinkingDownloadRow {
            unit: "llama-server",
            platform: None,
            filename: None,
            packaged: true,
        }];
        rows.extend(model_rows);
        return Some(rows);
    }

    let unit = match backend {
        crate::Backend::Vulkan => "llama-server-vulkan",
        crate::Backend::Cuda => "llama-server-cuda",
    };
    let platform = platform_from_key(platform_key);
    // Use the same selected, published runtime as the installer. CUDA's
    // upstream build inputs are not transferred by the journal.
    let artifact = super::select_artifact(unit, platform, None, Some(platform_key), None).ok()?;
    let mut rows = vec![ThinkingDownloadRow {
        unit,
        platform,
        filename: Some(artifact.filename),
        packaged: false,
    }];
    rows.extend(model_rows);
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_download_rows_matches_identities_and_differentiates_cuda_vulkan() {
        let model_id = model_identity("local/qwen3.5-4b").expect("model identity exists");
        let model_filename = model_id["filename"].as_str().unwrap();
        let model_mmproj = model_id["mmproj_filename"].as_str().unwrap();

        let key = "x86_64-unknown-linux-gnu";
        let cuda_rows = thinking_download_rows(crate::Backend::Cuda, key).expect("cuda rows");
        let vulkan_rows = thinking_download_rows(crate::Backend::Vulkan, key).expect("vulkan rows");

        let published_cuda = crate::install::select_artifact(
            "llama-server-cuda",
            Some(Platform::LinuxX64),
            None,
            Some(key),
            None,
        )
        .expect("published journal runtime");

        let actual_cuda_files: Vec<&str> = cuda_rows
            .iter()
            .filter(|r| r.unit == "llama-server-cuda")
            .map(|r| r.filename.unwrap())
            .collect();
        assert_eq!(actual_cuda_files, vec![published_cuda.filename]);

        let vulkan_file = vulkan_rows
            .iter()
            .find(|r| r.unit == "llama-server-vulkan")
            .unwrap()
            .filename
            .unwrap();
        assert!(!actual_cuda_files.contains(&vulkan_file));

        for rows in [&cuda_rows, &vulkan_rows] {
            let m1 = rows
                .iter()
                .find(|r| r.filename == Some(model_filename))
                .expect("model filename present");
            assert_eq!(m1.unit, "local-model");
            let m2 = rows
                .iter()
                .find(|r| r.filename == Some(model_mmproj))
                .expect("mmproj present");
            assert_eq!(m2.unit, "local-model");
        }

        let win_rows =
            thinking_download_rows(crate::Backend::Vulkan, "x86_64-windows").expect("win rows");
        let win_runtime = &win_rows[0];
        assert!(win_runtime.packaged);
        assert!(
            solstone_core_assets::resolve(win_runtime.unit, win_runtime.platform, None).is_empty()
        );
    }
}
