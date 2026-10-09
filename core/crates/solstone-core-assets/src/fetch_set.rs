// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Per-target runtime-fetch set and unit mapping.

use std::cmp::Ordering;
use std::fmt;

use crate::{Artifact, catalog};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFetch {
    unit: &'static str,
    origin_key: &'static str,
    sha256: &'static str,
    size_bytes: u64,
}

impl RuntimeFetch {
    #[must_use]
    pub fn unit(&self) -> &'static str {
        self.unit
    }

    #[must_use]
    pub fn origin_key(&self) -> &'static str {
        self.origin_key
    }

    #[must_use]
    pub fn sha256(&self) -> &'static str {
        self.sha256
    }

    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchSetError {
    UnknownTarget(String),
}

impl fmt::Display for FetchSetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTarget(target) => write!(f, "unknown-target: {target}"),
        }
    }
}

impl std::error::Error for FetchSetError {}

/// Map a catalog or authority unit to its public component ID.
///
/// The only non-identity mapping is `llama-server-vulkan` -> `llama-server`.
#[must_use]
pub fn unit_id(unit: &str) -> Option<&'static str> {
    match unit {
        "llama-server" | "llama-server-vulkan" => Some("llama-server"),
        "local-model" | "llama-server-cuda" | "parakeet-server" | "parakeet-model"
        | "parakeet-coreml" | "ced-engine" | "ced-model" | "restic" | "rclone" | "nvattest" => {
            Some(match unit {
                "local-model" => "local-model",
                "llama-server-cuda" => "llama-server-cuda",
                "parakeet-server" => "parakeet-server",
                "parakeet-model" => "parakeet-model",
                "parakeet-coreml" => "parakeet-coreml",
                "ced-engine" => "ced-engine",
                "ced-model" => "ced-model",
                "restic" => "restic",
                "rclone" => "rclone",
                "nvattest" => "nvattest",
                _ => unreachable!(),
            })
        }
        _ => None,
    }
}

/// Compiled bundled catalog-unit IDs per target.
///
/// POSIX targets bundle: ced-engine, ced-model, restic, rclone, nvattest.
/// Windows bundles: llama-server, parakeet-server, parakeet-model, ced-engine, ced-model,
/// restic, rclone, nvattest.
static COMPILED_BUNDLED_IDS: &[(&str, &[&str])] = &[
    (
        "linux-x86_64",
        &["ced-engine", "ced-model", "restic", "rclone", "nvattest"],
    ),
    (
        "linux-aarch64",
        &["ced-engine", "ced-model", "restic", "rclone", "nvattest"],
    ),
    (
        "macos-arm64",
        &["ced-engine", "ced-model", "restic", "rclone", "nvattest"],
    ),
    (
        "windows-x86_64",
        &[
            "llama-server",
            "parakeet-server",
            "parakeet-model",
            "ced-engine",
            "ced-model",
            "restic",
            "rclone",
            "nvattest",
        ],
    ),
];

/// Return compiled bundled catalog IDs for `target`.
#[must_use]
pub fn bundled_ids(target: &str) -> &'static [&'static str] {
    COMPILED_BUNDLED_IDS
        .iter()
        .find(|(t, _)| *t == target)
        .map(|(_, ids)| *ids)
        .unwrap_or(&[])
}

#[cfg(test)]
thread_local! {
    static BUNDLED_OVERRIDE: std::cell::Cell<Option<(&'static str, &'static [&'static str])>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub fn with_bundled_ids_override<T>(
    target: &'static str,
    ids: &'static [&'static str],
    f: impl FnOnce() -> T,
) -> T {
    struct Guard(Option<(&'static str, &'static [&'static str])>);
    impl Drop for Guard {
        fn drop(&mut self) {
            BUNDLED_OVERRIDE.with(|cell| cell.set(self.0));
        }
    }
    let prev = BUNDLED_OVERRIDE.with(|cell| cell.replace(Some((target, ids))));
    let _guard = Guard(prev);
    f()
}

fn effective_bundled_ids(target: &str) -> &'static [&'static str] {
    #[cfg(test)]
    {
        if let Some((t, ids)) = BUNDLED_OVERRIDE.with(|cell| cell.get())
            && t == target
        {
            return ids;
        }
    }
    bundled_ids(target)
}

/// Compare strings in UTF-16 code-unit order.
///
/// Example: `"\u{10000}"` (surrogate pair `[0xD800, 0xDC00]`) sorts before
/// `"\u{E000}"` (`[0xE000]`) in UTF-16 code units, whereas in UTF-8 byte
/// order (standard Rust `&str::cmp`), `"\u{E000}"` (`0xEE 0x80 0x80`) sorts
/// before `"\u{10000}"` (`0xF0 0x90 0x80 0x80`).
#[must_use]
pub fn cmp_utf16_code_units(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn find_catalog_row(unit: &str, filename: &str, artifact_key: Option<&str>) -> &'static Artifact {
    catalog()
        .iter()
        .find(|a| {
            a.unit == unit
                && a.filename == filename
                && (artifact_key.is_none() || a.artifact_key == artifact_key)
        })
        .unwrap_or_else(|| panic!("catalog missing {unit}/{filename}"))
}

fn nvattest_row(target: &'static str) -> RuntimeFetch {
    use std::sync::OnceLock;
    static NVAT_CACHE: OnceLock<[(&'static str, &'static str, u64); 3]> = OnceLock::new();
    let entries = NVAT_CACHE.get_or_init(|| {
        let authority = solstone_core_nvattest_authority::parse(
            solstone_core_nvattest_authority::AUTHORITY_JSON,
        )
        .expect("nvattest authority parses");
        let targets = ["linux-x86_64", "linux-aarch64", "macos-arm64"];
        let mut res: [(&'static str, &'static str, u64); 3] = [("", "", 0); 3];
        for (i, &t) in targets.iter().enumerate() {
            let spec = solstone_core_nvattest_authority::artifact_spec(&authority, t)
                .expect("nvattest spec");
            let key = Box::leak(spec.origin_key.into_boxed_str());
            let sha = Box::leak(spec.sha256.into_boxed_str());
            res[i] = (key, sha, spec.size_bytes);
        }
        res
    });
    let (key, sha, size) = match target {
        "linux-x86_64" => entries[0],
        "linux-aarch64" => entries[1],
        "macos-arm64" => entries[2],
        _ => unreachable!(),
    };
    RuntimeFetch {
        unit: "nvattest",
        origin_key: key,
        sha256: sha,
        size_bytes: size,
    }
}

fn candidates(target: &str) -> Result<Vec<RuntimeFetch>, FetchSetError> {
    let mut rows = Vec::new();

    match target {
        "linux-x86_64" => {
            // ced-engine linux-cpu-x64
            let a = find_catalog_row(
                "ced-engine",
                "ced-v0.1.0-lib-linux-cpu-x64.tar.gz",
                Some("linux-cpu-x64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // ced-model
            let a = find_catalog_row("ced-model", "ced-tiny-q8_0.gguf", None);
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // llama-server-vulkan x86_64
            let a = find_catalog_row(
                "llama-server-vulkan",
                "llama-b11429-bin-ubuntu-vulkan-x64.tar.gz",
                Some("x86_64-unknown-linux-gnu"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // local-model both files
            let a1 = find_catalog_row("local-model", "Qwen3.5-4B-Q4_K_M.gguf", None);
            rows.push(RuntimeFetch {
                unit: a1.unit,
                origin_key: a1.origin_key,
                sha256: a1.sha256,
                size_bytes: a1.size_bytes,
            });
            let a2 = find_catalog_row("local-model", "mmproj-F16.gguf", None);
            rows.push(RuntimeFetch {
                unit: a2.unit,
                origin_key: a2.origin_key,
                sha256: a2.sha256,
                size_bytes: a2.size_bytes,
            });

            // parakeet-model
            let a = find_catalog_row("parakeet-model", "tdt-0.6b-v3-q8_0.gguf", None);
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // parakeet-server CPU & Vulkan x64
            let a1 = find_catalog_row(
                "parakeet-server",
                "parakeet-v0.6.1-bin-linux-cpu-x64.tar.gz",
                Some("x86_64-unknown-linux-gnu"),
            );
            rows.push(RuntimeFetch {
                unit: a1.unit,
                origin_key: a1.origin_key,
                sha256: a1.sha256,
                size_bytes: a1.size_bytes,
            });
            let a2 = find_catalog_row(
                "parakeet-server",
                "parakeet-v0.6.1-bin-linux-vulkan-x64.tar.gz",
                Some("x86_64-unknown-linux-gnu"),
            );
            rows.push(RuntimeFetch {
                unit: a2.unit,
                origin_key: a2.origin_key,
                sha256: a2.sha256,
                size_bytes: a2.size_bytes,
            });

            // rclone linux-amd64
            let a = find_catalog_row(
                "rclone",
                "rclone-v1.74.4-linux-amd64.zip",
                Some("linux-amd64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // restic linux-amd64
            let a = find_catalog_row(
                "restic",
                "restic_0.19.0_linux_amd64.bz2",
                Some("linux-amd64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // nvattest linux-x86_64
            rows.push(nvattest_row("linux-x86_64"));

            // llama-server-cuda x86_64
            let a = find_catalog_row(
                "llama-server-cuda",
                "llama-b11429-bin-linux-cuda13-amd64-sol1.tar.gz",
                Some("x86_64-unknown-linux-gnu"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });
        }
        "linux-aarch64" => {
            // ced-engine linux-cpu-arm64
            let a = find_catalog_row(
                "ced-engine",
                "ced-v0.1.0-lib-linux-cpu-arm64.tar.gz",
                Some("linux-cpu-arm64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // ced-model
            let a = find_catalog_row("ced-model", "ced-tiny-q8_0.gguf", None);
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // llama-server-vulkan arm64
            let a = find_catalog_row(
                "llama-server-vulkan",
                "llama-b11429-bin-ubuntu-vulkan-arm64.tar.gz",
                Some("aarch64-unknown-linux-gnu"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // local-model both files
            let a1 = find_catalog_row("local-model", "Qwen3.5-4B-Q4_K_M.gguf", None);
            rows.push(RuntimeFetch {
                unit: a1.unit,
                origin_key: a1.origin_key,
                sha256: a1.sha256,
                size_bytes: a1.size_bytes,
            });
            let a2 = find_catalog_row("local-model", "mmproj-F16.gguf", None);
            rows.push(RuntimeFetch {
                unit: a2.unit,
                origin_key: a2.origin_key,
                sha256: a2.sha256,
                size_bytes: a2.size_bytes,
            });

            // parakeet-model
            let a = find_catalog_row("parakeet-model", "tdt-0.6b-v3-q8_0.gguf", None);
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // parakeet-server CPU & Vulkan arm64
            let a1 = find_catalog_row(
                "parakeet-server",
                "parakeet-v0.6.1-bin-linux-cpu-arm64.tar.gz",
                Some("aarch64-unknown-linux-gnu"),
            );
            rows.push(RuntimeFetch {
                unit: a1.unit,
                origin_key: a1.origin_key,
                sha256: a1.sha256,
                size_bytes: a1.size_bytes,
            });
            let a2 = find_catalog_row(
                "parakeet-server",
                "parakeet-v0.6.1-bin-linux-vulkan-arm64.tar.gz",
                Some("aarch64-unknown-linux-gnu"),
            );
            rows.push(RuntimeFetch {
                unit: a2.unit,
                origin_key: a2.origin_key,
                sha256: a2.sha256,
                size_bytes: a2.size_bytes,
            });

            // rclone linux-arm64
            let a = find_catalog_row(
                "rclone",
                "rclone-v1.74.4-linux-arm64.zip",
                Some("linux-arm64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // restic linux-arm64
            let a = find_catalog_row(
                "restic",
                "restic_0.19.0_linux_arm64.bz2",
                Some("linux-arm64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // nvattest linux-aarch64
            rows.push(nvattest_row("linux-aarch64"));

            // llama-server-cuda arm64
            let a = find_catalog_row(
                "llama-server-cuda",
                "llama-b11429-bin-linux-cuda13-arm64-sol1.tar.gz",
                Some("aarch64-unknown-linux-gnu"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });
        }
        "macos-arm64" => {
            // ced-engine macos-metal-arm64
            let a = find_catalog_row(
                "ced-engine",
                "ced-v0.1.0-lib-macos-metal-arm64.tar.gz",
                Some("macos-metal-arm64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // ced-model
            let a = find_catalog_row("ced-model", "ced-tiny-q8_0.gguf", None);
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // llama-server-vulkan macos-arm64
            let a = find_catalog_row(
                "llama-server-vulkan",
                "llama-b11429-bin-macos-arm64.tar.gz",
                Some("aarch64-apple-darwin"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // local-model both files
            let a1 = find_catalog_row("local-model", "Qwen3.5-4B-Q4_K_M.gguf", None);
            rows.push(RuntimeFetch {
                unit: a1.unit,
                origin_key: a1.origin_key,
                sha256: a1.sha256,
                size_bytes: a1.size_bytes,
            });
            let a2 = find_catalog_row("local-model", "mmproj-F16.gguf", None);
            rows.push(RuntimeFetch {
                unit: a2.unit,
                origin_key: a2.origin_key,
                sha256: a2.sha256,
                size_bytes: a2.size_bytes,
            });

            // all 23 parakeet-coreml rows from catalog
            for a in catalog().iter().filter(|a| a.unit == "parakeet-coreml") {
                rows.push(RuntimeFetch {
                    unit: a.unit,
                    origin_key: a.origin_key,
                    sha256: a.sha256,
                    size_bytes: a.size_bytes,
                });
            }

            // rclone darwin-arm64
            let a = find_catalog_row(
                "rclone",
                "rclone-v1.74.4-osx-arm64.zip",
                Some("darwin-arm64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // restic darwin-arm64
            let a = find_catalog_row(
                "restic",
                "restic_0.19.0_darwin_arm64.bz2",
                Some("darwin-arm64"),
            );
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });

            // nvattest macos-arm64
            rows.push(nvattest_row("macos-arm64"));
        }
        "windows-x86_64" => {
            // ced-model only
            let a = find_catalog_row("ced-model", "ced-tiny-q8_0.gguf", None);
            rows.push(RuntimeFetch {
                unit: a.unit,
                origin_key: a.origin_key,
                sha256: a.sha256,
                size_bytes: a.size_bytes,
            });
        }
        _ => return Err(FetchSetError::UnknownTarget(target.to_owned())),
    }

    Ok(rows)
}

/// Compute the per-target runtime-fetch set.
///
/// Returns entries sorted by origin_key according to `cmp_utf16_code_units`.
pub fn runtime_fetch_set(target: &str) -> Result<Vec<RuntimeFetch>, FetchSetError> {
    let mut cand = candidates(target)?;
    let bundled = effective_bundled_ids(target);

    cand.retain(|item| {
        let Some(id) = unit_id(item.unit()) else {
            return true;
        };
        !bundled.contains(&id)
    });

    cand.sort_by(|a, b| cmp_utf16_code_units(a.origin_key(), b.origin_key()));
    Ok(cand)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const ORACLE_LINUX_X86_64: &[&str] = &[
        "assets/llama-server-vulkan/b11429/llama-b11429-bin-ubuntu-vulkan-x64.tar.gz",
        "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
        "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/mmproj-F16.gguf",
        "assets/parakeet-model/bf0af9f425fa01809cadec671b3cb672709d13e9/tdt-0.6b-v3-q8_0.gguf",
        "assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-cpu-x64.tar.gz",
        "assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-vulkan-x64.tar.gz",
        "runtimes/llama-cuda13/b11429/llama-b11429-bin-linux-cuda13-amd64-sol1.tar.gz",
    ];

    const ORACLE_LINUX_AARCH64: &[&str] = &[
        "assets/llama-server-vulkan/b11429/llama-b11429-bin-ubuntu-vulkan-arm64.tar.gz",
        "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
        "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/mmproj-F16.gguf",
        "assets/parakeet-model/bf0af9f425fa01809cadec671b3cb672709d13e9/tdt-0.6b-v3-q8_0.gguf",
        "assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-cpu-arm64.tar.gz",
        "assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-vulkan-arm64.tar.gz",
        "runtimes/llama-cuda13/b11429/llama-b11429-bin-linux-cuda13-arm64-sol1.tar.gz",
    ];

    const ORACLE_MACOS_ARM64: &[&str] = &[
        "assets/llama-server-vulkan/b11429/llama-b11429-bin-macos-arm64.tar.gz",
        "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
        "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/mmproj-F16.gguf",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Decoder.mlmodelc/analytics/coremldata.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Decoder.mlmodelc/coremldata.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Decoder.mlmodelc/metadata.json",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Decoder.mlmodelc/model.mil",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Decoder.mlmodelc/weights/weight.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Encoder.mlmodelc/analytics/coremldata.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Encoder.mlmodelc/coremldata.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Encoder.mlmodelc/metadata.json",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Encoder.mlmodelc/model.mil",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Encoder.mlmodelc/weights/weight.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/JointDecision.mlmodelc/analytics/coremldata.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/JointDecision.mlmodelc/coremldata.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/JointDecision.mlmodelc/metadata.json",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/JointDecision.mlmodelc/model.mil",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/JointDecision.mlmodelc/weights/weight.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Preprocessor.mlmodelc/analytics/coremldata.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Preprocessor.mlmodelc/coremldata.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Preprocessor.mlmodelc/metadata.json",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Preprocessor.mlmodelc/model.mil",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Preprocessor.mlmodelc/weights/weight.bin",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/config.json",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/parakeet_v3_vocab.json",
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/parakeet_vocab.json",
    ];

    const ORACLE_WINDOWS_X86_64: &[&str] = &[];

    #[test]
    fn oracle_equality_for_all_four_targets() {
        for (target, oracle) in [
            ("linux-x86_64", ORACLE_LINUX_X86_64),
            ("linux-aarch64", ORACLE_LINUX_AARCH64),
            ("macos-arm64", ORACLE_MACOS_ARM64),
            ("windows-x86_64", ORACLE_WINDOWS_X86_64),
        ] {
            let set = runtime_fetch_set(target).expect("runtime_fetch_set succeeds");
            let keys: Vec<_> = set.iter().map(RuntimeFetch::origin_key).collect();
            assert_eq!(keys, oracle, "mismatch for {target}");
        }
    }

    #[test]
    fn unknown_target_is_rejected() {
        for target in ["windows", "darwin", "invalid", "linux", ""] {
            let err = runtime_fetch_set(target).expect_err("unknown target rejected");
            assert_eq!(err.to_string(), format!("unknown-target: {target}"));
        }
    }

    #[test]
    fn packaged_restic_and_rclone_leave_the_fetch_set() {
        // The package ships these members, so the real set omits them. With nothing
        // bundled, each target still composes exactly one catalog candidate.
        let expected = [
            (
                "linux-x86_64",
                "assets/restic/0.19.0/restic_0.19.0_linux_amd64.bz2",
                "assets/rclone/1.74.4/rclone-v1.74.4-linux-amd64.zip",
            ),
            (
                "linux-aarch64",
                "assets/restic/0.19.0/restic_0.19.0_linux_arm64.bz2",
                "assets/rclone/1.74.4/rclone-v1.74.4-linux-arm64.zip",
            ),
            (
                "macos-arm64",
                "assets/restic/0.19.0/restic_0.19.0_darwin_arm64.bz2",
                "assets/rclone/1.74.4/rclone-v1.74.4-osx-arm64.zip",
            ),
        ];
        for (target, restic_key, rclone_key) in expected {
            let set = runtime_fetch_set(target).expect("fetch set");
            assert!(!set.iter().any(|f| f.unit() == "restic"), "{target}");
            assert!(!set.iter().any(|f| f.unit() == "rclone"), "{target}");
            with_bundled_ids_override(target, &[], || {
                let set = runtime_fetch_set(target).expect("fetch set");
                let restic: Vec<_> = set.iter().filter(|f| f.unit() == "restic").collect();
                let rclone: Vec<_> = set.iter().filter(|f| f.unit() == "rclone").collect();
                assert_eq!(restic.len(), 1, "{target}");
                assert_eq!(rclone.len(), 1, "{target}");
                assert_eq!(restic[0].origin_key(), restic_key);
                assert_eq!(rclone[0].origin_key(), rclone_key);
            });
        }
    }

    #[test]
    fn unit_partition_covers_all_entries_cleanly() {
        let local_units = [
            "ced-engine",
            "ced-model",
            "llama-server-vulkan",
            "llama-server-cuda",
            "local-model",
            "parakeet-server",
            "parakeet-model",
            "parakeet-coreml",
        ];
        let backup_units = ["restic", "rclone"];
        let nvattest_units = ["nvattest"];

        let set1: BTreeSet<_> = local_units.into_iter().collect();
        let set2: BTreeSet<_> = backup_units.into_iter().collect();
        let set3: BTreeSet<_> = nvattest_units.into_iter().collect();

        assert!(set1.is_disjoint(&set2));
        assert!(set1.is_disjoint(&set3));
        assert!(set2.is_disjoint(&set3));

        for target in [
            "linux-x86_64",
            "linux-aarch64",
            "macos-arm64",
            "windows-x86_64",
        ] {
            let set = runtime_fetch_set(target).unwrap();
            for item in set {
                let u = item.unit();
                assert!(
                    set1.contains(u) || set2.contains(u) || set3.contains(u),
                    "unexpected unit {u} in {target}"
                );
            }
        }
    }

    #[test]
    fn cmp_utf16_code_units_ordering_oracle() {
        let u10000 = "\u{10000}";
        let ue000 = "\u{E000}";
        // In UTF-16 code units: 0xD800 < 0xE000, so u10000 < ue000.
        assert_eq!(cmp_utf16_code_units(u10000, ue000), Ordering::Less);
        // In UTF-8 bytes (&str cmp): 0xF0 > 0xEE, so u10000 > ue000.
        assert_eq!(u10000.cmp(ue000), Ordering::Greater);
    }

    #[test]
    fn macos_coreml_origin_keys_match_catalog_exact_rows() {
        let set = runtime_fetch_set("macos-arm64").unwrap();
        let coreml_set_keys: BTreeSet<_> = set
            .iter()
            .filter(|f| f.unit() == "parakeet-coreml")
            .map(RuntimeFetch::origin_key)
            .collect();

        let catalog_keys: BTreeSet<_> = catalog()
            .iter()
            .filter(|a| a.unit == "parakeet-coreml")
            .map(|a| a.origin_key)
            .collect();

        assert_eq!(coreml_set_keys, catalog_keys);
    }

    #[test]
    fn nvattest_origin_keys_match_authority_spec() {
        let authority = solstone_core_nvattest_authority::parse(
            solstone_core_nvattest_authority::AUTHORITY_JSON,
        )
        .expect("nvattest authority parses");

        for target in ["linux-x86_64", "linux-aarch64", "macos-arm64"] {
            let spec = solstone_core_nvattest_authority::artifact_spec(&authority, target)
                .expect("nvattest spec");
            // The verifier ships in the package, so it is never in the real set; with
            // nothing bundled, its candidate key still composes from the authority.
            let set = runtime_fetch_set(target).expect("fetch set");
            assert!(!set.iter().any(|f| f.unit() == "nvattest"), "{target}");
            let expected_key = format!("providers/nvattest/{}", spec.name);
            with_bundled_ids_override(target, &[], || {
                let set = runtime_fetch_set(target).expect("fetch set");
                let nvat_entries: Vec<_> = set.iter().filter(|f| f.unit() == "nvattest").collect();
                assert_eq!(
                    nvat_entries.len(),
                    1,
                    "expected exactly 1 nvattest candidate for {target}"
                );
                assert_eq!(nvat_entries[0].origin_key(), expected_key);
            });
        }

        let win_set = runtime_fetch_set("windows-x86_64").expect("windows fetch set");
        assert!(!win_set.iter().any(|f| f.unit() == "nvattest"));

        for target in [
            "linux-x86_64",
            "linux-aarch64",
            "macos-arm64",
            "windows-x86_64",
        ] {
            let set = runtime_fetch_set(target).expect("fetch set");
            for f in set {
                assert_ne!(f.unit(), "mlx-snapshot");
                assert!(!f.origin_key().contains("darwin-amd64"));
                assert!(!f.origin_key().contains("darwin_amd64"));
            }
        }
    }
}
