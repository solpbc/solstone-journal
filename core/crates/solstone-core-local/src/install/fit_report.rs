// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host fit checks for provider artifact installation.

use std::path::{Path, PathBuf};

use nix::sys::statvfs::statvfs;
use solstone_core_assets::resolve;

use super::{pins, rfdetr_install::rfdetr_artifact_key};
use crate::vulkan::{cpu_placement_suffix, select_device};
use crate::{Backend, BackendChoice, MemorySource, NvidiaProbe, VulkanDevice};

const LOCAL_MIN_RAM_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitSeverity {
    Ok,
    Warning,
    Blocked,
    Unknown,
}

impl FitSeverity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitCheck {
    pub name: &'static str,
    pub severity: FitSeverity,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitReport {
    pub artifact: String,
    pub checks: Vec<FitCheck>,
}

impl FitReport {
    pub fn overall(&self) -> FitSeverity {
        if self
            .checks
            .iter()
            .any(|check| check.severity == FitSeverity::Blocked)
        {
            FitSeverity::Blocked
        } else if self
            .checks
            .iter()
            .any(|check| matches!(check.severity, FitSeverity::Warning | FitSeverity::Unknown))
        {
            FitSeverity::Warning
        } else {
            FitSeverity::Ok
        }
    }
}

pub fn render_fit_report(report: &FitReport) -> String {
    let mut lines = vec![format!(
        "{} fit check: {}",
        report.artifact,
        report.overall().as_str()
    )];
    lines.extend(report.checks.iter().map(|check| {
        format!(
            "[{}] {}: {}",
            check.severity.as_str(),
            check.name,
            check.detail
        )
    }));
    lines.join("\n")
}

pub fn build_parakeet_fit_report(journal: &Path, os_name: &str, arch: &str) -> FitReport {
    build_parakeet_fit_report_with_free_bytes(
        journal,
        os_name,
        arch,
        free_bytes(&pins::parakeet_cache_root(journal)),
    )
}

pub fn build_parakeet_fit_report_with_free_bytes(
    journal: &Path,
    os_name: &str,
    arch: &str,
    available: Result<u64, String>,
) -> FitReport {
    let platform = match pins::parakeet_artifact_key(os_name, arch) {
        Ok(key) => FitCheck {
            name: "platform",
            severity: FitSeverity::Ok,
            detail: format!("pinned parakeet.cpp artifacts are available for {key}"),
        },
        Err(error) => FitCheck {
            name: "platform",
            severity: FitSeverity::Blocked,
            detail: error.to_string(),
        },
    };
    let cache = pins::parakeet_cache_root(journal);
    let disk = disk_check(
        &cache,
        available,
        &[("parakeet GGUF model", pins::PARAKEET_MODEL.4)],
        &[
            "parakeet CPU server tarball",
            "parakeet Vulkan server tarball",
        ],
        "known downloads",
    );
    FitReport {
        artifact: "parakeet.cpp artifacts".to_string(),
        checks: vec![platform, disk],
    }
}

pub fn build_rfdetr_fit_report(journal: &Path, os_name: &str, arch: &str) -> FitReport {
    build_rfdetr_fit_report_with_free_bytes(
        journal,
        os_name,
        arch,
        free_bytes(&journal.join("cache/providers/rfdetr")),
    )
}

pub fn build_rfdetr_fit_report_with_free_bytes(
    journal: &Path,
    os_name: &str,
    arch: &str,
    available: Result<u64, String>,
) -> FitReport {
    let platform = match rfdetr_artifact_key(os_name, arch) {
        Some(key) => FitCheck {
            name: "platform",
            severity: FitSeverity::Ok,
            detail: format!("pinned rf-detr.cpp artifacts are available for {key}"),
        },
        None => FitCheck {
            name: "platform",
            severity: FitSeverity::Blocked,
            detail: format!(
                "rf-detr.cpp requires darwin/arm64, linux/x86_64, or linux/arm64; got {os_name}/{arch}"
            ),
        },
    };
    let cache = journal.join("cache/providers/rfdetr");
    let disk = disk_check(
        &cache,
        available,
        &[
            ("rf-detr GGUF model", 63_439_488),
            ("rf-detr CLI binary", 1_048_576),
        ],
        &[],
        "bundled asset installation",
    );
    FitReport {
        artifact: "rf-detr.cpp artifacts".to_string(),
        checks: vec![platform, disk],
    }
}

/// Build the local-provider report from facts collected by the owner-facing
/// verb.  This function intentionally performs no host inspection.
#[allow(clippy::too_many_arguments)]
pub fn build_local_fit_report(
    journal: &Path,
    model_id: &str,
    os_name: &str,
    arch: &str,
    available_disk: Result<u64, String>,
    available_ram: Option<u64>,
    nvidia_probe: &NvidiaProbe,
    backend_choice: &BackendChoice,
    vulkan_probe_ok: bool,
    vulkan_devices: &[VulkanDevice],
    override_index: Option<u32>,
    force_cpu: bool,
) -> FitReport {
    let artifact_key = local_artifact_key(os_name, arch);
    let platform = match pins::vulkan_pin(&artifact_key) {
        Some(_) => FitCheck {
            name: "platform",
            severity: FitSeverity::Ok,
            detail: format!("pinned llama-server artifact is available for {artifact_key}"),
        },
        None => FitCheck {
            name: "platform",
            severity: FitSeverity::Blocked,
            detail: format!("No pinned llama-server artifact for platform {artifact_key}"),
        },
    };
    let ram = local_ram_check(model_id, available_ram);
    let mut known = local_model_downloads(model_id);
    let unknown = if os_name == "linux" && backend_choice.backend == Backend::Cuda {
        match pins::cuda_pin(&artifact_key) {
            Some((_, _, size)) => {
                known.push(("CUDA llama-server tarball", size));
                Vec::new()
            }
            None => vec!["llama-server tarball"],
        }
    } else {
        vec!["llama-server tarball"]
    };
    let disk = disk_check(
        &pins::cache_root(journal),
        available_disk,
        &known,
        &unknown,
        "known downloads",
    );
    let mut checks = vec![platform, ram, disk];
    if os_name == "linux" {
        checks.push(local_gpu_check(
            nvidia_probe,
            backend_choice,
            vulkan_probe_ok,
            vulkan_devices,
            override_index,
            force_cpu,
        ));
    }
    FitReport {
        artifact: "local provider artifacts".to_string(),
        checks,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MemoryVerdict {
    severity: FitSeverity,
}

fn assess_local_memory(available: Option<u64>) -> MemoryVerdict {
    let severity = match available {
        None => FitSeverity::Warning,
        Some(available) if available >= LOCAL_MIN_RAM_BYTES => FitSeverity::Ok,
        Some(_) => FitSeverity::Warning,
    };
    MemoryVerdict { severity }
}

fn local_ram_check(model_id: &str, available: Option<u64>) -> FitCheck {
    let verdict = assess_local_memory(available);
    let detail = match available {
        None => format!("available memory could not be verified for {model_id}"),
        Some(available) if available >= LOCAL_MIN_RAM_BYTES => format!(
            "{} GB available memory meets the {} GB requirement for {model_id}",
            gb_label(available),
            gb_label(LOCAL_MIN_RAM_BYTES),
        ),
        Some(available) => format!(
            "insufficient RAM for {model_id} (need {} GB available, have {} GB available)",
            gb_label(LOCAL_MIN_RAM_BYTES),
            gb_label(available),
        ),
    };
    FitCheck {
        name: "ram",
        severity: verdict.severity,
        detail,
    }
}

fn local_model_downloads(model_id: &str) -> Vec<(&'static str, u64)> {
    assert_eq!(model_id, "local/qwen3.5-4b", "unsupported local model");
    let assets = resolve("local-model", None, None);
    let gguf = assets
        .iter()
        .find(|artifact| artifact.filename == "Qwen3.5-4B-Q4_K_M.gguf")
        .expect("local GGUF asset is catalogued");
    let mmproj = assets
        .iter()
        .find(|artifact| artifact.filename == "mmproj-F16.gguf")
        .expect("local mmproj asset is catalogued");
    vec![
        ("GGUF model", gguf.size_bytes),
        ("mmproj", mmproj.size_bytes),
    ]
}

fn local_artifact_key(os_name: &str, arch: &str) -> String {
    let arch = match arch {
        "arm64" => "aarch64",
        value => value,
    };
    match os_name {
        "linux" => format!("{arch}-unknown-linux-gnu"),
        "darwin" => format!("{arch}-apple-darwin"),
        _ => format!("{arch}-{os_name}"),
    }
}

fn local_gpu_check(
    probe: &NvidiaProbe,
    choice: &BackendChoice,
    vulkan_probe_ok: bool,
    devices: &[VulkanDevice],
    override_index: Option<u32>,
    force_cpu: bool,
) -> FitCheck {
    let selected = select_device(devices, override_index);
    let suffix = cpu_placement_suffix(selected.as_ref(), force_cpu);
    let backend = match choice.backend {
        Backend::Cuda => "cuda",
        Backend::Vulkan => "vulkan",
    };
    if choice.backend == Backend::Cuda {
        if !probe.detected {
            return FitCheck {
                name: "gpu",
                severity: FitSeverity::Unknown,
                detail: format!(
                    "NVIDIA GPU probe unavailable; resolved backend is {backend}: {}",
                    choice.reason
                ),
            };
        }
        if probe.memory_source() == MemorySource::Unavailable {
            return FitCheck {
                name: "gpu",
                severity: FitSeverity::Unknown,
                detail: format!(
                    "resolved backend is {backend}: {}; GPU memory is unknown",
                    choice.reason
                ),
            };
        }
        let unified = probe.memory_source() == MemorySource::SystemAvailable;
        let unified_clause = if unified {
            "; GPU tiering memory uses system MemAvailable"
        } else {
            ""
        };
        return FitCheck {
            name: "gpu",
            severity: FitSeverity::Ok,
            detail: format!(
                "CUDA backend selected: {}{unified_clause}{suffix}",
                choice.reason
            ),
        };
    }
    if !vulkan_probe_ok {
        return FitCheck {
            name: "gpu",
            severity: FitSeverity::Unknown,
            detail: format!(
                "Vulkan GPU probe did not complete; resolved backend is {backend}: {}",
                choice.reason
            ),
        };
    }
    match selected {
        None => FitCheck {
            name: "gpu",
            severity: FitSeverity::Warning,
            detail: format!(
                "no hardware Vulkan GPU selected; resolved backend is {backend}: {}",
                choice.reason
            ),
        },
        Some(selected) => FitCheck {
            name: "gpu",
            severity: FitSeverity::Ok,
            detail: format!(
                "Vulkan GPU selected: {}; resolved backend is {backend}: {}{suffix}",
                selected.name, choice.reason
            ),
        },
    }
}

fn disk_check(
    cache: &Path,
    available: Result<u64, String>,
    known: &[(&str, u64)],
    unknown: &[&str],
    requirement: &str,
) -> FitCheck {
    let required = known.iter().map(|(_, size)| size).sum::<u64>();
    let known_names = known
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ");
    let unknown = if unknown.is_empty() {
        String::new()
    } else {
        format!("unknown download size for {}", unknown.join(", "))
    };
    match available {
        Err(error) => FitCheck {
            name: "disk",
            severity: FitSeverity::Unknown,
            detail: if unknown.is_empty() {
                format!(
                    "available disk space could not be verified at {}: {error}",
                    cache.display()
                )
            } else {
                format!(
                    "available disk space could not be verified at {}: {error}; {unknown}",
                    cache.display()
                )
            },
        },
        Ok(available) if available < required => {
            let mut detail = format!(
                "insufficient disk space for {requirement} (need {} GB, have {} GB free){}{}",
                gb_label(required),
                gb_label(available),
                if known_names.is_empty() { "" } else { ": " },
                known_names,
            );
            if !unknown.is_empty() {
                detail.push_str(&format!("; {unknown}"));
            }
            FitCheck {
                name: "disk",
                severity: FitSeverity::Blocked,
                detail,
            }
        }
        Ok(available) => {
            let detail = if unknown.is_empty() {
                format!(
                    "{} GB free for {} GB {requirement}",
                    gb_label(available),
                    gb_label(required)
                )
            } else {
                format!(
                    "{} GB free; {requirement} need {} GB; {unknown}",
                    gb_label(available),
                    gb_label(required)
                )
            };
            FitCheck {
                name: "disk",
                severity: if unknown.is_empty() {
                    FitSeverity::Ok
                } else {
                    FitSeverity::Warning
                },
                detail,
            }
        }
    }
}

pub fn free_bytes(target: &Path) -> Result<u64, String> {
    let usage_root = nearest_existing_ancestor(target);
    let stats = statvfs(&usage_root).map_err(|error| error.to_string())?;
    let blocks = stats.blocks_available() as u64;
    let fragment_size = stats.fragment_size() as u64;
    blocks
        .checked_mul(fragment_size)
        .ok_or_else(|| "available disk space overflow".to_owned())
}

fn nearest_existing_ancestor(target: &Path) -> PathBuf {
    let mut current = target.to_path_buf();
    while !current.exists() && current != current.parent().unwrap_or(Path::new("")) {
        if !current.pop() {
            break;
        }
    }
    current
}

fn gb_label(bytes: u64) -> String {
    let value = ((bytes as f64 / 1024_f64.powi(3)) * 10.0).round() / 10.0;
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Python parity is text-wise: cases compare severity and detail, not
    // byte-metadata fields. The injected builder matrix has 19 branches; its
    // cross-process subset has 18 because unified memory forces force_cpu=false.
    const BUILDER_BRANCH_COUNT: usize = 19;
    const END_TO_END_BRANCH_COUNT: usize = 18;

    fn probe(
        detected: bool,
        vram_mib: Option<u64>,
        unified_memory_mib: Option<u64>,
    ) -> NvidiaProbe {
        NvidiaProbe {
            schema: "test".to_owned(),
            detected,
            gpu_index: None,
            gpu_name: None,
            compute_cap: None,
            arch: None,
            driver_cuda_major: None,
            vram_mib,
            unified_memory_mib,
            probe_error: None,
        }
    }

    fn choice(backend: Backend) -> BackendChoice {
        BackendChoice {
            backend,
            reason: "test choice".to_owned(),
        }
    }

    fn device() -> VulkanDevice {
        VulkanDevice {
            index: 0,
            name: "Test GPU".to_owned(),
            device_type: Some(2),
            vram_mib: 6144,
        }
    }

    fn local_report(
        probe: &NvidiaProbe,
        choice: &BackendChoice,
        available: Result<u64, String>,
        ram: Option<u64>,
        probe_ok: bool,
        devices: &[VulkanDevice],
        force_cpu: bool,
    ) -> FitReport {
        build_local_fit_report(
            Path::new("/journal"),
            "local/qwen3.5-4b",
            "linux",
            "x86_64",
            available,
            ram,
            probe,
            choice,
            probe_ok,
            devices,
            None,
            force_cpu,
        )
    }

    #[test]
    fn parakeet_report_matches_the_blocked_disk_template() {
        let report = build_parakeet_fit_report_with_free_bytes(
            Path::new("/journal"),
            "linux",
            "arm64",
            Ok(1),
        );
        assert_eq!(report.overall(), FitSeverity::Blocked);
        assert_eq!(
            render_fit_report(&report),
            "parakeet.cpp artifacts fit check: blocked\n[ok] platform: pinned parakeet.cpp artifacts are available for aarch64-unknown-linux-gnu\n[blocked] disk: insufficient disk space for known downloads (need 0.9 GB, have 0 GB free): parakeet GGUF model; unknown download size for parakeet CPU server tarball, parakeet Vulkan server tarball"
        );
    }

    #[test]
    fn local_builder_matrix_has_nineteen_constructible_branches() {
        let mut executed = 0;
        let mut assert_branch =
            |report: FitReport, name: &str, severity: FitSeverity, detail: String| {
                let check = report
                    .checks
                    .iter()
                    .find(|check| check.name == name)
                    .expect("builder includes checked branch");
                assert_eq!(check.severity, severity, "{name}");
                assert_eq!(check.detail, detail, "{name}");
                executed += 1;
            };
        let available = 20_u64 * 1024 * 1024 * 1024;
        let cuda = choice(Backend::Cuda);
        let vulkan = choice(Backend::Vulkan);
        let required = local_model_downloads("local/qwen3.5-4b")
            .iter()
            .map(|(_, size)| size)
            .sum::<u64>();
        let unknown = "unknown download size for llama-server tarball";
        let cpu_suffix = format!("; {}", crate::vulkan::CPU_PLACEMENT_COPY);

        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "platform",
            FitSeverity::Ok,
            "pinned llama-server artifact is available for x86_64-unknown-linux-gnu".to_owned(),
        );
        assert_branch(
            build_local_fit_report(
                Path::new("/journal"),
                "local/qwen3.5-4b",
                "linux",
                "riscv64",
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                &probe(false, None, None),
                &vulkan,
                true,
                &[device()],
                None,
                false,
            ),
            "platform",
            FitSeverity::Blocked,
            "No pinned llama-server artifact for platform riscv64-unknown-linux-gnu".to_owned(),
        );

        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                None,
                true,
                &[device()],
                false,
            ),
            "ram",
            FitSeverity::Warning,
            "available memory could not be verified for local/qwen3.5-4b".to_owned(),
        );
        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "ram",
            FitSeverity::Ok,
            "8 GB available memory meets the 8 GB requirement for local/qwen3.5-4b".to_owned(),
        );
        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                Some(1),
                true,
                &[device()],
                false,
            ),
            "ram",
            FitSeverity::Warning,
            "insufficient RAM for local/qwen3.5-4b (need 8 GB available, have 0 GB available)"
                .to_owned(),
        );

        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Err("disk unavailable".to_owned()),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "disk",
            FitSeverity::Unknown,
            format!(
                "available disk space could not be verified at /journal/cache/providers/local: disk unavailable; {unknown}"
            ),
        );
        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(1),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "disk",
            FitSeverity::Blocked,
            format!(
                "insufficient disk space for known downloads (need {} GB, have 0 GB free): GGUF model, mmproj; {unknown}",
                gb_label(required),
            ),
        );
        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "disk",
            FitSeverity::Warning,
            format!(
                "{} GB free; known downloads need {} GB; {unknown}",
                gb_label(available),
                gb_label(required),
            ),
        );
        let cuda_required = required + pins::cuda_pin("x86_64-unknown-linux-gnu").unwrap().2;
        assert_branch(
            local_report(
                &probe(true, Some(6144), None),
                &cuda,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "disk",
            FitSeverity::Ok,
            format!(
                "{} GB free for {} GB known downloads",
                gb_label(available),
                gb_label(cuda_required),
            ),
        );

        assert_branch(
            local_report(
                &probe(false, None, None),
                &cuda,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "gpu",
            FitSeverity::Unknown,
            "NVIDIA GPU probe unavailable; resolved backend is cuda: test choice".to_owned(),
        );
        assert_branch(
            local_report(
                &probe(true, None, None),
                &cuda,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "gpu",
            FitSeverity::Unknown,
            "resolved backend is cuda: test choice; GPU memory is unknown".to_owned(),
        );
        assert_branch(
            local_report(
                &probe(true, Some(6144), None),
                &cuda,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "gpu",
            FitSeverity::Ok,
            "CUDA backend selected: test choice".to_owned(),
        );
        assert_branch(
            local_report(
                &probe(true, Some(6144), None),
                &cuda,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                true,
            ),
            "gpu",
            FitSeverity::Ok,
            format!("CUDA backend selected: test choice{cpu_suffix}"),
        );
        assert_branch(
            local_report(
                &probe(true, None, Some(6144)),
                &cuda,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "gpu",
            FitSeverity::Ok,
            "CUDA backend selected: test choice; GPU tiering memory uses system MemAvailable"
                .to_owned(),
        );
        assert_branch(
            local_report(
                &probe(true, None, Some(6144)),
                &cuda,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                true,
            ),
            "gpu",
            FitSeverity::Ok,
            format!(
                "CUDA backend selected: test choice; GPU tiering memory uses system MemAvailable{cpu_suffix}"
            ),
        );
        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                false,
                &[device()],
                false,
            ),
            "gpu",
            FitSeverity::Unknown,
            "Vulkan GPU probe did not complete; resolved backend is vulkan: test choice".to_owned(),
        );
        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[],
                false,
            ),
            "gpu",
            FitSeverity::Warning,
            "no hardware Vulkan GPU selected; resolved backend is vulkan: test choice".to_owned(),
        );
        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                false,
            ),
            "gpu",
            FitSeverity::Ok,
            "Vulkan GPU selected: Test GPU; resolved backend is vulkan: test choice".to_owned(),
        );
        assert_branch(
            local_report(
                &probe(false, None, None),
                &vulkan,
                Ok(available),
                Some(LOCAL_MIN_RAM_BYTES),
                true,
                &[device()],
                true,
            ),
            "gpu",
            FitSeverity::Ok,
            format!(
                "Vulkan GPU selected: Test GPU; resolved backend is vulkan: test choice{cpu_suffix}"
            ),
        );

        assert_eq!(executed, BUILDER_BRANCH_COUNT);
        assert_eq!(END_TO_END_BRANCH_COUNT, BUILDER_BRANCH_COUNT - 1);
    }

    #[test]
    fn local_cuda_known_downloads_are_ok_without_unknown_artifacts() {
        let report = local_report(
            &probe(true, Some(6144), None),
            &choice(Backend::Cuda),
            Ok(20_u64 * 1024 * 1024 * 1024),
            Some(LOCAL_MIN_RAM_BYTES),
            true,
            &[device()],
            false,
        );
        let disk = report
            .checks
            .iter()
            .find(|check| check.name == "disk")
            .unwrap();
        assert_eq!(disk.severity, FitSeverity::Ok);
        assert_eq!(disk.detail, "20 GB free for 3.7 GB known downloads");
    }

    #[test]
    fn local_ram_unavailable_is_warning() {
        let report = local_report(
            &probe(false, None, None),
            &choice(Backend::Vulkan),
            Ok(20_u64 * 1024 * 1024 * 1024),
            None,
            true,
            &[device()],
            false,
        );
        let ram = report
            .checks
            .iter()
            .find(|check| check.name == "ram")
            .unwrap();
        assert_eq!(ram.severity, FitSeverity::Warning);
        assert_eq!(
            ram.detail,
            "available memory could not be verified for local/qwen3.5-4b"
        );
    }

    #[test]
    fn local_builder_force_cpu_false_has_no_placement_suffix() {
        let report = local_report(
            &probe(true, Some(6144), None),
            &choice(Backend::Cuda),
            Ok(20_u64 * 1024 * 1024 * 1024),
            Some(LOCAL_MIN_RAM_BYTES),
            true,
            &[device()],
            false,
        );
        let gpu = report
            .checks
            .iter()
            .find(|check| check.name == "gpu")
            .unwrap();
        assert!(!gpu.detail.contains(crate::vulkan::CPU_PLACEMENT_COPY));
    }

    #[test]
    fn local_artifact_key_is_arch_agnostic_at_consumption() {
        // Explicit inputs prevent copying a producer-side current-host pin.
        assert_eq!(
            local_artifact_key("linux", "x86_64"),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            local_artifact_key("linux", "arm64"),
            "aarch64-unknown-linux-gnu"
        );
    }
}
