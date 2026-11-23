// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host fit checks for provider artifact installation.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use nix::sys::statvfs::statvfs;
use solstone_core_assets::resolve;
use solstone_core_cli_copy::ENGLISH;
use solstone_core_installed_payload::WindowsArchitecture;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

use super::{
    pins,
    rfdetr_install::{rfdetr_artifact_key, rfdetr_uses_package_payload},
};
use crate::vulkan::select_device;
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
    fn label(self, locale: &str) -> String {
        let id = match self {
            Self::Ok => "journal.cli.install.fit.severity.ok",
            Self::Warning => "journal.cli.install.fit.severity.warning",
            Self::Blocked => "journal.cli.install.fit.severity.blocked",
            Self::Unknown => "journal.cli.install.fit.severity.unknown",
        };
        solstone_core_cli_copy::render_for(locale, id, &[])
    }
}

fn check_name_label(locale: &str, name: &str) -> String {
    match name {
        "platform" => solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.install.fit.check.platform",
            &[],
        ),
        "ram" => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.install.fit.check.ram", &[])
        }
        "disk" => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.install.fit.check.disk", &[])
        }
        "gpu" => {
            solstone_core_cli_copy::render_for(locale, "journal.cli.install.fit.check.gpu", &[])
        }
        other => other.to_owned(),
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
    render_fit_report_in(ENGLISH, report)
}

pub fn render_fit_report_in(locale: &str, report: &FitReport) -> String {
    let overall = report.overall().label(locale);
    let mut lines = vec![solstone_core_cli_copy::display_for(
        locale,
        "journal.cli.install.fit.header",
        [("artifact", &report.artifact), ("overall", &overall)],
    )];
    lines.extend(report.checks.iter().map(|check| {
        let severity = check.severity.label(locale);
        let name = check_name_label(locale, check.name);
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.install.fit.row",
            [
                ("severity", &severity),
                ("name", &name),
                ("detail", &check.detail),
            ],
        )
    }));
    lines.join("\n")
}

pub fn build_parakeet_fit_report(journal: &Path, os_name: &str, arch: &str) -> FitReport {
    build_parakeet_fit_report_in(ENGLISH, journal, os_name, arch)
}

pub fn build_parakeet_fit_report_in(
    locale: &str,
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> FitReport {
    build_parakeet_fit_report_with_free_bytes_in(
        locale,
        journal,
        os_name,
        arch,
        free_bytes_in(locale, &pins::parakeet_cache_root(journal)),
    )
}

pub fn build_parakeet_fit_report_with_free_bytes(
    journal: &Path,
    os_name: &str,
    arch: &str,
    available: Result<u64, String>,
) -> FitReport {
    build_parakeet_fit_report_with_free_bytes_in(ENGLISH, journal, os_name, arch, available)
}

pub fn build_parakeet_fit_report_with_free_bytes_in(
    locale: &str,
    journal: &Path,
    os_name: &str,
    arch: &str,
    available: Result<u64, String>,
) -> FitReport {
    let platform = match pins::parakeet_artifact_key(os_name, arch) {
        Ok(key) => FitCheck {
            name: "platform",
            severity: FitSeverity::Ok,
            detail: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.fit.platform.parakeet.ok",
                [("key", &key)],
            ),
        },
        Err(error) => FitCheck {
            name: "platform",
            severity: FitSeverity::Blocked,
            detail: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.speech.platform",
                [("os_name", &error.os_name), ("arch", &error.arch)],
            ),
        },
    };
    let cache = pins::parakeet_cache_root(journal);
    let (repo, filename, revision, digest, size) = pins::PARAKEET_MODEL;
    let destination = cache
        .join("models")
        .join(repo.replace('/', "__"))
        .join(revision)
        .join(filename);
    let remaining = size.saturating_sub(solstone_core_artifact_download::retained_prefix_bytes(
        &destination,
        digest,
        size,
    ));
    let parakeet_model_label = solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.item.parakeet.model",
        &[],
    );
    let parakeet_cpu_label = solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.item.parakeet.cpu",
        &[],
    );
    let parakeet_vulkan_label = solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.item.parakeet.vulkan",
        &[],
    );
    let requirement_label = solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.requirement.downloads",
        &[],
    );
    let disk = disk_check_in(
        locale,
        &cache,
        available,
        &[(parakeet_model_label.as_str(), remaining)],
        &[parakeet_cpu_label.as_str(), parakeet_vulkan_label.as_str()],
        &requirement_label,
    );
    FitReport {
        artifact: solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.install.fit.artifact.parakeet",
            &[],
        ),
        checks: vec![platform, disk],
    }
}

pub fn build_rfdetr_fit_report(journal: &Path, os_name: &str, arch: &str) -> FitReport {
    build_rfdetr_fit_report_in(ENGLISH, journal, os_name, arch)
}

pub fn build_rfdetr_fit_report_in(
    locale: &str,
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> FitReport {
    build_rfdetr_fit_report_with_free_bytes_in(
        locale,
        journal,
        os_name,
        arch,
        free_bytes_in(locale, &journal.join("cache/providers/rfdetr")),
    )
}

pub fn build_rfdetr_fit_report_with_free_bytes(
    journal: &Path,
    os_name: &str,
    arch: &str,
    available: Result<u64, String>,
) -> FitReport {
    build_rfdetr_fit_report_with_free_bytes_in(ENGLISH, journal, os_name, arch, available)
}

pub fn build_rfdetr_fit_report_with_free_bytes_in(
    locale: &str,
    journal: &Path,
    os_name: &str,
    arch: &str,
    available: Result<u64, String>,
) -> FitReport {
    let artifact =
        solstone_core_cli_copy::render_for(locale, "journal.cli.install.fit.artifact.rfdetr", &[]);
    if rfdetr_uses_package_payload(os_name, arch) {
        return FitReport {
            artifact,
            checks: vec![FitCheck {
                name: "platform",
                severity: FitSeverity::Ok,
                detail: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.platform.rfdetr.windows.ok",
                    [("os", &os_name), ("arch", &arch)],
                ),
            }],
        };
    }
    let platform = match rfdetr_artifact_key(os_name, arch) {
        Some(key) => FitCheck {
            name: "platform",
            severity: FitSeverity::Ok,
            detail: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.fit.platform.rfdetr.ok",
                [("key", &key)],
            ),
        },
        None => FitCheck {
            name: "platform",
            severity: FitSeverity::Blocked,
            detail: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.fit.platform.rfdetr.blocked",
                [("os", &os_name), ("arch", &arch)],
            ),
        },
    };
    let cache = journal.join("cache/providers/rfdetr");
    let model_label = solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.item.rfdetr.model",
        &[],
    );
    let cli_label = solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.item.rfdetr.cli",
        &[],
    );
    let requirement_label = solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.requirement.bundled",
        &[],
    );
    let disk = disk_check_in(
        locale,
        &cache,
        available,
        &[
            (model_label.as_str(), 63_439_488),
            (cli_label.as_str(), 1_048_576),
        ],
        &[],
        &requirement_label,
    );
    FitReport {
        artifact,
        checks: vec![platform, disk],
    }
}

/// Build the local-provider report from facts collected by the owner-facing
/// verb. Windows also verifies the current signed package once for this report.
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
    build_local_fit_report_in(
        ENGLISH,
        journal,
        model_id,
        os_name,
        arch,
        available_disk,
        available_ram,
        nvidia_probe,
        backend_choice,
        vulkan_probe_ok,
        vulkan_devices,
        override_index,
        force_cpu,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_local_fit_report_in(
    locale: &str,
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
    let platform = if crate::embedded::supports_key(&artifact_key) {
        let ready = crate::embedded::identity_for_key(&artifact_key).is_some();
        FitCheck {
            name: "platform",
            severity: if ready {
                FitSeverity::Ok
            } else {
                FitSeverity::Blocked
            },
            detail: solstone_core_cli_copy::render_for(
                locale,
                if ready {
                    "journal.cli.install.fit.platform.embedded.ok"
                } else {
                    "journal.cli.install.fit.platform.embedded.blocked"
                },
                &[],
            ),
        }
    } else if os_name == "darwin" {
        let supported = matches!(arch, "arm64" | "aarch64");
        FitCheck {
            name: "platform",
            severity: if supported {
                FitSeverity::Ok
            } else {
                FitSeverity::Blocked
            },
            detail: if supported {
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.install.fit.platform.mlx.ok",
                    &[],
                )
            } else {
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.platform.mlx.blocked",
                    [("os", &os_name), ("arch", &arch)],
                )
            },
        }
    } else if os_name == "windows" {
        if let Some(windows_arch) = WindowsArchitecture::from_architecture(arch) {
            match super::windows_engine::verified_windows_llama_package_detail()
                .and_then(|pkg| pkg.require_runtime_key_detail(windows_arch.runtime_key()))
            {
                Ok(_) => FitCheck {
                    name: "platform",
                    severity: FitSeverity::Ok,
                    detail: solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.install.fit.platform.windows.llama.ok",
                        [("arch", &arch)],
                    ),
                },
                Err(
                    super::windows_engine::WindowsLlamaPackageError::Missing(detail)
                    | super::windows_engine::WindowsLlamaPackageError::Invalid(detail),
                ) => FitCheck {
                    name: "platform",
                    severity: FitSeverity::Blocked,
                    detail: detail.render(locale),
                },
            }
        } else {
            FitCheck {
                name: "platform",
                severity: FitSeverity::Blocked,
                detail: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.platform.windows.llama.blocked",
                    [("os", &os_name), ("arch", &arch)],
                ),
            }
        }
    } else {
        match pins::vulkan_pin(&artifact_key) {
            Some(_) => FitCheck {
                name: "platform",
                severity: FitSeverity::Ok,
                detail: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.platform.llama.ok",
                    [("key", &artifact_key)],
                ),
            },
            None => FitCheck {
                name: "platform",
                severity: FitSeverity::Blocked,
                detail: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.platform.llama.blocked",
                    [("key", &artifact_key)],
                ),
            },
        }
    };
    let ram = local_ram_check_in(locale, model_id, available_ram);
    let mut known: Vec<(String, u64)> = if os_name == "darwin" {
        let fetch = pins::cache_root(journal)
            .join("downloads/local-model-mlx")
            .join(pins::mlx_model_generation().key());
        vec![(
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.install.fit.disk.item.mlx.model",
                &[],
            ),
            pins::MLX_QWEN_PINS
                .iter()
                .map(|pin| {
                    pin.size
                        .saturating_sub(solstone_core_artifact_download::retained_prefix_bytes(
                            &fetch.join(pin.name),
                            pin.sha256,
                            pin.size,
                        ))
                })
                .sum(),
        )]
    } else {
        local_model_downloads_in(locale, model_id)
    };
    if os_name != "darwin"
        && let (Some(generation), Some(staging)) = (
            pins::model_generation(model_id),
            pins::model_fetch_directory(journal, model_id),
        )
    {
        let installed = pins::model_directory(journal, model_id);
        for (index, name) in ["Qwen3.5-4B-Q4_K_M.gguf", "mmproj-F16.gguf"]
            .iter()
            .enumerate()
        {
            if let Some(member) = generation.files.iter().find(|m| m.file == *name) {
                known[index].1 = pins::remaining_model_member_bytes(
                    journal, model_id, member, &installed, &staging,
                );
            }
        }
    }
    let unknown: Vec<String> = if matches!(os_name, "windows" | "darwin" | "android" | "ios") {
        Vec::new()
    } else if os_name == "linux" && backend_choice.backend == Backend::Cuda {
        match pins::cuda_pin(&artifact_key) {
            Some((_, digest, size)) => {
                let archive = pins::cache_root(journal)
                    .join(".fetch")
                    .join(digest)
                    .join(format!("llama-{digest}.tar.gz"));
                let remaining = size.saturating_sub(
                    solstone_core_artifact_download::retained_prefix_bytes(&archive, digest, size),
                );
                known.push((
                    solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.install.fit.disk.item.cuda.tarball",
                        &[],
                    ),
                    remaining,
                ));
                Vec::new()
            }
            None => vec![solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.install.fit.disk.item.llama.tarball",
                &[],
            )],
        }
    } else {
        vec![solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.install.fit.disk.item.llama.tarball",
            &[],
        )]
    };
    let known_refs: Vec<(&str, u64)> = known
        .iter()
        .map(|(label, size)| (label.as_str(), *size))
        .collect();
    let unknown_refs: Vec<&str> = unknown.iter().map(String::as_str).collect();
    let requirement_label = solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.requirement.downloads",
        &[],
    );
    let disk = disk_check_in(
        locale,
        &pins::cache_root(journal),
        available_disk,
        &known_refs,
        &unknown_refs,
        &requirement_label,
    );
    let platform_ready = platform.severity == FitSeverity::Ok;
    let mut checks = vec![platform, ram, disk];
    if os_name == "linux" || os_name == "windows" {
        let mut gpu = local_gpu_check_in(
            locale,
            os_name,
            nvidia_probe,
            backend_choice,
            vulkan_probe_ok,
            vulkan_devices,
            override_index,
            force_cpu,
        );
        if os_name == "windows" && !platform_ready && gpu.severity == FitSeverity::Ok {
            gpu.severity = FitSeverity::Warning;
        }
        checks.push(gpu);
    }
    FitReport {
        artifact: solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.install.fit.artifact.local",
            &[],
        ),
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

fn local_ram_check_in(locale: &str, model_id: &str, available: Option<u64>) -> FitCheck {
    let verdict = assess_local_memory(available);
    let detail = match available {
        None => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.install.fit.ram.unknown",
            [("model", &model_id)],
        ),
        Some(available) if available >= LOCAL_MIN_RAM_BYTES => {
            let available_gb = gb_label(available);
            let required_gb = gb_label(LOCAL_MIN_RAM_BYTES);
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.fit.ram.ok",
                [
                    ("available", &available_gb),
                    ("required", &required_gb),
                    ("model", &model_id),
                ],
            )
        }
        Some(available) => {
            let required_gb = gb_label(LOCAL_MIN_RAM_BYTES);
            let available_gb = gb_label(available);
            solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.fit.ram.insufficient",
                [
                    ("model", &model_id),
                    ("required", &required_gb),
                    ("available", &available_gb),
                ],
            )
        }
    };
    FitCheck {
        name: "ram",
        severity: verdict.severity,
        detail,
    }
}

#[cfg(test)]
fn local_model_downloads(model_id: &str) -> Vec<(String, u64)> {
    local_model_downloads_in(ENGLISH, model_id)
}

fn local_model_downloads_in(locale: &str, model_id: &str) -> Vec<(String, u64)> {
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
        (
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.install.fit.disk.item.gguf.model",
                &[],
            ),
            gguf.size_bytes,
        ),
        (
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.install.fit.disk.item.mmproj",
                &[],
            ),
            mmproj.size_bytes,
        ),
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

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn local_gpu_check(
    os_name: &str,
    probe: &NvidiaProbe,
    choice: &BackendChoice,
    vulkan_probe_ok: bool,
    devices: &[VulkanDevice],
    override_index: Option<u32>,
    force_cpu: bool,
) -> FitCheck {
    local_gpu_check_in(
        ENGLISH,
        os_name,
        probe,
        choice,
        vulkan_probe_ok,
        devices,
        override_index,
        force_cpu,
    )
}

#[allow(clippy::too_many_arguments)]
fn local_gpu_check_in(
    locale: &str,
    os_name: &str,
    probe: &NvidiaProbe,
    choice: &BackendChoice,
    vulkan_probe_ok: bool,
    devices: &[VulkanDevice],
    override_index: Option<u32>,
    force_cpu: bool,
) -> FitCheck {
    let selected = select_device(devices, override_index);
    let cpu_placement = selected.is_some() && force_cpu;
    let backend = match choice.backend {
        Backend::Cuda => "cuda",
        Backend::Vulkan => "vulkan",
    };
    if choice.backend == Backend::Cuda {
        if !probe.detected {
            return FitCheck {
                name: "gpu",
                severity: FitSeverity::Unknown,
                detail: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.gpu.cuda.probe.unavailable",
                    [("backend", &backend), ("reason", &choice.reason)],
                ),
            };
        }
        if probe.memory_source() == MemorySource::Unavailable {
            return FitCheck {
                name: "gpu",
                severity: FitSeverity::Unknown,
                detail: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.gpu.cuda.memory.unknown",
                    [("backend", &backend), ("reason", &choice.reason)],
                ),
            };
        }
        let unified = probe.memory_source() == MemorySource::SystemAvailable;
        let id = match (unified, cpu_placement) {
            (false, false) => "journal.cli.install.fit.gpu.cuda.ok",
            (true, false) => "journal.cli.install.fit.gpu.cuda.ok.unified",
            (false, true) => "journal.cli.install.fit.gpu.cuda.ok.cpu",
            (true, true) => "journal.cli.install.fit.gpu.cuda.ok.unified.cpu",
        };
        return FitCheck {
            name: "gpu",
            severity: FitSeverity::Ok,
            detail: solstone_core_cli_copy::display_for(locale, id, [("reason", &choice.reason)]),
        };
    }
    if os_name == "windows" {
        return match crate::vulkan::gpu_device_local_verdict(
            vulkan_probe_ok,
            devices,
            override_index,
        ) {
            crate::vulkan::GpuDeviceLocalVerdict::Eligible { .. } => {
                let selected = selected.expect("eligible verdict requires a selected device");
                let id = if cpu_placement {
                    "journal.cli.install.fit.gpu.vulkan.eligible.cpu"
                } else {
                    "journal.cli.install.fit.gpu.vulkan.eligible"
                };
                FitCheck {
                    name: "gpu",
                    severity: FitSeverity::Ok,
                    detail: solstone_core_cli_copy::display_for(
                        locale,
                        id,
                        [
                            ("name", &selected.name),
                            ("backend", &backend),
                            ("reason", &choice.reason),
                        ],
                    ),
                }
            }
            crate::vulkan::GpuDeviceLocalVerdict::BelowBar { available_bytes } => {
                let selected = selected.expect("below-bar verdict requires a selected device");
                let available_gb = gb_label(available_bytes);
                let id = if cpu_placement {
                    "journal.cli.install.fit.gpu.vulkan.below.bar.cpu"
                } else {
                    "journal.cli.install.fit.gpu.vulkan.below.bar"
                };
                FitCheck {
                    name: "gpu",
                    severity: FitSeverity::Blocked,
                    detail: solstone_core_cli_copy::display_for(
                        locale,
                        id,
                        [
                            ("name", &selected.name),
                            ("available", &available_gb),
                            ("backend", &backend),
                            ("reason", &choice.reason),
                        ],
                    ),
                }
            }
            crate::vulkan::GpuDeviceLocalVerdict::Unknown => FitCheck {
                name: "gpu",
                severity: FitSeverity::Unknown,
                detail: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.gpu.vulkan.memory.unknown",
                    [("backend", &backend), ("reason", &choice.reason)],
                ),
            },
            crate::vulkan::GpuDeviceLocalVerdict::NoHardwareDevice => FitCheck {
                name: "gpu",
                severity: FitSeverity::Blocked,
                detail: solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.gpu.vulkan.no.hardware",
                    [("backend", &backend), ("reason", &choice.reason)],
                ),
            },
        };
    }
    if !vulkan_probe_ok {
        return FitCheck {
            name: "gpu",
            severity: FitSeverity::Unknown,
            detail: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.fit.gpu.vulkan.probe.incomplete",
                [("backend", &backend), ("reason", &choice.reason)],
            ),
        };
    }
    match selected {
        None => FitCheck {
            name: "gpu",
            severity: FitSeverity::Warning,
            detail: solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.fit.gpu.vulkan.no.hardware",
                [("backend", &backend), ("reason", &choice.reason)],
            ),
        },
        Some(selected) => {
            match crate::vulkan::gpu_device_local_verdict(vulkan_probe_ok, devices, override_index)
            {
                crate::vulkan::GpuDeviceLocalVerdict::Eligible { .. } => {
                    let id = if cpu_placement {
                        "journal.cli.install.fit.gpu.vulkan.eligible.cpu"
                    } else {
                        "journal.cli.install.fit.gpu.vulkan.eligible"
                    };
                    FitCheck {
                        name: "gpu",
                        severity: FitSeverity::Ok,
                        detail: solstone_core_cli_copy::display_for(
                            locale,
                            id,
                            [
                                ("name", &selected.name),
                                ("backend", &backend),
                                ("reason", &choice.reason),
                            ],
                        ),
                    }
                }
                crate::vulkan::GpuDeviceLocalVerdict::BelowBar { available_bytes } => {
                    let available_gb = gb_label(available_bytes);
                    let id = if cpu_placement {
                        "journal.cli.install.fit.gpu.vulkan.below.bar.cpu"
                    } else {
                        "journal.cli.install.fit.gpu.vulkan.below.bar"
                    };
                    FitCheck {
                        name: "gpu",
                        severity: FitSeverity::Blocked,
                        detail: solstone_core_cli_copy::display_for(
                            locale,
                            id,
                            [
                                ("name", &selected.name),
                                ("available", &available_gb),
                                ("backend", &backend),
                                ("reason", &choice.reason),
                            ],
                        ),
                    }
                }
                crate::vulkan::GpuDeviceLocalVerdict::Unknown => FitCheck {
                    name: "gpu",
                    severity: FitSeverity::Unknown,
                    detail: solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.install.fit.gpu.vulkan.memory.unknown",
                        [("backend", &backend), ("reason", &choice.reason)],
                    ),
                },
                crate::vulkan::GpuDeviceLocalVerdict::NoHardwareDevice => FitCheck {
                    name: "gpu",
                    severity: FitSeverity::Blocked,
                    detail: solstone_core_cli_copy::display_for(
                        locale,
                        "journal.cli.install.fit.gpu.vulkan.no.hardware",
                        [("backend", &backend), ("reason", &choice.reason)],
                    ),
                },
            }
        }
    }
}

fn disk_check_in(
    locale: &str,
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
    let unknown_names = unknown.join(", ");
    match available {
        Err(error) => FitCheck {
            name: "disk",
            severity: FitSeverity::Unknown,
            detail: if unknown.is_empty() {
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.disk.unavailable",
                    [("path", &cache.display()), ("error", &error)],
                )
            } else {
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.disk.unavailable.unknown",
                    [
                        ("path", &cache.display()),
                        ("error", &error),
                        ("unknown", &unknown_names),
                    ],
                )
            },
        },
        Ok(available) if available < required => {
            let required_gb = gb_label(required);
            let available_gb = gb_label(available);
            let detail = match (known_names.is_empty(), unknown.is_empty()) {
                (true, true) => solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.disk.insufficient",
                    [
                        ("requirement", &requirement),
                        ("required", &required_gb),
                        ("available", &available_gb),
                    ],
                ),
                (false, true) => solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.disk.insufficient.named",
                    [
                        ("requirement", &requirement),
                        ("required", &required_gb),
                        ("available", &available_gb),
                        ("known", &known_names),
                    ],
                ),
                (true, false) => solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.disk.insufficient.unknown",
                    [
                        ("requirement", &requirement),
                        ("required", &required_gb),
                        ("available", &available_gb),
                        ("unknown", &unknown_names),
                    ],
                ),
                (false, false) => solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.disk.insufficient.named.unknown",
                    [
                        ("requirement", &requirement),
                        ("required", &required_gb),
                        ("available", &available_gb),
                        ("known", &known_names),
                        ("unknown", &unknown_names),
                    ],
                ),
            };
            FitCheck {
                name: "disk",
                severity: FitSeverity::Blocked,
                detail,
            }
        }
        Ok(available) => {
            let available_gb = gb_label(available);
            let required_gb = gb_label(required);
            let detail = if unknown.is_empty() {
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.disk.ok",
                    [
                        ("available", &available_gb),
                        ("required", &required_gb),
                        ("requirement", &requirement),
                    ],
                )
            } else {
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.fit.disk.ok.unknown",
                    [
                        ("available", &available_gb),
                        ("requirement", &requirement),
                        ("required", &required_gb),
                        ("unknown", &unknown_names),
                    ],
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
    free_bytes_in(ENGLISH, target)
}

#[cfg(unix)]
pub fn free_bytes_in(locale: &str, target: &Path) -> Result<u64, String> {
    let usage_root = nearest_existing_ancestor(target);
    let stats = statvfs(&usage_root).map_err(|error| error.to_string())?;
    let blocks = stats.blocks_available() as u64;
    let fragment_size = stats.fragment_size() as u64;
    blocks.checked_mul(fragment_size).ok_or_else(|| {
        solstone_core_cli_copy::render_for(locale, "journal.cli.install.fit.disk.overflow", &[])
    })
}

#[cfg(windows)]
pub fn free_bytes_in(_locale: &str, target: &Path) -> Result<u64, String> {
    let usage_root = nearest_existing_ancestor(target);
    let mut path = usage_root.as_os_str().encode_wide().collect::<Vec<_>>();
    path.push(0);
    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut free = 0_u64;
    // SAFETY: `path` is nul-terminated and live for the synchronous call; the
    // three u64 values are writable Windows ULARGE_INTEGER-compatible output.
    #[allow(unsafe_code)]
    let result =
        unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) };
    if result == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(available)
}

#[cfg(not(any(unix, windows)))]
pub fn free_bytes_in(locale: &str, _target: &Path) -> Result<u64, String> {
    Err(solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.fit.disk.unsupported",
        &[],
    ))
}

#[cfg(windows)]
#[link(name = "kernel32")]
#[allow(unsafe_code)]
unsafe extern "system" {
    fn GetDiskFreeSpaceExW(
        directory_name: *const u16,
        available_to_caller: *mut u64,
        total_bytes: *mut u64,
        total_free_bytes: *mut u64,
    ) -> i32;
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
                "available disk space could not be verified at {}: disk unavailable; {unknown}",
                pins::cache_root(Path::new("/journal")).display(),
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

    #[test]
    fn rfdetr_fit_report_windows_architectures_are_ok_without_cache_bytes() {
        let journal = tempfile::tempdir().unwrap();
        for architecture in ["x86_64", "arm64", "aarch64"] {
            let report = build_rfdetr_fit_report_with_free_bytes(
                journal.path(),
                "windows",
                architecture,
                Err("disk query failed".to_owned()),
            );
            assert_eq!(report.overall(), FitSeverity::Ok);
            assert_eq!(report.checks.len(), 1);
            assert_eq!(report.checks[0].name, "platform");
            assert_eq!(report.checks[0].severity, FitSeverity::Ok);
        }
    }

    #[test]
    fn rfdetr_fit_report_unsupported_platform_blocked() {
        let journal = tempfile::tempdir().unwrap();
        let report = build_rfdetr_fit_report_with_free_bytes(
            journal.path(),
            "linux",
            "mips",
            Ok(100 * 1024 * 1024 * 1024),
        );
        assert_eq!(report.overall(), FitSeverity::Blocked);
        let platform = report.checks.iter().find(|c| c.name == "platform").unwrap();
        assert_eq!(platform.severity, FitSeverity::Blocked);
    }

    #[test]
    fn windows_gpu_fit_verdicts() {
        let probe = probe(false, None, None);
        let choice = choice(Backend::Vulkan);

        // 1. BelowBar (4096 MiB) -> Blocked
        let dev_below = vec![VulkanDevice {
            index: 0,
            name: "RTX 3050".into(),
            device_type: Some(2),
            vram_mib: 4096,
        }];
        let check_below =
            local_gpu_check("windows", &probe, &choice, true, &dev_below, None, false);
        assert_eq!(check_below.severity, FitSeverity::Blocked);

        // 2. Eligible (6144 MiB) -> Ok
        let dev_eligible = vec![VulkanDevice {
            index: 0,
            name: "RTX 4060".into(),
            device_type: Some(2),
            vram_mib: 6144,
        }];
        let check_eligible =
            local_gpu_check("windows", &probe, &choice, true, &dev_eligible, None, false);
        assert_eq!(check_eligible.severity, FitSeverity::Ok);

        // 3. No hardware device -> Blocked
        let dev_none = vec![];
        let check_none = local_gpu_check("windows", &probe, &choice, true, &dev_none, None, false);
        assert_eq!(check_none.severity, FitSeverity::Blocked);

        // 4. Probe failure -> Unknown
        let check_unknown = local_gpu_check(
            "windows",
            &probe,
            &choice,
            false,
            &dev_eligible,
            None,
            false,
        );
        assert_eq!(check_unknown.severity, FitSeverity::Unknown);
    }

    #[test]
    fn linux_gpu_fit_verdicts() {
        let probe = probe(false, None, None);
        let choice = choice(Backend::Vulkan);

        // 1. BelowBar (4096 MiB) -> Blocked
        let dev_below = vec![VulkanDevice {
            index: 0,
            name: "RTX 3050".into(),
            device_type: Some(2),
            vram_mib: 4096,
        }];
        let check_below = local_gpu_check("linux", &probe, &choice, true, &dev_below, None, false);
        assert_eq!(check_below.severity, FitSeverity::Blocked);

        // 2. Eligible (6144 MiB) -> Ok
        let dev_eligible = vec![VulkanDevice {
            index: 0,
            name: "RTX 4060".into(),
            device_type: Some(2),
            vram_mib: 6144,
        }];
        let check_eligible =
            local_gpu_check("linux", &probe, &choice, true, &dev_eligible, None, false);
        assert_eq!(check_eligible.severity, FitSeverity::Ok);

        // 3. No hardware device -> Warning
        let dev_none = vec![];
        let check_none = local_gpu_check("linux", &probe, &choice, true, &dev_none, None, false);
        assert_eq!(check_none.severity, FitSeverity::Warning);

        // 4. Probe failure -> Unknown
        let check_unknown =
            local_gpu_check("linux", &probe, &choice, false, &dev_eligible, None, false);
        assert_eq!(check_unknown.severity, FitSeverity::Unknown);

        // 5. Unequal pair with override index 1:
        // Case A: dev0 8192, dev1 4096 -> Override Some(1) selects small device -> Blocked
        let dev_unequal_a = vec![
            VulkanDevice {
                index: 0,
                name: "dev0-large".into(),
                device_type: Some(2),
                vram_mib: 8192,
            },
            VulkanDevice {
                index: 1,
                name: "dev1-small".into(),
                device_type: Some(2),
                vram_mib: 4096,
            },
        ];
        let check_override_a = local_gpu_check(
            "linux",
            &probe,
            &choice,
            true,
            &dev_unequal_a,
            Some(1),
            false,
        );
        assert_eq!(check_override_a.severity, FitSeverity::Blocked);
        assert!(check_override_a.detail.contains("dev1-small"));

        // Case B: dev0 4096, dev1 8192 -> Override Some(1) selects large device -> Ok
        let dev_unequal_b = vec![
            VulkanDevice {
                index: 0,
                name: "dev0-small".into(),
                device_type: Some(2),
                vram_mib: 4096,
            },
            VulkanDevice {
                index: 1,
                name: "dev1-large".into(),
                device_type: Some(2),
                vram_mib: 8192,
            },
        ];
        let check_override_b = local_gpu_check(
            "linux",
            &probe,
            &choice,
            true,
            &dev_unequal_b,
            Some(1),
            false,
        );
        assert_eq!(check_override_b.severity, FitSeverity::Ok);
        assert!(check_override_b.detail.contains("dev1-large"));

        // 6. 0-byte device local memory -> Unknown
        let dev_zero = vec![VulkanDevice {
            index: 0,
            name: "zero".into(),
            device_type: Some(2),
            vram_mib: 0,
        }];
        let check_zero = local_gpu_check("linux", &probe, &choice, true, &dev_zero, None, false);
        assert_eq!(check_zero.severity, FitSeverity::Unknown);
    }

    #[test]
    fn windows_gpu_fit_downgrades_ok_when_package_not_ready() {
        let journal = tempfile::tempdir().unwrap();
        let probe = probe(false, None, None);
        let choice = choice(Backend::Vulkan);

        super::super::windows_engine::set_test_windows_llama_package(None);

        let dev_eligible = vec![VulkanDevice {
            index: 0,
            name: "RTX 4060".into(),
            device_type: Some(2),
            vram_mib: 6144,
        }];
        let report_eligible = build_local_fit_report(
            journal.path(),
            "local/qwen3.5-4b",
            "windows",
            "x86_64",
            Ok(u64::MAX),
            Some(32 * 1024 * 1024 * 1024),
            &probe,
            &choice,
            true,
            &dev_eligible,
            None,
            false,
        );
        let gpu_eligible = report_eligible
            .checks
            .iter()
            .find(|c| c.name == "gpu")
            .unwrap();
        assert_eq!(gpu_eligible.severity, FitSeverity::Warning);

        let dev_below = vec![VulkanDevice {
            index: 0,
            name: "RTX 3050".into(),
            device_type: Some(2),
            vram_mib: 4096,
        }];
        let report_below = build_local_fit_report(
            journal.path(),
            "local/qwen3.5-4b",
            "windows",
            "x86_64",
            Ok(u64::MAX),
            Some(32 * 1024 * 1024 * 1024),
            &probe,
            &choice,
            true,
            &dev_below,
            None,
            false,
        );
        let gpu_below = report_below
            .checks
            .iter()
            .find(|c| c.name == "gpu")
            .unwrap();
        assert_eq!(gpu_below.severity, FitSeverity::Blocked);
    }
}
