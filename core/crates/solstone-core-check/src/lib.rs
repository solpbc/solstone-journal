// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure readiness verdict shared by the native `solstone-core check` adapter and tests.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const GIB: u64 = 1024 * 1024 * 1024;
const GPU_MIN: u64 = 6 * GIB;
const DISK_MIN: u64 = 20 * GIB;
const MAC_MEMORY_MIN: u64 = 16 * GIB;
const MAC_AVAILABLE_MIN: u64 = 13 * GIB;
const LINUX_RAM_WARN: u64 = 8 * GIB;
const FEEDBACK_URL: &str = "https://github.com/solpbc/solstone-journal";
const RENDER_HINT: &str = "a GPU render node exists under /dev/dri but this user cannot open it — add yourself to the render group with `sudo usermod -aG render $USER`, then log out and back in and run `sol check` again";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckInputs {
    pub platform: PlatformInput,
    pub memory: MemoryInput,
    pub disk: DiskInput,
    pub journal_path: String,
    pub nvidia: NvidiaInput,
    pub vulkan: VulkanInput,
    pub render_nodes_present_but_inaccessible: bool,
    pub gpu_evaluation_error: Option<String>,
    pub version: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlatformInput {
    pub os: String,
    pub os_version: String,
    pub arch: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryInput {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiskInput {
    Ok { free_bytes: u64 },
    Error { message: String },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NvidiaInput {
    pub detected: bool,
    pub vram_mib: Option<u64>,
    pub tiering_memory_mib: Option<u64>,
    pub memory_source: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VulkanInput {
    pub probe_ok: bool,
    pub devices: Vec<VulkanDevice>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VulkanDevice {
    pub index: u32,
    pub name: String,
    pub device_type: i32,
    pub vram_mib: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub platform: PlatformReport,
    pub checks: Vec<Check>,
    pub overall: Severity,
    pub recommended_package: Option<&'static str>,
    pub version: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct PlatformReport {
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub python: Option<String>,
    pub supported: bool,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Warning,
    Blocked,
    Unknown,
}
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub severity: Severity,
    pub detail: String,
    pub required_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}
fn check(
    name: &'static str,
    severity: Severity,
    detail: impl Into<String>,
    required_bytes: Option<u64>,
    available_bytes: Option<u64>,
) -> Check {
    Check {
        name,
        severity,
        detail: detail.into(),
        required_bytes,
        available_bytes,
    }
}
fn label(bytes: u64) -> String {
    let tenths = (bytes.saturating_mul(10) + GIB / 2) / GIB;
    if tenths.is_multiple_of(10) {
        (tenths / 10).to_string()
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    }
}
fn supported(platform: &PlatformInput) -> bool {
    (platform.os == "Darwin" && platform.arch == "arm64")
        || (platform.os == "Linux" && matches!(platform.arch.as_str(), "x86_64" | "aarch64"))
}
fn overall(checks: &[Check]) -> Severity {
    if checks.iter().any(|item| item.severity == Severity::Blocked) {
        Severity::Blocked
    } else if checks
        .iter()
        .any(|item| matches!(item.severity, Severity::Warning | Severity::Unknown))
    {
        Severity::Warning
    } else {
        Severity::Ok
    }
}

pub fn build_check_report(inputs: &CheckInputs) -> CheckReport {
    let supported = supported(&inputs.platform);
    let platform = PlatformReport {
        os: inputs.platform.os.clone(),
        os_version: inputs.platform.os_version.clone(),
        arch: inputs.platform.arch.clone(),
        python: None,
        supported,
    };
    if !supported {
        let detail = if inputs.platform.os == "Windows" {
            "Windows isn't supported for the bundled local models yet — run the journal on Linux or an Apple Silicon Mac.".into()
        } else if inputs.platform.os == "Darwin" {
            "Intel Macs aren't supported — use an Apple Silicon Mac, or run the journal on supported Linux.".into()
        } else {
            format!(
                "{}/{} can't run the bundled local models yet — only Apple Silicon macOS and x86_64/aarch64 Linux are supported.",
                inputs.platform.os, inputs.platform.arch
            )
        };
        let checks = vec![check("platform", Severity::Blocked, detail, None, None)];
        return CheckReport {
            platform,
            overall: Severity::Blocked,
            checks,
            recommended_package: None,
            version: inputs.version.clone(),
        };
    }
    let platform_detail = if inputs.platform.os == "Darwin" {
        "Apple Silicon macOS (arm64)".into()
    } else {
        format!("Linux ({})", inputs.platform.arch)
    };
    let mut checks = vec![check("platform", Severity::Ok, platform_detail, None, None)];
    if inputs.platform.os == "Darwin" {
        checks.push(mac_memory(&inputs.memory));
        checks.push(disk(inputs));
        return CheckReport {
            platform,
            overall: overall(&checks),
            checks,
            recommended_package: Some("solstone-journal"),
            version: inputs.version.clone(),
        };
    }
    checks.push(gpu(inputs));
    checks.push(ram(&inputs.memory));
    checks.push(disk(inputs));
    let package = if inputs.platform.arch == "x86_64" && inputs.nvidia.detected {
        "solstone-journal-cuda"
    } else {
        "solstone-journal"
    };
    CheckReport {
        platform,
        overall: overall(&checks),
        checks,
        recommended_package: Some(package),
        version: inputs.version.clone(),
    }
}
fn mac_memory(memory: &MemoryInput) -> Check {
    match memory.total_bytes {
        None => check(
            "memory",
            Severity::Unknown,
            "available memory could not be verified",
            Some(MAC_MEMORY_MIN),
            None,
        ),
        Some(total) if total < MAC_MEMORY_MIN => check(
            "memory",
            Severity::Blocked,
            format!(
                "Apple Silicon needs at least 16 GB of memory for the bundled local models (this Mac has {} GB)",
                label(total)
            ),
            Some(MAC_MEMORY_MIN),
            Some(total),
        ),
        Some(_total)
            if memory
                .available_bytes
                .is_some_and(|available| available < MAC_AVAILABLE_MIN) =>
        {
            let available = memory.available_bytes.unwrap();
            check(
                "memory",
                Severity::Warning,
                format!(
                    "enough total memory, but only {} GB is free right now — close some apps before the local models load",
                    label(available)
                ),
                Some(MAC_AVAILABLE_MIN),
                Some(available),
            )
        }
        Some(total) => check(
            "memory",
            Severity::Ok,
            format!("{} GB of memory meets the 16 GB minimum", label(total)),
            Some(MAC_MEMORY_MIN),
            Some(total),
        ),
    }
}
fn gpu(inputs: &CheckInputs) -> Check {
    if let Some(error) = &inputs.gpu_evaluation_error {
        return check(
            "gpu",
            Severity::Unknown,
            format!("GPU readiness could not be determined: {error}"),
            None,
            None,
        );
    }
    let selected = if inputs.vulkan.probe_ok {
        inputs
            .vulkan
            .devices
            .iter()
            .filter(|device| matches!(device.device_type, 1 | 2))
            .min_by_key(|device| (if device.device_type == 2 { 0 } else { 1 }, device.index))
    } else {
        None
    };
    if !inputs.nvidia.detected {
        if (!inputs.vulkan.probe_ok || selected.is_none())
            && inputs.render_nodes_present_but_inaccessible
        {
            return check("gpu", Severity::Unknown, RENDER_HINT, None, None);
        }
        if !inputs.vulkan.probe_ok {
            return check(
                "gpu",
                Severity::Unknown,
                "no NVIDIA GPU found and the Vulkan probe did not complete — GPU readiness is unknown",
                None,
                None,
            );
        }
        let Some(selected) = selected else {
            return check(
                "gpu",
                Severity::Blocked,
                "no usable GPU found — the bundled local models need a hardware GPU with at least 6 GB",
                Some(GPU_MIN),
                None,
            );
        };
        let bytes = selected.vram_mib * 1024 * 1024;
        if bytes < GPU_MIN {
            return check(
                "gpu",
                Severity::Blocked,
                format!(
                    "GPU {} has {} GB — the bundled local models need at least 6 GB",
                    selected.name,
                    label(bytes)
                ),
                Some(GPU_MIN),
                Some(bytes),
            );
        }
        return check(
            "gpu",
            Severity::Ok,
            format!("Vulkan GPU {} with {} GB", selected.name, label(bytes)),
            Some(GPU_MIN),
            Some(bytes),
        );
    }
    let effective = inputs.nvidia.vram_mib.or(inputs.nvidia.tiering_memory_mib);
    let Some(mib) = effective else {
        return check(
            "gpu",
            Severity::Unknown,
            "NVIDIA GPU detected but its memory could not be read — GPU readiness is unknown",
            Some(GPU_MIN),
            None,
        );
    };
    let bytes = mib * 1024 * 1024;
    if bytes < GPU_MIN {
        return check(
            "gpu",
            Severity::Blocked,
            format!(
                "the NVIDIA GPU has {} GB — the bundled local models need at least 6 GB",
                label(bytes)
            ),
            Some(GPU_MIN),
            Some(bytes),
        );
    }
    let unified = inputs.nvidia.memory_source == "system_available";
    let mut detail = format!("NVIDIA GPU with {} GB", label(bytes));
    if unified {
        detail.push_str(" (unified memory)");
    }
    if !unified
        && mib < 16000
        && selected.is_some()
        && inputs
            .vulkan
            .devices
            .iter()
            .filter(|device| device.device_type == 2)
            .count()
            == 1
    {
        detail.push_str("; sol thinks on your GPU; transcription runs on your CPU on this machine");
    }
    check("gpu", Severity::Ok, detail, Some(GPU_MIN), Some(bytes))
}
fn ram(memory: &MemoryInput) -> Check {
    match memory.total_bytes {
        None => check(
            "ram",
            Severity::Warning,
            "system memory could not be verified",
            None,
            None,
        ),
        Some(total) if total < LINUX_RAM_WARN => check(
            "ram",
            Severity::Warning,
            format!(
                "{} GB of system memory is on the low side — 8 GB or more is recommended",
                label(total)
            ),
            Some(LINUX_RAM_WARN),
            Some(total),
        ),
        Some(total) => check(
            "ram",
            Severity::Ok,
            format!("{} GB of system memory", label(total)),
            None,
            Some(total),
        ),
    }
}
fn disk(inputs: &CheckInputs) -> Check {
    match &inputs.disk {
        DiskInput::Error { message } => check(
            "disk",
            Severity::Unknown,
            format!(
                "free space at {} could not be verified: {message}",
                inputs.journal_path
            ),
            Some(DISK_MIN),
            None,
        ),
        DiskInput::Ok { free_bytes } if *free_bytes < DISK_MIN => check(
            "disk",
            Severity::Blocked,
            format!(
                "the journal and local models need at least 20 GB free — {} has {} GB",
                inputs.journal_path,
                label(*free_bytes)
            ),
            Some(DISK_MIN),
            Some(*free_bytes),
        ),
        DiskInput::Ok { free_bytes } => check(
            "disk",
            Severity::Ok,
            format!(
                "{} GB free at {} (need 20 GB)",
                label(*free_bytes),
                inputs.journal_path
            ),
            Some(DISK_MIN),
            Some(*free_bytes),
        ),
    }
}
pub fn exit_code(report: &CheckReport) -> u8 {
    match report.overall {
        Severity::Ok => 0,
        Severity::Warning => 1,
        Severity::Blocked => 2,
        Severity::Unknown => unreachable!(),
    }
}
pub fn json_output(report: &CheckReport) -> String {
    let checks = report.checks.iter().map(|item| json!({"name": item.name, "severity": item.severity, "detail": item.detail, "required_bytes": item.required_bytes, "available_bytes": item.available_bytes})).collect::<Vec<_>>();
    let value: Value = json!({"platform":{"os":report.platform.os,"os_version":report.platform.os_version,"arch":report.platform.arch,"python":null,"supported":report.platform.supported},"checks":checks,"overall":report.overall,"feedback_url":FEEDBACK_URL,"version":report.version});
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("JSON value serializes")
    )
}
pub fn human_output(report: &CheckReport) -> String {
    let mut lines = vec![
        "sol check — can this computer run the journal with the bundled local models?".to_owned(),
    ];
    for item in &report.checks {
        let marker = match item.severity {
            Severity::Ok => "[ok]",
            Severity::Warning => "[warn]",
            Severity::Blocked => "[blocked]",
            Severity::Unknown => "[unknown]",
        };
        lines.push(format!("  {marker:<9} {:<9} {}", item.name, item.detail));
    }
    match report.overall { Severity::Ok => lines.push(format!("Ready — install the journal next:  uv tool install {}", report.recommended_package.expect("supported ready report has package"))), Severity::Warning => lines.push(format!("Mostly ready (see the warnings above) — you can install the journal:  uv tool install {}", report.recommended_package.expect("supported warning report has package"))), Severity::Blocked => lines.push("Not ready — this computer can't run the bundled local models yet.".into()), Severity::Unknown => unreachable!() };
    lines.push(String::new());
    lines.push(format!(
        "Think this readout is wrong for your machine? We'd love a patch — {FEEDBACK_URL}"
    ));
    format!("{}\n", lines.join("\n"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_platform_python_is_null() {
        let inputs = CheckInputs {
            platform: PlatformInput {
                os: "Linux".into(),
                os_version: "x".into(),
                arch: "x86_64".into(),
            },
            memory: MemoryInput {
                total_bytes: Some(8 * GIB),
                available_bytes: Some(8 * GIB),
            },
            disk: DiskInput::Ok {
                free_bytes: 20 * GIB,
            },
            journal_path: "/journal".into(),
            nvidia: NvidiaInput {
                detected: false,
                vram_mib: None,
                tiering_memory_mib: None,
                memory_source: "unavailable".into(),
            },
            vulkan: VulkanInput {
                probe_ok: true,
                devices: vec![],
            },
            render_nodes_present_but_inaccessible: false,
            gpu_evaluation_error: None,
            version: "x".into(),
        };
        assert!(json_output(&build_check_report(&inputs)).contains("\"python\": null"));
    }
}
