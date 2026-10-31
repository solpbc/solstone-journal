// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure readiness verdict shared by the native `solstone-core check` adapter and tests.

use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(unix)]
use nix::sys::statvfs::statvfs;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_assets::canonical_host_pair;
use solstone_core_local::install::capability_status::CapabilityStatus;
use solstone_core_local::install::ced_readiness::{CED_READY_DETAIL, CedVerdict};
#[cfg(test)]
use solstone_core_local::install::rfdetr_readiness::RFDETR_UNAVAILABLE_GUIDANCE;
use solstone_core_local::install::rfdetr_readiness::{
    RFDETR_READY_DETAIL, RfdetrDegradedCause, RfdetrReadiness, evaluate_rfdetr_readiness,
};
use solstone_core_local::{
    APPLE_SILICON_TOTAL_MEMORY_MIN_BYTES, AppleTotalMemoryVerdict, GPU_DEVICE_LOCAL_MIN_BYTES,
    GpuDeviceLocalVerdict, VulkanDevice, apple_total_memory_verdict, cpu_placement_suffix,
    gpu_device_local_verdict, select_device,
};
use solstone_core_system::provider_runtime::parakeet_placement_for_devices;

const GIB: u64 = 1024 * 1024 * 1024;
const DISK_MIN: u64 = 20 * GIB;
const MAC_AVAILABLE_MIN: u64 = 13 * GIB;
const LINUX_RAM_WARN: u64 = 8 * GIB;
const FEEDBACK_URL: &str = "https://github.com/solpbc/solstone-journal";
const RENDER_HINT: &str = "a GPU render node exists under /dev/dri but this user cannot open it — add yourself to the render group with `sudo usermod -aG render $USER`, then log out and back in and run `solstone journal check` again";

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulkan_device_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_backend: Option<solstone_core_local::Backend>,
    pub bundled_local_selected_or_planned: bool,
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
    /// The specific reason a `ced`/`rfdetr` check is Degraded (`absent`,
    /// `integrity_invalid`, `unloadable`/`unrunnable`) -- `None` for every
    /// other check and for a non-Degraded ced/rfdetr check. CED detail is
    /// now the status detail; RF-DETR detail stays the fixed owner-facing
    /// sentence (`RFDETR_UNAVAILABLE_GUIDANCE` on Unix; `RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE`
    /// on Windows package platforms) regardless of which cause fired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<&'static str>,
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
    // ⚠ Every arm here must be added in step with `supported()` and the
    // `platform_detail` match below. Windows was missing from this mapping while
    // a `platform.os == "Windows"` arm existed downstream, so that arm was dead
    // code and Windows owners fell through to a message written for platforms
    // the journal does not ship on at all.
    let os = match raw_os {
        "macos" => "Darwin",
        "linux" => "Linux",
        "windows" => "Windows",
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
#[cfg(windows)]
fn memory() -> MemoryInput {
    match solstone_core_system::memory_admission::windows_physical_memory_bytes() {
        Some((total, available)) => MemoryInput {
            total_bytes: Some(total),
            available_bytes: Some(available),
        },
        None => MemoryInput {
            total_bytes: None,
            available_bytes: None,
        },
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn memory() -> MemoryInput {
    MemoryInput {
        total_bytes: None,
        available_bytes: None,
    }
}
#[cfg(unix)]
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

#[cfg(windows)]
fn free_disk(path: &Path) -> Result<u64, String> {
    solstone_core_local::install::fit_report::free_bytes(path)
}

#[cfg(not(any(unix, windows)))]
fn free_disk(_path: &Path) -> Result<u64, String> {
    Err("disk-space inspection is unsupported on this platform".into())
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCheckGpuSelection {
    pub vulkan_device_index: Option<u32>,
    pub local_backend: Option<solstone_core_local::Backend>,
    pub gpu_evaluation_error: Option<String>,
    pub bundled_local_selected_or_planned: bool,
}

#[must_use]
pub fn local_check_gpu_selection(
    journal: &Path,
    nvidia: &solstone_core_local::NvidiaProbe,
) -> LocalCheckGpuSelection {
    let read = match solstone_core_journal_config::read_journal_config(journal) {
        Ok(read) => read,
        Err(error) => {
            return LocalCheckGpuSelection {
                vulkan_device_index: None,
                local_backend: None,
                gpu_evaluation_error: Some(error.to_string()),
                bundled_local_selected_or_planned: false,
            };
        }
    };
    let override_index = read
        .config
        .as_ref()
        .and_then(|c| c.get("providers"))
        .and_then(Value::as_object)
        .and_then(|p| p.get("local"))
        .and_then(Value::as_object)
        .and_then(|l| l.get("vulkan_device_index"))
        .and_then(solstone_core_local::vulkan::local_vulkan_device_index);
    let bundled_local_selected_or_planned = if read.present {
        read.config
            .as_ref()
            .is_some_and(solstone_core_local::bundled_local_model_holds_gpu)
    } else {
        true
    };

    match solstone_core_local::install::local_backend_choice(journal, Some(nvidia.clone())) {
        solstone_core_local::BackendSelection::Selected(choice) => LocalCheckGpuSelection {
            vulkan_device_index: override_index,
            local_backend: Some(choice.backend),
            gpu_evaluation_error: None,
            bundled_local_selected_or_planned,
        },
        solstone_core_local::BackendSelection::IntegrityBlocked => LocalCheckGpuSelection {
            vulkan_device_index: None,
            local_backend: None,
            gpu_evaluation_error: Some("CUDA runtime integrity failure".into()),
            bundled_local_selected_or_planned,
        },
    }
}

/// Gather the same real host state used by `journal check` for all native consumers.
#[must_use]
pub fn gather_host_inputs(journal: &Path, version: &str) -> CheckInputs {
    gather_host_inputs_with(
        journal,
        version,
        std::env::consts::OS,
        solstone_core_local::probe_nvidia_gpu,
        solstone_core_system::vulkan_observe::observe_vulkan_devices,
    )
}

#[must_use]
pub fn gather_host_inputs_with<N, V>(
    journal: &Path,
    version: &str,
    os: &str,
    nvidia_fn: N,
    vulkan_fn: V,
) -> CheckInputs
where
    N: FnOnce() -> solstone_core_local::nvidia::NvidiaProbe,
    V: FnOnce() -> solstone_core_system::vulkan_observe::VulkanObservation,
{
    let nvidia = if os.eq_ignore_ascii_case("windows") {
        solstone_core_local::nvidia::NvidiaProbe::absent()
    } else {
        nvidia_fn()
    };
    let (tiering_memory_mib, memory_source) = if let Some(vram) = nvidia.vram_mib {
        (Some(vram), "nvidia_vram")
    } else if let Some(unified) = nvidia.unified_memory_mib {
        (Some(unified), "system_available")
    } else {
        (None, "unavailable")
    };
    let vulkan_obs = vulkan_fn();
    let selection = local_check_gpu_selection(journal, &nvidia);
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
            probe_ok: vulkan_obs.succeeded,
            devices: vulkan_obs.devices,
        },
        render_nodes_present_but_inaccessible: render_nodes_present_but_inaccessible(Path::new(
            "/dev/dri",
        )),
        gpu_evaluation_error: selection.gpu_evaluation_error,
        version: version.into(),
        ced: {
            let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
            ced_input_from(evaluate_host_ced(journal, os, arch))
        },
        rfdetr: {
            let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
            rfdetr_input_from(evaluate_host_rfdetr(journal, os, arch))
        },
        vulkan_device_index: selection.vulkan_device_index,
        local_backend: selection.local_backend,
        bundled_local_selected_or_planned: selection.bundled_local_selected_or_planned,
    }
}
pub fn evaluate_host_rfdetr(journal: &Path, os: &str, arch: &str) -> RfdetrReadiness {
    if solstone_core_local::install::rfdetr_install::rfdetr_uses_package_payload(os, arch) {
        solstone_core_local::install::rfdetr_readiness::evaluate_windows_rfdetr_readiness(
            probe_windows_rfdetr_help,
        )
    } else {
        evaluate_rfdetr_readiness(journal, os, arch)
    }
}

#[cfg(windows)]
fn probe_windows_rfdetr_help(
    package: &solstone_core_local::install::rfdetr_windows::WindowsRfdetrPackage,
) -> Value {
    use solstone_core_local::install::rfdetr_windows::rfdetr_windows_help_launch;
    use solstone_core_system::process::{
        BoundedHelperBudget, BoundedHelperError, BoundedHelperRequest, BoundedHelperResourceLimits,
        run_bounded_helper,
    };

    let system_root = match std::env::var_os("SystemRoot") {
        Some(val) if !val.is_empty() => val,
        _ => {
            return json!({"runnable": false, "reason_code": "binary_unavailable", "message": "SystemRoot environment variable is not set"});
        }
    };
    let spec = rfdetr_windows_help_launch(package, system_root);
    let request = BoundedHelperRequest {
        resources: solstone_core_system::process::BoundedHelperResources::new(),
        package_root: spec.package_root,
        executable: spec.executable,
        current_directory: spec.current_directory,
        arguments: spec.arguments,
        environment: spec.environment,
        stdin: spec.stdin,
        budget: BoundedHelperBudget {
            timeout: spec.timeout,
            stdin_limit_bytes: spec.stdin_limit_bytes,
            stdout_limit_bytes: spec.stdout_limit_bytes,
            stderr_limit_bytes: spec.stderr_limit_bytes,
        },
        resource_limits: Some(BoundedHelperResourceLimits {
            cpu_rate_per_10_000: spec.cpu_rate_per_10_000,
            committed_memory_bytes: spec.committed_memory_bytes,
        }),
        stdout_sink: None,
        cancellation: None,
    };
    match run_bounded_helper(request) {
        Ok(output) if output.exit_code == 0 => {
            json!({"runnable": true, "reason_code": Value::Null})
        }
        Ok(output) => {
            json!({"runnable": false, "reason_code": "binary_exit", "exit_code": output.exit_code})
        }
        Err(error) => {
            let reason = if error.cleanup().is_none()
                && matches!(error.cause(), BoundedHelperError::DeadlineExceeded { .. })
            {
                "timeout"
            } else {
                "binary_unavailable"
            };
            json!({"runnable": false, "reason_code": reason, "message": error.to_string()})
        }
    }
}

#[cfg(not(windows))]
fn probe_windows_rfdetr_help(
    _package: &solstone_core_local::install::rfdetr_windows::WindowsRfdetrPackage,
) -> Value {
    json!({"runnable": false, "reason_code": "windows_only"})
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
        cause: None,
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
    unified_memory: bool,
    bundled_local_selected_or_planned: bool,
) -> String {
    // Missing journal config is a planned bundled-local install.
    // A present config uses that config's bundled-local selection.
    let decision = parakeet_placement_for_devices(
        devices,
        selected,
        unified_memory,
        bundled_local_selected_or_planned,
    );
    cpu_placement_suffix(selected, decision.force_cpu)
}
/// Whether this host runs any of the bundled local capabilities.
///
/// The per-capability rows carry hardware and asset readiness. This gate only
/// decides whether the host's native platform can reach those checks.
fn supported(platform: &PlatformInput) -> bool {
    (platform.os == "Darwin" && platform.arch == "arm64")
        || (platform.os == "Linux" && matches!(platform.arch.as_str(), "x86_64" | "aarch64"))
        || (platform.os == "Windows" && matches!(platform.arch.as_str(), "x86_64" | "aarch64"))
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
        let detail = if inputs.platform.os == "Darwin" {
            "Intel Macs aren't supported — use an Apple Silicon Mac, or run the journal on supported Linux.".into()
        } else {
            format!(
                "{}/{} can't run the bundled local models yet. supported platforms are apple silicon macos, x86_64/aarch64 linux and x86_64/aarch64 windows.",
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
    let platform_detail = match inputs.platform.os.as_str() {
        "Darwin" => "Apple Silicon macOS (arm64)".into(),
        "Windows" => format!("Windows ({})", inputs.platform.arch),
        _ => format!("Linux ({})", inputs.platform.arch),
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
/// Stable diagnostic string for each CED degraded cause. Not owner-facing
/// copy -- CED detail is now the status detail; this is what a future
/// investigator (or `journal check --json`) reads to tell `Absent`,
/// `IntegrityInvalid` and `Unloadable` apart as a stable snake_case token.
///
/// `main` models CED readiness with the richer [`CapabilityStatus`] (it carries a
/// `capability` and a free-text `detail`), but its check builder discarded that
/// distinction: all four degraded variants rendered one indistinguishable warning.
/// The burn-in fix this replaces surfaced the cause on the diagnostic surface; keep
/// that, mapped onto the newer vocabulary. `Ready` has no cause and is handled by
/// the caller before this is reached.
fn ced_cause_str(status: &CapabilityStatus) -> &'static str {
    status.cause_token().unwrap_or("ready")
}
/// Same as [`ced_cause_str`], for RF-DETR's degraded causes.
fn rfdetr_cause_str(cause: RfdetrDegradedCause) -> &'static str {
    match cause {
        RfdetrDegradedCause::Absent => "absent",
        RfdetrDegradedCause::IntegrityInvalid => "integrity_invalid",
        RfdetrDegradedCause::Unrunnable => "unrunnable",
    }
}
fn ced_check(inputs: &CheckInputs) -> Option<Check> {
    match &inputs.ced {
        None => None,
        Some(CapabilityStatus::Ready) => {
            Some(check("ced", Severity::Ok, CED_READY_DETAIL, None, None))
        }
        Some(status) => {
            let detail = status.detail().expect("non-ready status carries detail");
            let mut item = check("ced", Severity::Warning, detail, None, None);
            item.cause = Some(ced_cause_str(status));
            Some(item)
        }
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
        RfdetrCheckInput::Degraded { cause } => {
            // ⚠ Warning, not Blocked, and deliberately identical to `ced_check`.
            // Object detection is an optional enhancement -- this check's own
            // guidance says "Screen descriptions will continue" -- so a degraded
            // RF-DETR must not set `overall` to Blocked.
            // 🔴 `platform.os` is a DISPLAY field ("Darwin"/"Linux"/"Windows")
            // and this predicate wants the canonical pair. It matched only
            // because Windows was the one platform whose display value happened
            // to be lowercase, so capitalizing it for consistency would have
            // silently swapped the Windows package guidance for the Unix one.
            // Canonicalize instead of reading the display string.
            let lowercase_os = inputs.platform.os.to_ascii_lowercase();
            let (guidance_os, guidance_arch) =
                canonical_host_pair(&lowercase_os, &inputs.platform.arch);
            let guidance = solstone_core_local::install::rfdetr_windows::rfdetr_degraded_guidance(
                guidance_os,
                guidance_arch,
            );
            let mut item = check("rfdetr", Severity::Warning, guidance, None, None);
            item.cause = Some(rfdetr_cause_str(*cause));
            Some(item)
        }
    }
}
fn mac_memory(memory: &MemoryInput) -> Check {
    match apple_total_memory_verdict(memory.total_bytes) {
        AppleTotalMemoryVerdict::Unreadable => check(
            "memory",
            Severity::Unknown,
            "available memory could not be verified",
            Some(APPLE_SILICON_TOTAL_MEMORY_MIN_BYTES),
            None,
        ),
        AppleTotalMemoryVerdict::BelowBar => {
            let total = memory.total_bytes.unwrap_or(0);
            check(
                "memory",
                Severity::Blocked,
                format!(
                    "Apple Silicon needs at least 16 GB of memory for the bundled local models (this Mac has {} GB)",
                    label(total)
                ),
                Some(APPLE_SILICON_TOTAL_MEMORY_MIN_BYTES),
                Some(total),
            )
        }
        AppleTotalMemoryVerdict::Eligible => {
            let total = memory.total_bytes.unwrap_or(0);
            if memory
                .available_bytes
                .is_some_and(|available| available < MAC_AVAILABLE_MIN)
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
            } else {
                check(
                    "memory",
                    Severity::Ok,
                    format!("{} GB of memory meets the 16 GB minimum", label(total)),
                    Some(APPLE_SILICON_TOTAL_MEMORY_MIN_BYTES),
                    Some(total),
                )
            }
        }
    }
}
fn windows_vulkan_verdict_check(
    probe_ok: bool,
    devices: &[VulkanDevice],
    override_index: Option<u32>,
    selected: Option<&VulkanDevice>,
    bundled_local_selected_or_planned: bool,
) -> Check {
    match gpu_device_local_verdict(probe_ok, devices, override_index) {
        GpuDeviceLocalVerdict::Eligible { available_bytes } => {
            let selected = selected.expect("eligible verdict requires a selected device");
            check(
                "gpu",
                Severity::Ok,
                format!(
                    "Vulkan GPU {} with {} GB{}",
                    selected.name,
                    label(available_bytes),
                    placement_suffix(
                        devices,
                        Some(selected),
                        false,
                        bundled_local_selected_or_planned
                    )
                ),
                Some(GPU_DEVICE_LOCAL_MIN_BYTES),
                Some(available_bytes),
            )
        }
        GpuDeviceLocalVerdict::BelowBar { available_bytes } => {
            let selected = selected.expect("below-bar verdict requires a selected device");
            check(
                "gpu",
                Severity::Blocked,
                format!(
                    "GPU {} has {} GB; the bundled local models need at least 6 GB",
                    selected.name,
                    label(available_bytes)
                ),
                Some(GPU_DEVICE_LOCAL_MIN_BYTES),
                Some(available_bytes),
            )
        }
        GpuDeviceLocalVerdict::Unknown => check(
            "gpu",
            Severity::Unknown,
            "couldn't read GPU memory. local thinking needs at least 6 GB",
            Some(GPU_DEVICE_LOCAL_MIN_BYTES),
            None,
        ),
        GpuDeviceLocalVerdict::NoHardwareDevice => check(
            "gpu",
            Severity::Blocked,
            "no usable GPU found — the bundled local models need a hardware GPU",
            None,
            None,
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
        .then(|| select_device(&inputs.vulkan.devices, inputs.vulkan_device_index))
        .flatten();
    if !inputs.nvidia.detected {
        if (!inputs.vulkan.probe_ok || selected.is_none())
            && inputs.render_nodes_present_but_inaccessible
        {
            return check("gpu", Severity::Unknown, RENDER_HINT, None, None);
        }
        if !inputs.vulkan.probe_ok {
            let (detail, expected) = if inputs.platform.os == "Windows" {
                (
                    "couldn't read GPU memory. local thinking needs at least 6 GB".to_string(),
                    Some(GPU_DEVICE_LOCAL_MIN_BYTES),
                )
            } else {
                (
                    "no NVIDIA GPU found and the Vulkan probe did not complete — GPU readiness is unknown".to_string(),
                    None,
                )
            };
            return check("gpu", Severity::Unknown, detail, expected, None);
        }
        if inputs.platform.os == "Windows" {
            return windows_vulkan_verdict_check(
                inputs.vulkan.probe_ok,
                &inputs.vulkan.devices,
                inputs.vulkan_device_index,
                selected.as_ref(),
                inputs.bundled_local_selected_or_planned,
            );
        }
        let Some(selected) = selected else {
            return check(
                "gpu",
                Severity::Blocked,
                "no usable GPU found — the bundled local models need a hardware GPU with at least 6 GB",
                Some(GPU_DEVICE_LOCAL_MIN_BYTES),
                None,
            );
        };
        let bytes = selected.vram_mib * 1024 * 1024;
        if bytes < GPU_DEVICE_LOCAL_MIN_BYTES {
            return check(
                "gpu",
                Severity::Blocked,
                format!(
                    "GPU {} has {} GB — the bundled local models need at least 6 GB",
                    selected.name,
                    label(bytes)
                ),
                Some(GPU_DEVICE_LOCAL_MIN_BYTES),
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
                    false,
                    inputs.bundled_local_selected_or_planned
                )
            ),
            Some(GPU_DEVICE_LOCAL_MIN_BYTES),
            Some(bytes),
        );
    }
    if inputs.local_backend == Some(solstone_core_local::Backend::Vulkan) {
        return windows_vulkan_verdict_check(
            inputs.vulkan.probe_ok,
            &inputs.vulkan.devices,
            inputs.vulkan_device_index,
            selected.as_ref(),
            inputs.bundled_local_selected_or_planned,
        );
    }
    let effective = inputs.nvidia.vram_mib.or(inputs.nvidia.tiering_memory_mib);
    let Some(mib) = effective else {
        return check(
            "gpu",
            Severity::Unknown,
            "NVIDIA GPU detected but its memory could not be read — GPU readiness is unknown",
            Some(GPU_DEVICE_LOCAL_MIN_BYTES),
            None,
        );
    };
    let bytes = mib * 1024 * 1024;
    if bytes < GPU_DEVICE_LOCAL_MIN_BYTES {
        return check(
            "gpu",
            Severity::Blocked,
            format!(
                "the NVIDIA GPU has {} GB — the bundled local models need at least 6 GB",
                label(bytes)
            ),
            Some(GPU_DEVICE_LOCAL_MIN_BYTES),
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
        unified,
        inputs.bundled_local_selected_or_planned,
    ));
    check(
        "gpu",
        Severity::Ok,
        detail,
        Some(GPU_DEVICE_LOCAL_MIN_BYTES),
        Some(bytes),
    )
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
    let checks = report.checks.iter().map(|item| json!({"name": item.name, "severity": item.severity, "detail": item.detail, "required_bytes": item.required_bytes, "available_bytes": item.available_bytes, "cause": item.cause})).collect::<Vec<_>>();
    let value: Value = json!({"platform":{"os":report.platform.os,"os_version":report.platform.os_version,"arch":report.platform.arch,"python":null,"supported":report.platform.supported},"checks":checks,"overall":report.overall,"feedback_url":FEEDBACK_URL,"version":report.version});
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("JSON value serializes")
    )
}
pub fn human_output(report: &CheckReport) -> String {
    let mut lines = vec![
        "solstone journal check — can this computer run the journal with the bundled local models?"
            .to_owned(),
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
/// Shared host entry point: Windows owns its helper through the bounded Job
/// facade; Unix retains the existing local verdict and process runner.
pub fn evaluate_host_ced(journal: &Path, os: &str, arch: &str) -> CedVerdict {
    #[cfg(windows)]
    {
        solstone_core_local::install::ced_readiness::evaluate_ced_readiness_with_probe(
            journal,
            os,
            arch,
            ced_windows::probe,
        )
    }
    #[cfg(not(windows))]
    {
        solstone_core_local::install::ced_readiness::evaluate_ced_readiness(journal, os, arch)
    }
}

#[cfg(windows)]
pub mod ced_windows;

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
            vulkan_device_index: None,
            local_backend: None,
            bundled_local_selected_or_planned: true,
        }
    }

    #[test]
    fn ready_rfdetr_is_ok_and_does_not_change_overall() {
        let inputs = check_inputs(None, RfdetrCheckInput::Ready);
        let rfdetr = rfdetr_check(&inputs).expect("ready RF-DETR check");
        assert_eq!(rfdetr.severity, Severity::Ok);
        assert_eq!(overall(&[rfdetr]), Severity::Ok);
    }

    /// A degraded optional capability warns; it does not declare the machine unfit.
    ///
    /// ⚠ This test previously asserted `Blocked` for all three causes. That
    /// expectation was wrong: `overall == Blocked` renders as "Not ready -- this
    /// computer can't run the bundled local models yet", which contradicts this
    /// check's own detail ("Screen descriptions will continue") and was false on
    /// an owner's 2.0.0 journal, which ran normally while reporting
    /// `overall: blocked` and exit 2 over a stale RF-DETR sidecar. CED is the
    /// same class of optional inference asset and has always warned; the two
    /// must agree.
    #[test]
    fn degraded_rfdetr_warns_for_every_cause_and_matches_ced() {
        for (cause, expected_cause_str) in [
            (RfdetrDegradedCause::Absent, "absent"),
            (RfdetrDegradedCause::IntegrityInvalid, "integrity_invalid"),
            (RfdetrDegradedCause::Unrunnable, "unrunnable"),
        ] {
            let inputs = check_inputs(None, RfdetrCheckInput::Degraded { cause });
            let rfdetr = rfdetr_check(&inputs).expect("degraded RF-DETR check");
            assert_eq!(rfdetr.severity, Severity::Warning, "{cause:?}");
            // Done condition 7: the owner-facing `detail` stays the single
            // shared guidance sentence (unchanged, out of bounds to reword),
            // but `cause` now distinguishes which of the three fired.
            assert_eq!(rfdetr.detail, RFDETR_UNAVAILABLE_GUIDANCE);
            assert_eq!(rfdetr.cause, Some(expected_cause_str), "{cause:?}");
            assert_eq!(overall(&[rfdetr]), Severity::Warning, "{cause:?}");
        }

        // The two optional-asset checks must not disagree about severity.
        let degraded_ced = ced_check(&check_inputs(
            Some(CapabilityStatus::UnloadableOrUnrunnable {
                capability: CED_CAPABILITY.to_owned(),
                detail: "ced library failed to load".to_owned(),
            }),
            RfdetrCheckInput::Omit,
        ))
        .expect("degraded CED check");
        let degraded_rfdetr = rfdetr_check(&check_inputs(
            None,
            RfdetrCheckInput::Degraded {
                cause: RfdetrDegradedCause::Unrunnable,
            },
        ))
        .expect("degraded RF-DETR check");
        assert_eq!(
            degraded_rfdetr.severity, degraded_ced.severity,
            "CED and RF-DETR are both optional inference assets and must agree"
        );
    }

    /// Windows is a first-class `check` platform, and the rows it renders are
    /// the capabilities it actually ships.
    ///
    /// 🔴 Before this, `supported()` admitted only macOS and Linux, so a Windows
    /// owner running `journal check` got a single `platform` row, `overall:
    /// blocked`, and a sentence saying the machine "can't run the bundled local
    /// models yet" -- from a build that ships CED and RF-DETR, runs both, and
    /// downloads a CED model on request. The diagnostic an owner reaches for
    /// first was the one surface lying to them.
    #[test]
    fn windows_renders_the_capabilities_it_actually_ships() {
        let mut inputs = check_inputs(Some(CapabilityStatus::Ready), RfdetrCheckInput::Ready);
        inputs.platform.os = "Windows".into();
        inputs.vulkan.devices = vec![VulkanDevice {
            index: 0,
            name: "RTX 4090".into(),
            device_type: Some(1),
            vram_mib: 24_000,
        }];

        for arch in ["x86_64", "aarch64"] {
            inputs.platform.arch = arch.into();
            let report = build_check_report(&inputs);
            assert!(report.platform.supported, "windows/{arch} is supported");
            assert_eq!(report.overall, Severity::Ok);
            assert_eq!(report.recommended_package, Some("solstone-journal"));

            let names: Vec<_> = report.checks.iter().map(|item| item.name).collect();
            assert!(names.contains(&"ced"), "{names:?}");
            assert!(names.contains(&"rfdetr"), "{names:?}");
            assert!(names.contains(&"gpu"), "{names:?}");
            assert_eq!(report.checks[0].name, "platform");
            assert_eq!(report.checks[0].severity, Severity::Ok);
        }
    }

    /// An unshipped architecture still stops before capability checks.
    #[test]
    fn an_unsupported_host_is_told_what_is_actually_supported() {
        let mut inputs = check_inputs(None, RfdetrCheckInput::Omit);
        inputs.platform.os = "Windows".into();
        inputs.platform.arch = "x86".into();

        let report = build_check_report(&inputs);
        assert!(!report.platform.supported);
        assert_eq!(report.overall, Severity::Blocked);
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.checks[0].name, "platform");
        assert_eq!(report.recommended_package, None);
    }

    #[test]
    fn windows_arm64_capabilities_preserve_hardware_admission() {
        let mut inputs = check_inputs(None, RfdetrCheckInput::Omit);
        inputs.platform.os = "Windows".into();
        inputs.platform.arch = "aarch64".into();
        for (probe_ok, device_type, vram_mib, expected) in [
            (true, 2, 6144, Severity::Ok),
            (true, 2, 6143, Severity::Blocked),
            (true, 4, 8192, Severity::Blocked),
            (true, 2, 0, Severity::Unknown),
            (false, 2, 8192, Severity::Unknown),
        ] {
            inputs.vulkan.probe_ok = probe_ok;
            inputs.vulkan.devices = vec![VulkanDevice {
                index: 0,
                name: "Test GPU".into(),
                device_type: Some(device_type),
                vram_mib,
            }];
            let report = build_check_report(&inputs);
            assert!(report.platform.supported);
            let gpu = report.checks.iter().find(|row| row.name == "gpu").unwrap();
            assert_eq!(
                gpu.severity, expected,
                "{probe_ok}/{device_type}/{vram_mib}"
            );
        }
        inputs.vulkan.devices.clear();
        inputs.vulkan.probe_ok = true;
        let report = build_check_report(&inputs);
        assert_eq!(report.checks[1].severity, Severity::Blocked);
    }

    #[cfg(all(test, feature = "full-tests"))]
    /// `host_platform()` and `supported()` have to name the OS the same way, and
    /// they did not: the mapping left Windows as the raw lowercase `"windows"`
    /// while a downstream arm tested `== "Windows"`, so that arm was unreachable
    /// and nobody noticed. This pins the value each target actually produces.
    #[test]
    fn the_host_os_name_matches_what_every_downstream_arm_tests_for() {
        let os = host_platform().os;
        let expected = if cfg!(target_os = "macos") {
            "Darwin"
        } else if cfg!(target_os = "linux") {
            "Linux"
        } else if cfg!(windows) {
            "Windows"
        } else {
            std::env::consts::OS
        };
        assert_eq!(os, expected);
        assert!(
            !os.is_empty() && os.chars().next().is_some_and(char::is_uppercase)
                || os == std::env::consts::OS,
            "a shipped platform names itself in the display register: {os}"
        );
    }

    #[test]
    fn windows_degraded_rfdetr_uses_package_guidance() {
        use solstone_core_local::install::rfdetr_windows::RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE;

        let mut inputs = check_inputs(
            None,
            RfdetrCheckInput::Degraded {
                cause: RfdetrDegradedCause::Absent,
            },
        );
        inputs.platform.arch = "x86_64".into();

        // 🔴 BOTH spellings, deliberately. This predicate reads a DISPLAY field,
        // and it used to match only because Windows was the one platform whose
        // display value happened to be lowercase -- so capitalizing it for
        // consistency would have silently handed Windows owners the Unix
        // guidance ("run journal install-models") for a package-payload asset.
        for spelling in ["windows", "Windows"] {
            inputs.platform.os = spelling.into();
            let rfdetr = rfdetr_check(&inputs).expect("degraded RF-DETR check on Windows");
            assert_eq!(rfdetr.severity, Severity::Warning, "{spelling}");
            assert_eq!(
                rfdetr.detail, RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE,
                "{spelling}"
            );
            assert!(!rfdetr.detail.contains("install-models"), "{spelling}");
            assert_eq!(rfdetr.cause, Some("absent"), "{spelling}");
        }
    }

    /// CED counterpart of `degraded_rfdetr_blocks_for_every_cause`: same
    /// done-condition-7 guarantee (shared owner sentence, distinguishable
    /// `cause`), for every non-Ready [`CapabilityStatus`] variant.
    ///
    /// Ported from the burn-in's `CedDegradedCause` form when `main` replaced that
    /// enum with the richer `CapabilityStatus`. `main`'s builder collapsed every
    /// degraded variant into one indistinguishable warning; this is the test that
    /// keeps them distinguishable, now covering the two variants `main` added.
    #[test]
    fn degraded_ced_warns_and_surfaces_every_cause() {
        let detail = "ced degraded".to_owned();
        let capability = CED_CAPABILITY.to_owned();
        for (status, expected_cause_str) in [
            (
                CapabilityStatus::Absent {
                    capability: capability.clone(),
                    detail: detail.clone(),
                },
                "absent",
            ),
            (
                CapabilityStatus::IntegrityInvalid {
                    capability: capability.clone(),
                    detail: detail.clone(),
                },
                "integrity_invalid",
            ),
            (
                CapabilityStatus::UnloadableOrUnrunnable {
                    capability: capability.clone(),
                    detail: detail.clone(),
                },
                "unloadable",
            ),
            (
                CapabilityStatus::WrongAbiOrProtocol {
                    capability: capability.clone(),
                    detail: detail.clone(),
                },
                "wrong_abi_or_protocol",
            ),
            (
                CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                    capability: capability.clone(),
                    detail: detail.clone(),
                },
                "resource_or_owner_scope_unavailable",
            ),
        ] {
            let inputs = check_inputs(Some(status.clone()), RfdetrCheckInput::Omit);
            let ced = ced_check(&inputs).expect("degraded CED check");
            assert_eq!(ced.severity, Severity::Warning, "{status:?}");
            assert_eq!(ced.detail, detail);
            assert_eq!(ced.cause, Some(expected_cause_str), "{status:?}");
        }
    }

    /// Done condition 7, end to end: the distinguishing cause reaches the
    /// actual diagnostic surface (`journal check --json`'s `cause` key),
    /// not just the in-memory `Check` value -- and a Ready check (no cause
    /// to report) serializes `cause` as `null` rather than omitting or
    /// misreporting the field.
    #[test]
    fn json_output_surfaces_the_distinguishing_cause() {
        let inputs = check_inputs(
            Some(CapabilityStatus::UnloadableOrUnrunnable {
                capability: CED_CAPABILITY.to_owned(),
                detail: "ced library failed to load".to_owned(),
            }),
            RfdetrCheckInput::Ready,
        );
        let report = build_check_report(&inputs);
        let rendered = json_output(&report);
        let parsed: Value = serde_json::from_str(&rendered).expect("valid JSON");
        let checks = parsed["checks"].as_array().expect("checks array");
        let ced = checks
            .iter()
            .find(|item| item["name"] == "ced")
            .expect("ced check present");
        assert_eq!(ced["cause"], json!("unloadable"), "{rendered}");
        let rfdetr = checks
            .iter()
            .find(|item| item["name"] == "rfdetr")
            .expect("rfdetr check present");
        assert_eq!(rfdetr["cause"], Value::Null, "{rendered}");
    }

    /// Brief A, done condition 6: once both provider readiness verdicts are
    /// Ready (the fixed CED path plus a correctly re-installed RF-DETR),
    /// the severity combination `rfdetr_check`'s `Degraded -> Blocked`
    /// feeds into `overall()` must not read `Blocked` -- that propagation is
    /// what currently suppresses the unrelated text-generation bootstrap via
    /// setup's `local_provider_readiness`. Isolated to just the ced/rfdetr checks
    /// (matching `ced_warning_stays_independent_from_ready_rfdetr` below) so
    /// an unrelated host-fit check (gpu/ram/disk) can't confound the result;
    /// this does not depend on how either provider became Ready, only on
    /// the propagation once both readiness verdicts agree the assets work.
    #[test]
    fn both_providers_ready_leaves_overall_ok() {
        let inputs = check_inputs(Some(CapabilityStatus::Ready), RfdetrCheckInput::Ready);
        let overall_severity = overall(&[
            ced_check(&inputs).expect("ready CED check"),
            rfdetr_check(&inputs).expect("ready RF-DETR check"),
        ]);
        assert_eq!(overall_severity, Severity::Ok);
        assert_ne!(overall_severity, Severity::Blocked);
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
            vulkan_device_index: None,
            local_backend: None,
            bundled_local_selected_or_planned: true,
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
            vulkan_device_index: None,
            local_backend: None,
            bundled_local_selected_or_planned: true,
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
            vulkan_device_index: None,
            local_backend: None,
            bundled_local_selected_or_planned: true,
        };
        let report = build_check_report(&inputs);
        assert_eq!(
            report.checks[1].detail,
            "Vulkan GPU Test GPU with 6 GB; a model runs on your GPU; transcription runs on your CPU on this machine"
        );
    }

    #[test]
    fn placement_suffix_tracks_selected_bundled_model() {
        let mut inputs = check_inputs(None, RfdetrCheckInput::Ready);
        inputs.vulkan.devices = vec![VulkanDevice {
            index: 0,
            name: "Test GPU".into(),
            device_type: Some(2),
            vram_mib: 8192,
        }];
        let selected = inputs.vulkan.devices.first();
        assert!(placement_suffix(&inputs.vulkan.devices, selected, false, false).is_empty());
        assert!(!placement_suffix(&inputs.vulkan.devices, selected, false, true).is_empty());
    }

    #[test]
    #[cfg(feature = "full-tests")]
    fn gather_host_inputs_with_windows_platform_skips_nvidia_and_calls_vulkan_once() {
        let hardware = VulkanDevice {
            index: 0,
            name: "RTX".into(),
            device_type: Some(1),
            vram_mib: 16384,
        };
        for obs in [
            solstone_core_system::vulkan_observe::VulkanObservation {
                devices: vec![hardware],
                succeeded: true,
            },
            solstone_core_system::vulkan_observe::VulkanObservation {
                devices: Vec::new(),
                succeeded: true,
            },
            solstone_core_system::vulkan_observe::VulkanObservation {
                devices: Vec::new(),
                succeeded: false,
            },
        ] {
            let mut nvidia_calls = 0;
            let mut vulkan_calls = 0;
            let expected_obs = obs.clone();
            let inputs = gather_host_inputs_with(
                Path::new("/journal"),
                "1.0.0",
                "windows",
                || {
                    nvidia_calls += 1;
                    solstone_core_local::nvidia::NvidiaProbe {
                        schema: "solstone-local-nvidia-probe-v1".into(),
                        detected: true,
                        gpu_index: Some(0),
                        gpu_name: Some("RTX".into()),
                        compute_cap: None,
                        arch: None,
                        driver_cuda_major: None,
                        vram_mib: Some(16384),
                        unified_memory_mib: None,
                        probe_error: None,
                    }
                },
                || {
                    vulkan_calls += 1;
                    obs
                },
            );
            assert_eq!(nvidia_calls, 0);
            assert_eq!(vulkan_calls, 1);
            assert!(!inputs.nvidia.detected);
            assert_eq!(inputs.vulkan.probe_ok, expected_obs.succeeded);
            assert_eq!(inputs.vulkan.devices, expected_obs.devices);

            let mut windows_inputs = inputs;
            windows_inputs.platform = PlatformInput {
                os: "Windows".into(),
                os_version: "10.0".into(),
                arch: "x86_64".into(),
            };
            let report = build_check_report(&windows_inputs);
            assert!(
                report.checks.iter().any(|c| c.name == "gpu"),
                "Windows check report must render gpu row"
            );
        }
    }

    #[test]
    fn windows_gpu_verdict_rows_and_linux_probe_failure() {
        let base_inputs = |os: &str, probe_ok: bool, devices: Vec<VulkanDevice>| -> CheckInputs {
            let mut inputs = check_inputs(Some(CapabilityStatus::Ready), RfdetrCheckInput::Ready);
            inputs.platform.os = os.into();
            inputs.platform.arch = "x86_64".into();
            inputs.nvidia.detected = false;
            inputs.vulkan.probe_ok = probe_ok;
            inputs.vulkan.devices = devices;
            inputs
        };

        for mib in [4096, 6143] {
            let dev = vec![VulkanDevice {
                index: 0,
                name: "RTX".into(),
                device_type: Some(1),
                vram_mib: mib,
            }];
            let report = build_check_report(&base_inputs("Windows", true, dev));
            let gpu = report
                .checks
                .iter()
                .find(|c| c.name == "gpu")
                .expect("gpu check");
            assert_eq!(gpu.severity, Severity::Blocked, "mib: {mib}");
            assert_eq!(
                gpu.required_bytes,
                Some(GPU_DEVICE_LOCAL_MIN_BYTES),
                "mib: {mib}"
            );
            assert_eq!(gpu.available_bytes, Some(mib * 1024 * 1024), "mib: {mib}");
        }

        let dev_6144 = vec![VulkanDevice {
            index: 0,
            name: "RTX".into(),
            device_type: Some(1),
            vram_mib: 6144,
        }];
        let report = build_check_report(&base_inputs("Windows", true, dev_6144));
        let gpu = report
            .checks
            .iter()
            .find(|c| c.name == "gpu")
            .expect("gpu check");
        assert_eq!(gpu.severity, Severity::Ok);
        assert_eq!(gpu.required_bytes, Some(GPU_DEVICE_LOCAL_MIN_BYTES));
        assert_eq!(gpu.available_bytes, Some(6144 * 1024 * 1024));

        let report_zero = build_check_report(&base_inputs(
            "Windows",
            true,
            vec![VulkanDevice {
                index: 0,
                name: "unreadable memory".into(),
                device_type: Some(1),
                vram_mib: 0,
            }],
        ));
        let zero_gpu = report_zero.checks.iter().find(|c| c.name == "gpu").unwrap();
        assert_eq!(zero_gpu.severity, Severity::Unknown);
        assert_eq!(zero_gpu.available_bytes, None);
        assert_eq!(zero_gpu.required_bytes, Some(GPU_DEVICE_LOCAL_MIN_BYTES));

        let report_win_failed = build_check_report(&base_inputs("Windows", false, Vec::new()));
        let gpu_win = report_win_failed
            .checks
            .iter()
            .find(|c| c.name == "gpu")
            .expect("gpu check");
        assert_eq!(gpu_win.severity, Severity::Unknown);
        assert_eq!(gpu_win.required_bytes, Some(GPU_DEVICE_LOCAL_MIN_BYTES));
        assert_eq!(gpu_win.available_bytes, None);

        let report_linux_failed = build_check_report(&base_inputs("Linux", false, Vec::new()));
        let gpu_linux = report_linux_failed
            .checks
            .iter()
            .find(|c| c.name == "gpu")
            .expect("gpu check");
        assert_eq!(gpu_linux.severity, Severity::Unknown);
        assert_eq!(gpu_linux.required_bytes, None);
        assert_eq!(gpu_linux.available_bytes, None);
    }

    #[test]
    fn mac_memory_evaluates_against_shared_local_bar() {
        let mac_inputs = |total: Option<u64>, available: Option<u64>| -> CheckInputs {
            let mut inputs = check_inputs(Some(CapabilityStatus::Ready), RfdetrCheckInput::Ready);
            inputs.platform.os = "Darwin".into();
            inputs.platform.arch = "arm64".into();
            inputs.memory.total_bytes = total;
            inputs.memory.available_bytes = available;
            inputs
        };

        let report_below = build_check_report(&mac_inputs(
            Some(APPLE_SILICON_TOTAL_MEMORY_MIN_BYTES - 1),
            Some(14 * GIB),
        ));
        let mem_below = report_below
            .checks
            .iter()
            .find(|c| c.name == "memory")
            .expect("memory check");
        assert_eq!(mem_below.severity, Severity::Blocked);

        let report_ok = build_check_report(&mac_inputs(
            Some(APPLE_SILICON_TOTAL_MEMORY_MIN_BYTES),
            Some(14 * GIB),
        ));
        let mem_ok = report_ok
            .checks
            .iter()
            .find(|c| c.name == "memory")
            .expect("memory check");
        assert_eq!(mem_ok.severity, Severity::Ok);

        let report_unknown = build_check_report(&mac_inputs(None, None));
        let mem_unknown = report_unknown
            .checks
            .iter()
            .find(|c| c.name == "memory")
            .expect("memory check");
        assert_eq!(mem_unknown.severity, Severity::Unknown);
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn local_check_gpu_selection_contract() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path();

        let probe = solstone_core_local::NvidiaProbe {
            schema: "solstone-local-nvidia-probe-v1".into(),
            detected: false,
            gpu_index: None,
            gpu_name: None,
            compute_cap: None,
            arch: None,
            driver_cuda_major: None,
            vram_mib: None,
            unified_memory_mib: None,
            probe_error: None,
        };

        let test_devices = vec![
            VulkanDevice {
                index: 0,
                name: "default-discrete".into(),
                device_type: Some(2),
                vram_mib: 24576,
            },
            VulkanDevice {
                index: 1,
                name: "configured-small".into(),
                device_type: Some(2),
                vram_mib: 4096,
            },
        ];

        // Missing config file
        let sel = local_check_gpu_selection(journal, &probe);
        assert_eq!(sel.vulkan_device_index, None);
        assert_eq!(
            sel.local_backend,
            Some(solstone_core_local::Backend::Vulkan)
        );
        assert_eq!(sel.gpu_evaluation_error, None);
        assert!(sel.bundled_local_selected_or_planned);

        let mut inputs = check_inputs(None, RfdetrCheckInput::Ready);
        inputs.vulkan.devices = test_devices.clone();
        inputs.vulkan_device_index = sel.vulkan_device_index;
        inputs.local_backend = sel.local_backend;
        inputs.gpu_evaluation_error = sel.gpu_evaluation_error;
        inputs.bundled_local_selected_or_planned = sel.bundled_local_selected_or_planned;
        let report = build_check_report(&inputs);
        let gpu_chk = report.checks.iter().find(|c| c.name == "gpu").unwrap();
        assert_eq!(gpu_chk.severity, Severity::Ok);
        assert!(gpu_chk.detail.contains("default-discrete"));

        // Corrupt config file
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(journal.join("config/journal.json"), b"invalid json").unwrap();
        let sel = local_check_gpu_selection(journal, &probe);
        assert_eq!(sel.vulkan_device_index, None);
        assert_eq!(sel.local_backend, None);
        assert!(sel.gpu_evaluation_error.is_some());
        assert!(!sel.bundled_local_selected_or_planned);

        inputs.vulkan_device_index = sel.vulkan_device_index;
        inputs.local_backend = sel.local_backend;
        inputs.gpu_evaluation_error = sel.gpu_evaluation_error;
        inputs.bundled_local_selected_or_planned = sel.bundled_local_selected_or_planned;
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Unknown);
        assert!(!g.detail.contains("default-discrete"));

        // Valid config with vulkan_device_index: 1
        std::fs::write(
            journal.join("config/journal.json"),
            br#"{"providers": {"local": {"vulkan_device_index": 1}}}"#,
        )
        .unwrap();
        let sel = local_check_gpu_selection(journal, &probe);
        assert_eq!(sel.vulkan_device_index, Some(1));
        assert_eq!(
            sel.local_backend,
            Some(solstone_core_local::Backend::Vulkan)
        );
        assert_eq!(sel.gpu_evaluation_error, None);
        assert!(!sel.bundled_local_selected_or_planned);

        inputs.vulkan_device_index = sel.vulkan_device_index;
        inputs.local_backend = sel.local_backend;
        inputs.gpu_evaluation_error = sel.gpu_evaluation_error;
        inputs.bundled_local_selected_or_planned = sel.bundled_local_selected_or_planned;
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Blocked);
        assert!(g.detail.contains("configured-small"));

        std::fs::write(
            journal.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"local"}}}"#,
        )
        .unwrap();
        let sel = local_check_gpu_selection(journal, &probe);
        assert!(sel.bundled_local_selected_or_planned);
        assert_eq!(sel.gpu_evaluation_error, None);
    }

    #[test]
    fn check_gpu_respects_vulkan_override_and_backend() {
        let pair_a = vec![
            VulkanDevice {
                index: 0,
                name: "default-large".into(),
                device_type: Some(2),
                vram_mib: 24576,
            },
            VulkanDevice {
                index: 1,
                name: "configured-small".into(),
                device_type: Some(2),
                vram_mib: 4096,
            },
        ];

        let pair_b = vec![
            VulkanDevice {
                index: 0,
                name: "default-small".into(),
                device_type: Some(2),
                vram_mib: 4096,
            },
            VulkanDevice {
                index: 1,
                name: "configured-large".into(),
                device_type: Some(2),
                vram_mib: 24576,
            },
        ];

        // 1. Linux !nvidia.detected with pair_a and override Some(1) -> Blocked, names configured-small
        let mut inputs = check_inputs(None, RfdetrCheckInput::Ready);
        inputs.platform.os = "Linux".into();
        inputs.vulkan.probe_ok = true;
        inputs.vulkan.devices = pair_a.clone();
        inputs.vulkan_device_index = Some(1);
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Blocked);
        assert!(g.detail.contains("configured-small"));

        let report_a = build_check_report(&inputs);
        let gpu_rep_a = report_a.checks.iter().find(|c| c.name == "gpu").unwrap();
        assert_eq!(gpu_rep_a.severity, Severity::Blocked);
        assert!(gpu_rep_a.detail.contains("configured-small"));

        // Linux !nvidia.detected with pair_b and override Some(1) -> Ok, names configured-large
        inputs.vulkan.devices = pair_b.clone();
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Ok);
        assert!(g.detail.contains("configured-large"));

        let report_b = build_check_report(&inputs);
        let gpu_rep_b = report_b.checks.iter().find(|c| c.name == "gpu").unwrap();
        assert_eq!(gpu_rep_b.severity, Severity::Ok);
        assert!(gpu_rep_b.detail.contains("configured-large"));

        // 2. Windows !nvidia.detected with pair_a and override Some(1) -> Blocked
        inputs.platform.os = "Windows".into();
        inputs.vulkan.devices = pair_a.clone();
        inputs.vulkan_device_index = Some(1);
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Blocked);
        assert!(g.detail.contains("configured-small"));

        // Windows with pair_b and override Some(1) -> Ok
        inputs.vulkan.devices = pair_b.clone();
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Ok);
        assert!(g.detail.contains("configured-large"));

        // 3. Linux with nvidia.detected: true and local_backend: Vulkan on pair_a
        inputs.platform.os = "Linux".into();
        inputs.nvidia.detected = true;
        inputs.nvidia.vram_mib = Some(16384);
        inputs.local_backend = Some(solstone_core_local::Backend::Vulkan);
        inputs.vulkan.devices = pair_a.clone();
        inputs.vulkan_device_index = Some(1);
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Blocked);
        assert!(g.detail.contains("configured-small"));

        // Small NVIDIA capacity must not block the eligible selected Vulkan device.
        inputs.nvidia.vram_mib = Some(2048);
        inputs.vulkan.devices = pair_b.clone();
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Ok);
        assert!(g.detail.contains("configured-large"));

        // 4. Linux with nvidia.detected: true and local_backend: Cuda
        inputs.local_backend = Some(solstone_core_local::Backend::Cuda);
        inputs.nvidia.vram_mib = Some(16384);
        inputs.vulkan.devices = pair_a.clone();
        inputs.vulkan_device_index = Some(1); // Vulkan device 1 is below bar
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Ok);
        assert!(g.detail.contains("NVIDIA GPU with 16 GB"));

        // NVIDIA capacity below bar: refuses on NVIDIA memory, ignoring eligible Vulkan device
        inputs.nvidia.vram_mib = Some(4096);
        inputs.vulkan.devices = pair_b.clone();
        inputs.vulkan_device_index = Some(1); // Vulkan device 1 is 24 GB
        let g = gpu(&inputs);
        assert_eq!(g.severity, Severity::Blocked);
        assert!(
            g.detail
                .contains("the NVIDIA GPU has 4 GB — the bundled local models need at least 6 GB")
        );
    }
}
