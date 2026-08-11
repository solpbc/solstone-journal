// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Declarative inventory of journal-downloadable artifacts.

use std::fmt;
use std::sync::LazyLock;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    MacosArm64,
    LinuxX64,
    LinuxArm64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
// A backend is present only when it discriminates artifacts within a unit.
pub enum Backend {
    Cpu,
    Vulkan,
    Metal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Artifact {
    pub unit: &'static str,
    pub version: &'static str,
    pub filename: &'static str,
    pub sha256: &'static str,
    pub size_bytes: u64,
    /// The URL a future fetch path resolves; this wave resolves upstream.
    pub upstream_url: &'static str,
    pub origin_key: &'static str,
    pub artifact_key: Option<&'static str>,
    pub platform: Option<Platform>,
    pub backend: Option<Backend>,
    pub extracted_binary_sha256: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    EmptyOrWhitespace,
    MutableRef(String),
}

impl fmt::Display for VersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyOrWhitespace => {
                formatter.write_str("version is empty or contains outer whitespace")
            }
            Self::MutableRef(value) => write!(formatter, "version rejects mutable ref `{value}`"),
        }
    }
}

impl std::error::Error for VersionError {}

/// Reject known mutable refs. This denylist is not a proof that every other
/// version string is immutable.
pub fn check_version(version: &str) -> Result<(), VersionError> {
    if version.trim().is_empty() || version.trim() != version {
        return Err(VersionError::EmptyOrWhitespace);
    }
    match version.to_ascii_lowercase().as_str() {
        "main" | "latest" | "head" | "master" | "develop" | "development" | "trunk" | "default"
        | "stable" | "edge" | "nightly" | "canary" => {
            Err(VersionError::MutableRef(version.to_owned()))
        }
        _ => Ok(()),
    }
}

/// `origin_key` convention: downloadable artifacts ordinarily declare
/// `assets/{unit}/{version}/{filename}`. CUDA is intentionally exceptional:
/// its released objects are served under `runtimes/llama-cuda13/b10068/{filename}`.
/// These are declarations only; this crate performs no fetches.
///
/// MLX is excluded because it is a repository snapshot without per-file
/// filenames and digests. nvattest is excluded because its three archive plus
/// companion-manifest pairs are governed by `nvattest_authority_v1.json` and
/// its own `url_prefix` contract; duplicating them here would be unbound truth.
pub static ARTIFACTS: &[Artifact] = &[
    Artifact {
        unit: "ced-engine",
        version: "v0.1.0",
        filename: "ced-v0.1.0-lib-linux-cpu-arm64.tar.gz",
        sha256: "a87de0a8b086429aa5d6544a6f881a70e62726d07901734640ac85dbf146181e",
        size_bytes: 720034,
        upstream_url: "https://github.com/localai-org/ced.cpp/releases/download/v0.1.0/ced-v0.1.0-lib-linux-cpu-arm64.tar.gz",
        origin_key: "assets/ced-engine/v0.1.0/ced-v0.1.0-lib-linux-cpu-arm64.tar.gz",
        artifact_key: Some("linux-cpu-arm64"),
        platform: Some(Platform::LinuxArm64),
        backend: Some(Backend::Cpu),
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "ced-engine",
        version: "v0.1.0",
        filename: "ced-v0.1.0-lib-linux-cpu-x64.tar.gz",
        sha256: "915e0573bc4e17197a7a893d0eb98e1a851abb64451b2e1a8ad51f5f99040360",
        size_bytes: 788651,
        upstream_url: "https://github.com/localai-org/ced.cpp/releases/download/v0.1.0/ced-v0.1.0-lib-linux-cpu-x64.tar.gz",
        origin_key: "assets/ced-engine/v0.1.0/ced-v0.1.0-lib-linux-cpu-x64.tar.gz",
        artifact_key: Some("linux-cpu-x64"),
        platform: Some(Platform::LinuxX64),
        backend: Some(Backend::Cpu),
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "ced-engine",
        version: "v0.1.0",
        filename: "ced-v0.1.0-lib-macos-metal-arm64.tar.gz",
        sha256: "4c913ba0ece1d06ba2210da9fcaee3d8199ca3c62697c331810f224444e4054b",
        size_bytes: 686952,
        upstream_url: "https://github.com/localai-org/ced.cpp/releases/download/v0.1.0/ced-v0.1.0-lib-macos-metal-arm64.tar.gz",
        origin_key: "assets/ced-engine/v0.1.0/ced-v0.1.0-lib-macos-metal-arm64.tar.gz",
        artifact_key: Some("macos-metal-arm64"),
        platform: Some(Platform::MacosArm64),
        backend: Some(Backend::Metal),
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "ced-model",
        version: "b5e9a4aad6438763c8da16079d77563fbed35c65",
        filename: "ced-tiny-q8_0.gguf",
        sha256: "48bee4e2fc3cc85d7806e03471db24e77fda6c2a2e81ffe9ef67caebaf2bd674",
        size_bytes: 6211616,
        upstream_url: "https://huggingface.co/mudler/ced-gguf/resolve/b5e9a4aad6438763c8da16079d77563fbed35c65/ced-tiny-q8_0.gguf",
        origin_key: "assets/ced-model/b5e9a4aad6438763c8da16079d77563fbed35c65/ced-tiny-q8_0.gguf",
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "llama-server-cuda",
        version: "b10068",
        filename: "llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz",
        sha256: "3727630e6ac79953f5c652fddcfd7100da98c55d773c0aec115a55f40f3aafea",
        size_bytes: 550238443,
        upstream_url: "https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz",
        origin_key: "runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz",
        artifact_key: Some("x86_64-unknown-linux-gnu"),
        platform: Some(Platform::LinuxX64),
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "llama-server-cuda",
        version: "b10068",
        filename: "llama-b10068-bin-linux-cuda13-arm64-sol1.tar.gz",
        sha256: "6de68319db40e8c0eb45dc4bd3a45a16971dbdc128f2b621b19bef5dae87d064",
        size_bytes: 654508507,
        upstream_url: "https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-arm64-sol1.tar.gz",
        origin_key: "runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-arm64-sol1.tar.gz",
        artifact_key: Some("aarch64-unknown-linux-gnu"),
        platform: Some(Platform::LinuxArm64),
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "llama-server-vulkan",
        version: "b10068",
        filename: "llama-b10068-bin-macos-arm64.tar.gz",
        sha256: "13aa2d40c76ad1dcb8ebeec5f0d2814bf3b2f84a66935c7d4dc6f7cca8e38d68",
        size_bytes: 10603591,
        upstream_url: "https://github.com/ggml-org/llama.cpp/releases/download/b10068/llama-b10068-bin-macos-arm64.tar.gz",
        origin_key: "assets/llama-server-vulkan/b10068/llama-b10068-bin-macos-arm64.tar.gz",
        artifact_key: Some("aarch64-apple-darwin"),
        platform: Some(Platform::MacosArm64),
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "llama-server-vulkan",
        version: "b10068",
        filename: "llama-b10068-bin-ubuntu-vulkan-arm64.tar.gz",
        sha256: "c3c49e6e124a574165ca28317be021b1a12a2ea06977e3eb7daee3eb443eb186",
        size_bytes: 26119233,
        upstream_url: "https://github.com/ggml-org/llama.cpp/releases/download/b10068/llama-b10068-bin-ubuntu-vulkan-arm64.tar.gz",
        origin_key: "assets/llama-server-vulkan/b10068/llama-b10068-bin-ubuntu-vulkan-arm64.tar.gz",
        artifact_key: Some("aarch64-unknown-linux-gnu"),
        platform: Some(Platform::LinuxArm64),
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "llama-server-vulkan",
        version: "b10068",
        filename: "llama-b10068-bin-ubuntu-vulkan-x64.tar.gz",
        sha256: "713641920dce6c8efb953ebc9ffa309977e200cec5e182e6ad0e8b086203cdc3",
        size_bytes: 32028597,
        upstream_url: "https://github.com/ggml-org/llama.cpp/releases/download/b10068/llama-b10068-bin-ubuntu-vulkan-x64.tar.gz",
        origin_key: "assets/llama-server-vulkan/b10068/llama-b10068-bin-ubuntu-vulkan-x64.tar.gz",
        artifact_key: Some("x86_64-unknown-linux-gnu"),
        platform: Some(Platform::LinuxX64),
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "local-model",
        version: "e87f176479d0855a907a41277aca2f8ee7a09523",
        filename: "Qwen3.5-4B-Q4_K_M.gguf",
        sha256: "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
        size_bytes: 2740937888,
        upstream_url: "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
        origin_key: "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "local-model",
        version: "e87f176479d0855a907a41277aca2f8ee7a09523",
        filename: "mmproj-F16.gguf",
        sha256: "cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864",
        size_bytes: 672423616,
        upstream_url: "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/mmproj-F16.gguf",
        origin_key: "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/mmproj-F16.gguf",
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "parakeet-model",
        version: "bf0af9f425fa01809cadec671b3cb672709d13e9",
        filename: "tdt-0.6b-v3-q8_0.gguf",
        sha256: "4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757",
        size_bytes: 940663680,
        upstream_url: "https://huggingface.co/mudler/parakeet-cpp-gguf/resolve/bf0af9f425fa01809cadec671b3cb672709d13e9/tdt-0.6b-v3-q8_0.gguf",
        origin_key: "assets/parakeet-model/bf0af9f425fa01809cadec671b3cb672709d13e9/tdt-0.6b-v3-q8_0.gguf",
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "parakeet-server",
        version: "v0.5.0",
        filename: "parakeet-v0.5.0-bin-linux-cpu-arm64.tar.gz",
        sha256: "a7c9064c64b84f6b041252d5d2334d4a47693636e9c7c6ab2c535fcef11cf88b",
        size_bytes: 1931531,
        upstream_url: "https://github.com/mudler/parakeet.cpp/releases/download/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-arm64.tar.gz",
        origin_key: "assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-arm64.tar.gz",
        artifact_key: Some("aarch64-unknown-linux-gnu"),
        platform: Some(Platform::LinuxArm64),
        backend: Some(Backend::Cpu),
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "parakeet-server",
        version: "v0.5.0",
        filename: "parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz",
        sha256: "636a9fc48ac023096037790f9b77d7e5043b200dd6399ec0438bd648c35d79b9",
        size_bytes: 2103219,
        upstream_url: "https://github.com/mudler/parakeet.cpp/releases/download/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz",
        origin_key: "assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz",
        artifact_key: Some("x86_64-unknown-linux-gnu"),
        platform: Some(Platform::LinuxX64),
        backend: Some(Backend::Cpu),
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "parakeet-server",
        version: "v0.5.0",
        filename: "parakeet-v0.5.0-bin-linux-vulkan-arm64.tar.gz",
        sha256: "b95483070eb87ed144b9f39826a69fb67ea516c68aacc4fcf13a121a746ad7e4",
        size_bytes: 29207915,
        upstream_url: "https://github.com/mudler/parakeet.cpp/releases/download/v0.5.0/parakeet-v0.5.0-bin-linux-vulkan-arm64.tar.gz",
        origin_key: "assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-vulkan-arm64.tar.gz",
        artifact_key: Some("aarch64-unknown-linux-gnu"),
        platform: Some(Platform::LinuxArm64),
        backend: Some(Backend::Vulkan),
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "parakeet-server",
        version: "v0.5.0",
        filename: "parakeet-v0.5.0-bin-linux-vulkan-x64.tar.gz",
        sha256: "36c8d4b93594ec18928c9c76b02e04b2d738e859deda8b5e3944bb34fc0646eb",
        size_bytes: 36864577,
        upstream_url: "https://github.com/mudler/parakeet.cpp/releases/download/v0.5.0/parakeet-v0.5.0-bin-linux-vulkan-x64.tar.gz",
        origin_key: "assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-vulkan-x64.tar.gz",
        artifact_key: Some("x86_64-unknown-linux-gnu"),
        platform: Some(Platform::LinuxX64),
        backend: Some(Backend::Vulkan),
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "rerank-model",
        version: "a09144355adeed5f58c8ed011d209bf8ee5a1fec",
        filename: "onnx/model.onnx",
        sha256: "c623d0bcb99f4622beb413eaef00cfbe5db20df9f1dd982da4b4f26022881870",
        size_bytes: 90992115,
        upstream_url: "https://huggingface.co/Xenova/ms-marco-MiniLM-L-6-v2/resolve/a09144355adeed5f58c8ed011d209bf8ee5a1fec/onnx/model.onnx",
        origin_key: "assets/rerank-model/a09144355adeed5f58c8ed011d209bf8ee5a1fec/onnx/model.onnx",
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "rerank-model",
        version: "a09144355adeed5f58c8ed011d209bf8ee5a1fec",
        filename: "tokenizer.json",
        sha256: "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
        size_bytes: 711396,
        upstream_url: "https://huggingface.co/Xenova/ms-marco-MiniLM-L-6-v2/resolve/a09144355adeed5f58c8ed011d209bf8ee5a1fec/tokenizer.json",
        origin_key: "assets/rerank-model/a09144355adeed5f58c8ed011d209bf8ee5a1fec/tokenizer.json",
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    },
    Artifact {
        unit: "rfdetr-engine",
        version: "bin-65c0ffcc-1",
        filename: "rfdetr-cli-65c0ffcc-linux-cpu-x64.tar.gz",
        sha256: "74f3258a94c975444923be0cc451d90c1e8d9e2595d3cab6876a11086d8357dd",
        size_bytes: 995601,
        upstream_url: "https://github.com/solpbc/rf-detr.cpp/releases/download/bin-65c0ffcc-1/rfdetr-cli-65c0ffcc-linux-cpu-x64.tar.gz",
        origin_key: "assets/rfdetr-engine/bin-65c0ffcc-1/rfdetr-cli-65c0ffcc-linux-cpu-x64.tar.gz",
        artifact_key: Some("linux-cpu-x64"),
        platform: Some(Platform::LinuxX64),
        backend: Some(Backend::Cpu),
        extracted_binary_sha256: Some(
            "7c4fb4d499d53509d5099e768510a164c6647b84480c72170b865233504f367c",
        ),
    },
    Artifact {
        unit: "rfdetr-model",
        version: "c3dc0c037df499f5503545247df6618415fca643",
        filename: "rfdetr-nano-f16.gguf",
        sha256: "d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee",
        size_bytes: 63439488,
        upstream_url: "https://huggingface.co/mudler/rfdetr-cpp-nano/resolve/c3dc0c037df499f5503545247df6618415fca643/rfdetr-nano-f16.gguf",
        origin_key: "assets/rfdetr-model/c3dc0c037df499f5503545247df6618415fca643/rfdetr-nano-f16.gguf",
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    },
];

static VALIDATED: LazyLock<()> = LazyLock::new(|| {
    for artifact in ARTIFACTS {
        if let Err(error) = check_version(artifact.version) {
            panic!(
                "invalid asset version for {}/{}: {error}",
                artifact.unit, artifact.filename
            );
        }
    }
});

pub fn catalog() -> &'static [Artifact] {
    LazyLock::force(&VALIDATED);
    ARTIFACTS
}

pub fn resolve(
    unit: &str,
    platform: Option<Platform>,
    backend: Option<Backend>,
) -> Vec<&'static Artifact> {
    catalog()
        .iter()
        .filter(|artifact| {
            artifact.unit == unit && artifact.platform == platform && artifact.backend == backend
        })
        .collect()
}

pub fn assets_json() -> String {
    serde_json::to_string(catalog()).expect("static artifact catalog serializes")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn catalog_has_the_handed_down_sha_and_size_multiset() {
        let expected = BTreeSet::from([
            (
                "a87de0a8b086429aa5d6544a6f881a70e62726d07901734640ac85dbf146181e",
                720034,
            ),
            (
                "915e0573bc4e17197a7a893d0eb98e1a851abb64451b2e1a8ad51f5f99040360",
                788651,
            ),
            (
                "4c913ba0ece1d06ba2210da9fcaee3d8199ca3c62697c331810f224444e4054b",
                686952,
            ),
            (
                "48bee4e2fc3cc85d7806e03471db24e77fda6c2a2e81ffe9ef67caebaf2bd674",
                6211616,
            ),
            (
                "3727630e6ac79953f5c652fddcfd7100da98c55d773c0aec115a55f40f3aafea",
                550238443,
            ),
            (
                "6de68319db40e8c0eb45dc4bd3a45a16971dbdc128f2b621b19bef5dae87d064",
                654508507,
            ),
            (
                "13aa2d40c76ad1dcb8ebeec5f0d2814bf3b2f84a66935c7d4dc6f7cca8e38d68",
                10603591,
            ),
            (
                "c3c49e6e124a574165ca28317be021b1a12a2ea06977e3eb7daee3eb443eb186",
                26119233,
            ),
            (
                "713641920dce6c8efb953ebc9ffa309977e200cec5e182e6ad0e8b086203cdc3",
                32028597,
            ),
            (
                "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
                2740937888,
            ),
            (
                "cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864",
                672423616,
            ),
            (
                "4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757",
                940663680,
            ),
            (
                "a7c9064c64b84f6b041252d5d2334d4a47693636e9c7c6ab2c535fcef11cf88b",
                1931531,
            ),
            (
                "636a9fc48ac023096037790f9b77d7e5043b200dd6399ec0438bd648c35d79b9",
                2103219,
            ),
            (
                "b95483070eb87ed144b9f39826a69fb67ea516c68aacc4fcf13a121a746ad7e4",
                29207915,
            ),
            (
                "36c8d4b93594ec18928c9c76b02e04b2d738e859deda8b5e3944bb34fc0646eb",
                36864577,
            ),
            (
                "c623d0bcb99f4622beb413eaef00cfbe5db20df9f1dd982da4b4f26022881870",
                90992115,
            ),
            (
                "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
                711396,
            ),
            (
                "74f3258a94c975444923be0cc451d90c1e8d9e2595d3cab6876a11086d8357dd",
                995601,
            ),
            (
                "d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee",
                63439488,
            ),
        ]);
        let actual: BTreeSet<(&str, u64)> = catalog()
            .iter()
            .map(|artifact| (artifact.sha256, artifact.size_bytes))
            .collect();
        assert_eq!(catalog().len(), 20);
        assert_eq!(actual, expected);
    }

    #[test]
    fn selectors_return_complete_file_sets_without_ordering_contracts() {
        assert_eq!(resolve("local-model", None, None).len(), 2);
        assert_eq!(resolve("rerank-model", None, None).len(), 2);
        assert_eq!(
            resolve("llama-server-cuda", Some(Platform::LinuxX64), None).len(),
            1
        );
        assert!(resolve("missing", None, None).is_empty());
    }

    #[test]
    fn versions_reject_each_mutable_or_blank_form() {
        for version in [
            "",
            "   ",
            " v0.1.0",
            "v0.1.0 ",
            "main",
            "LATEST",
            "HEAD",
            "master",
            "develop",
            "development",
            "trunk",
            "default",
            "stable",
            "edge",
            "nightly",
            "canary",
        ] {
            assert!(
                check_version(version).is_err(),
                "{version:?} must be rejected"
            );
        }
    }

    #[test]
    fn immutable_versions_pass_the_denylist() {
        for version in [
            "v0.1.0",
            "b10068",
            "v0.5.0",
            "bin-65c0ffcc-1",
            "b5e9a4aad6438763c8da16079d77563fbed35c65",
            "e87f176479d0855a907a41277aca2f8ee7a09523",
            "bf0af9f425fa01809cadec671b3cb672709d13e9",
            "a09144355adeed5f58c8ed011d209bf8ee5a1fec",
            "c3dc0c037df499f5503545247df6618415fca643",
        ] {
            assert_eq!(check_version(version), Ok(()));
        }
    }

    #[test]
    fn json_round_trips_every_row() {
        let decoded: Vec<serde_json::Value> = serde_json::from_str(&assets_json()).unwrap();
        let expected = serde_json::to_value(catalog())
            .unwrap()
            .as_array()
            .unwrap()
            .to_vec();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn origin_keys_follow_the_mirror_convention_with_only_cuda_exceptions() {
        let exceptions: Vec<_> = catalog()
            .iter()
            .filter(|artifact| artifact.unit == "llama-server-cuda")
            .collect();
        assert_eq!(exceptions.len(), 2);

        for artifact in catalog() {
            if artifact.unit == "llama-server-cuda" {
                continue;
            }
            assert_eq!(
                artifact.origin_key,
                format!(
                    "assets/{}/{}/{}",
                    artifact.unit, artifact.version, artifact.filename
                )
            );
        }

        assert!(exceptions.iter().any(|artifact| {
            artifact.origin_key
                == "runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz"
        }));
        assert!(exceptions.iter().any(|artifact| {
            artifact.origin_key
                == "runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-arm64-sol1.tar.gz"
        }));
    }

    #[test]
    fn rfdetr_extracted_digest_is_metadata_not_a_row() {
        let row = catalog()
            .iter()
            .find(|artifact| artifact.unit == "rfdetr-engine")
            .unwrap();
        assert_eq!(
            row.extracted_binary_sha256,
            Some("7c4fb4d499d53509d5099e768510a164c6647b84480c72170b865233504f367c")
        );
        assert!(
            !catalog()
                .iter()
                .any(|artifact| artifact.sha256 == row.extracted_binary_sha256.unwrap())
        );
    }
}
