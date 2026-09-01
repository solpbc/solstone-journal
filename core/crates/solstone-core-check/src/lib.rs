// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure readiness verdict shared by the native `solstone-core check` adapter and tests.

use std::fs;
use std::path::Path;
use std::process::Command;

use nix::sys::statvfs::statvfs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_assets::canonical_host_pair;
use solstone_core_local::install::capability_status::CapabilityStatus;
use solstone_core_local::install::ced_readiness::{
    CED_READY_DETAIL, CED_UNAVAILABLE_GUIDANCE, CedVerdict, evaluate_ced_readiness,
};
use solstone_core_local::install::rfdetr_readiness::{
    RFDETR_READY_DETAIL, RFDETR_UNAVAILABLE_GUIDANCE, RfdetrDegradedCause, RfdetrReadiness,
    evaluate_rfdetr_readiness,
};
use solstone_core_local::{
    VulkanDevice, cpu_placement_suffix, discrete_hardware_gpu_count, is_discrete, select_device,
};
use solstone_core_system::provider_runtime::decide_parakeet_auto_placement;

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
    #[serde(default)]
    pub ced: Option<CapabilityStatus>,
    #[serde(default)]
    pub rfdetr: RfdetrCheckInput,
}
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RfdetrCheckInput {
    #[default]
    Omit,
    Ready,
    Degraded {
        cause: RfdetrDegradedCause,
    },
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

fn command_text(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}
fn host_platform() -> PlatformInput {
    let raw_os = std::env::consts::OS;
    let os = match raw_os {
        "macos" => "Darwin",
        "linux" => "Linux",
        other => other,
    };
    let arch = if raw_os == "macos" && std::env::consts::ARCH == "aarch64" {
        "arm64"
    } else {
        std::env::consts::ARCH
    };
    let os_version = if raw_os == "macos" {
        command_text("/usr/bin/sw_vers", &["-productVersion"])
            .or_else(|| command_text("sw_vers", &["-productVersion"]))
    } else {
        command_text("uname", &["-r"])
    }
    .unwrap_or_default();
    PlatformInput {
        os: os.into(),
        os_version,
        arch: arch.into(),
    }
}
#[cfg(target_os = "linux")]
fn meminfo() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        let (key, value) = line.split_once(':')?;
        let kib = value.split_whitespace().next()?.parse::<u64>().ok()?;
        match key {
            "MemTotal" => total = Some(kib * 1024),
            "MemAvailable" => available = Some(kib * 1024),
            _ => {}
        }
    }
    Some((total?, available?))
}
#[cfg(target_os = "linux")]
fn memory() -> MemoryInput {
    if let Some((total, available)) = meminfo() {
        return MemoryInput {
            total_bytes: (total > 0).then_some(total),
            available_bytes: (total > 0 && available > 0 && available <= total)
                .then_some(available),
        };
    }
    MemoryInput {
        total_bytes: None,
        available_bytes: None,
    }
}
#[cfg(target_os = "macos")]
fn memory() -> MemoryInput {
    let total =
        command_text("/usr/sbin/sysctl", &["-n", "hw.memsize"]).and_then(|text| text.parse().ok());
    let available = command_text("/usr/bin/vm_stat", &[]).and_then(|text| {
        let page = text
            .split("page size of ")
            .nth(1)?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?;
        let mut count = 0;
        for line in text.lines() {
            if line.starts_with("Pages free") || line.starts_with("Pages inactive") {
                count += line
                    .split(':')
                    .nth(1)?
                    .trim()
                    .trim_end_matches('.')
                    .parse::<u64>()
                    .ok()?;
            }
        }
        Some(count * page)
    });
    MemoryInput {
        total_bytes: total,
        available_bytes: available,
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn memory() -> MemoryInput {
    MemoryInput {
        total_bytes: None,
        available_bytes: None,
    }
}
fn free_disk(path: &Path) -> Result<u64, String> {
    let mut root = path.to_path_buf();
    while !root.exists() && root != root.parent().unwrap_or(&root) {
        root = root.parent().unwrap_or(&root).to_path_buf();
    }
    let stat = statvfs(&root).map_err(|error| error.to_string())?;
    (stat.blocks_available() as u64)
        .checked_mul(stat.fragment_size() as u64)
        .ok_or_else(|| "free space overflow".into())
}
fn render_nodes_present_but_inaccessible(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    let nodes = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("renderD"))
        })
        .collect::<Vec<_>>();
    !nodes.is_empty()
        && nodes.iter().all(|path| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .is_err()
        })
}

/// Gather the same real host state used by `journal check` for all native consumers.
#[must_use]
pub fn gather_host_inputs(journal: &Path, version: &str) -> CheckInputs {
    let nvidia = solstone_core_local::probe_nvidia_gpu();
    let (tiering_memory_mib, memory_source) = if let Some(vram) = nvidia.vram_mib {
        (Some(vram), "nvidia_vram")
    } else if let Some(unified) = nvidia.unified_memory_mib {
        (Some(unified), "system_available")
    } else {
        (None, "unavailable")
    };
    CheckInputs {
        platform: host_platform(),
        memory: memory(),
        disk: free_disk(journal)
            .map(|free_bytes| DiskInput::Ok { free_bytes })
            .unwrap_or_else(|message| DiskInput::Error { message }),
        journal_path: journal.display().to_string(),
        nvidia: NvidiaInput {
            detected: nvidia.detected,
            vram_mib: nvidia.vram_mib,
            tiering_memory_mib,
            memory_source: memory_source.into(),
        },
        vulkan: VulkanInput {
            probe_ok: solstone_core_local::gpu_probe_ok(),
            devices: solstone_core_local::detect_gpus(),
        },
        render_nodes_present_but_inaccessible: render_nodes_present_but_inaccessible(Path::new(
            "/dev/dri",
        )),
        gpu_evaluation_error: None,
        version: version.into(),
        ced: {
            let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
            ced_input_from(evaluate_ced_readiness(journal, os, arch))
        },
        rfdetr: {
            let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
            rfdetr_input_from(evaluate_rfdetr_readiness(journal, os, arch))
        },
    }
}
fn ced_input_from(verdict: CedVerdict) -> Option<CapabilityStatus> {
    match verdict {
        CedVerdict::Ready { .. } => Some(CapabilityStatus::Ready),
        CedVerdict::Degraded(status) => Some(status),
        CedVerdict::Unsupported { .. } => None,
    }
}
fn rfdetr_input_from(readiness: RfdetrReadiness) -> RfdetrCheckInput {
    match readiness {
        RfdetrReadiness::Ready { .. } => RfdetrCheckInput::Ready,
        RfdetrReadiness::Degraded { cause, .. } => RfdetrCheckInput::Degraded { cause },
        RfdetrReadiness::Unsupported { .. } => RfdetrCheckInput::Degraded {
            cause: RfdetrDegradedCause::Absent,
        },
    }
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
    let rounded = ((bytes as f64 / GIB as f64) * 10.0).round_ties_even() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}")
    }
}
fn placement_suffix(
    devices: &[VulkanDevice],
    selected: Option<&VulkanDevice>,
    vram_mib: Option<u64>,
    unified_memory: bool,
) -> String {
    let decision = decide_parakeet_auto_placement(
        vram_mib.and_then(|value| value.try_into().ok()),
        selected.is_some_and(is_discrete),
        discrete_hardware_gpu_count(devices),
        unified_memory,
        // `sol check` runs before install and cannot rely on journal config.
        true,
    );
    cpu_placement_suffix(selected, decision.force_cpu)
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
        if let Some(ced) = ced_check(inputs) {
            checks.push(ced);
        }
        if let Some(rfdetr) = rfdetr_check(inputs) {
            checks.push(rfdetr);
        }
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
    if let Some(ced) = ced_check(inputs) {
        checks.push(ced);
    }
    if let Some(rfdetr) = rfdetr_check(inputs) {
        checks.push(rfdetr);
    }
    CheckReport {
        platform,
        overall: overall(&checks),
        checks,
        recommended_package: Some("solstone-journal"),
        version: inputs.version.clone(),
    }
}
fn ced_check(inputs: &CheckInputs) -> Option<Check> {
    match &inputs.ced {
        None => None,
        Some(CapabilityStatus::Ready) => {
            Some(check("ced", Severity::Ok, CED_READY_DETAIL, None, None))
        }
        Some(_) => Some(check(
            "ced",
            Severity::Warning,
            CED_UNAVAILABLE_GUIDANCE,
            None,
            None,
        )),
    }
}
fn rfdetr_check(inputs: &CheckInputs) -> Option<Check> {
    match &inputs.rfdetr {
        RfdetrCheckInput::Omit => None,
        RfdetrCheckInput::Ready => Some(check(
            "rfdetr",
            Severity::Ok,
            RFDETR_READY_DETAIL,
            None,
            None,
        )),
        RfdetrCheckInput::Degraded { .. } => Some(check(
            "rfdetr",
            Severity::Blocked,
            RFDETR_UNAVAILABLE_GUIDANCE,
            None,
            None,
        )),
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
    let selected = inputs
        .vulkan
        .probe_ok
        .then(|| select_device(&inputs.vulkan.devices, None))
        .flatten();
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
            format!(
                "Vulkan GPU {} with {} GB{}",
                selected.name,
                label(bytes),
                placement_suffix(
                    &inputs.vulkan.devices,
                    Some(&selected),
                    Some(selected.vram_mib),
                    false
                )
            ),
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
    detail.push_str(&placement_suffix(
        &inputs.vulkan.devices,
        selected.as_ref(),
        Some(mib),
        unified,
    ));
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
    match report.overall {
        Severity::Ok => {
            let _ = report.recommended_package;
            lines.push("Ready — install the journal next:  see INSTALL.md".to_owned());
        }
        Severity::Warning => {
            let _ = report.recommended_package;
            lines.push(
                "Mostly ready (see the warnings above) — you can install the journal:  see INSTALL.md"
                    .to_owned(),
            );
        }
        Severity::Blocked => {
            lines.push("Not ready — this computer can't run the bundled local models yet.".into())
        }
        Severity::Unknown => unreachable!(),
    };
    lines.push(String::new());
    lines.push(format!(
        "Think this readout is wrong for your machine? We'd love a patch — {FEEDBACK_URL}"
    ));
    format!("{}\n", lines.join("\n"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_local::install::ced_readiness::CED_CAPABILITY;

    fn check_inputs(ced: Option<CapabilityStatus>, rfdetr: RfdetrCheckInput) -> CheckInputs {
        CheckInputs {
            platform: PlatformInput {
                os: "Linux".into(),
                os_version: "x".into(),
                arch: "x86_64".into(),
            },
            memory: MemoryInput {
                total_bytes: Some(16 * GIB),
                available_bytes: Some(16 * GIB),
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
            ced,
            rfdetr,
        }
    }

    #[test]
    fn ready_rfdetr_is_ok_and_does_not_change_overall() {
        let inputs = check_inputs(None, RfdetrCheckInput::Ready);
        let rfdetr = rfdetr_check(&inputs).expect("ready RF-DETR check");
        assert_eq!(rfdetr.severity, Severity::Ok);
        assert_eq!(overall(&[rfdetr]), Severity::Ok);
    }

    #[test]
    fn degraded_rfdetr_blocks_for_every_cause() {
        for cause in [
            RfdetrDegradedCause::Absent,
            RfdetrDegradedCause::IntegrityInvalid,
            RfdetrDegradedCause::Unrunnable,
        ] {
            let inputs = check_inputs(None, RfdetrCheckInput::Degraded { cause });
            let rfdetr = rfdetr_check(&inputs).expect("degraded RF-DETR check");
            assert_eq!(rfdetr.severity, Severity::Blocked, "{cause:?}");
            assert_eq!(overall(&[rfdetr]), Severity::Blocked, "{cause:?}");
        }
    }

    #[test]
    fn ced_warning_stays_independent_from_ready_rfdetr() {
        let inputs = check_inputs(
            Some(CapabilityStatus::Absent {
                capability: CED_CAPABILITY.to_owned(),
                detail: "sidecar missing".to_owned(),
            }),
            RfdetrCheckInput::Ready,
        );
        assert_eq!(
            overall(&[
                ced_check(&inputs).unwrap(),
                rfdetr_check(&inputs).expect("ready RF-DETR check"),
            ]),
            Severity::Warning
        );
    }

    #[test]
    fn nvidia_linux_still_recommends_the_cpu_journal_package() {
        let inputs = CheckInputs {
            platform: PlatformInput {
                os: "Linux".into(),
                os_version: "x".into(),
                arch: "x86_64".into(),
            },
            memory: MemoryInput {
                total_bytes: Some(16 * GIB),
                available_bytes: Some(16 * GIB),
            },
            disk: DiskInput::Ok {
                free_bytes: 20 * GIB,
            },
            journal_path: "/journal".into(),
            nvidia: NvidiaInput {
                detected: true,
                vram_mib: Some(8192),
                tiering_memory_mib: Some(8192),
                memory_source: "nvidia_vram".into(),
            },
            vulkan: VulkanInput {
                probe_ok: true,
                devices: vec![],
            },
            render_nodes_present_but_inaccessible: false,
            gpu_evaluation_error: None,
            version: "x".into(),
            ced: None,
            rfdetr: RfdetrCheckInput::Ready,
        };
        let report = build_check_report(&inputs);
        assert_eq!(report.recommended_package, Some("solstone-journal"));
        assert_ne!(report.recommended_package, Some("solstone-journal-cuda"));
    }

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
            ced: None,
            rfdetr: RfdetrCheckInput::Ready,
        };
        assert!(json_output(&build_check_report(&inputs)).contains("\"python\": null"));
    }

    #[test]
    fn labels_match_python_rounding_and_g_formatting() {
        assert_eq!(label(24 * GIB), "24");
        assert_eq!(label(11 * GIB / 2), "5.5");
        assert_eq!(label(5 * GIB / 4), "1.2");
        assert_eq!(label(7 * GIB / 4), "1.8");
    }

    #[test]
    fn vulkan_gpu_uses_shared_cpu_placement_decision() {
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
                devices: vec![VulkanDevice {
                    index: 0,
                    name: "Test GPU".into(),
                    device_type: Some(2),
                    vram_mib: 6144,
                }],
            },
            render_nodes_present_but_inaccessible: false,
            gpu_evaluation_error: None,
            version: "x".into(),
            ced: None,
            rfdetr: RfdetrCheckInput::Ready,
        };
        let report = build_check_report(&inputs);
        assert_eq!(
            report.checks[1].detail,
            "Vulkan GPU Test GPU with 6 GB; a model runs on your GPU; transcription runs on your CPU on this machine"
        );
    }
}
