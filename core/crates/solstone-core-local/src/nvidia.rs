// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
pub const CUDA_EMBEDDED_ARCH_SET: [&str; 4] = ["sm_86", "sm_89", "sm_120a", "sm_121a"];
pub const CUDA_MIN_DRIVER_VERSION: u32 = 13;
pub(crate) const NVIDIA_PROBE_SCHEMA: &str = "solstone-local-nvidia-probe-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactTrust {
    Trusted,
    Absent,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Cuda,
    Vulkan,
}

/// The source used for GPU-memory tiering. This stays structured so callers do
/// not create incompatible display spellings for the same probe fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySource {
    Unavailable,
    NvidiaVram,
    SystemAvailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendChoice {
    pub backend: Backend,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NvidiaProbe {
    pub schema: String,
    pub detected: bool,
    pub gpu_index: Option<u32>,
    pub gpu_name: Option<String>,
    pub compute_cap: Option<String>,
    pub arch: Option<String>,
    pub driver_cuda_major: Option<u32>,
    pub vram_mib: Option<u64>,
    pub unified_memory_mib: Option<u64>,
    pub probe_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NvidiaSmiRow {
    gpu_index: u32,
    gpu_name: String,
    compute_cap: Option<String>,
    arch: Option<String>,
    vram_mib: Option<u64>,
}

pub fn probe_nvidia_gpu() -> NvidiaProbe {
    let gpu_output = match run_nvidia_smi(&[
        "--query-gpu=index,name,compute_cap,driver_version,memory.total",
        "--format=csv,noheader,nounits",
    ]) {
        Ok(output) => output,
        Err(error) => return NvidiaProbe::undetected(error),
    };
    if !gpu_output.status.success() {
        return NvidiaProbe::undetected(format!(
            "NVIDIA GPU probe exited with status {}",
            gpu_output.status
        ));
    }
    let Some(row_text) = first_nonblank_line(&gpu_output.stdout) else {
        return NvidiaProbe::undetected("NVIDIA GPU probe returned no rows".to_string());
    };
    let Some(row) = parse_nvidia_smi_row(row_text) else {
        return NvidiaProbe::undetected("NVIDIA GPU probe returned invalid CSV".to_string());
    };

    let (driver_cuda_major, driver_error) = match probe_driver_cuda_major() {
        Ok(version) => (version, None),
        Err(error) => (None, Some(error)),
    };
    let (unified_memory_mib, meminfo_error) =
        if row.vram_mib.is_none() && has_unified_memory_name(&row.gpu_name) {
            match read_linux_memavailable_mib() {
                Ok(value) => (value, None),
                Err(error) => (None, Some(error)),
            }
        } else {
            (None, None)
        };

    NvidiaProbe {
        schema: NVIDIA_PROBE_SCHEMA.to_string(),
        detected: true,
        gpu_index: Some(row.gpu_index),
        gpu_name: Some(row.gpu_name),
        compute_cap: row.compute_cap,
        arch: row.arch,
        driver_cuda_major,
        vram_mib: row.vram_mib,
        unified_memory_mib,
        probe_error: driver_error.or(meminfo_error),
    }
}

impl NvidiaProbe {
    pub fn memory_source(&self) -> MemorySource {
        if self.vram_mib.is_some() {
            MemorySource::NvidiaVram
        } else if self.unified_memory_mib.is_some() {
            MemorySource::SystemAvailable
        } else {
            MemorySource::Unavailable
        }
    }

    fn undetected(probe_error: String) -> Self {
        Self {
            schema: NVIDIA_PROBE_SCHEMA.to_string(),
            detected: false,
            gpu_index: None,
            gpu_name: None,
            compute_cap: None,
            arch: None,
            driver_cuda_major: None,
            vram_mib: None,
            unified_memory_mib: None,
            probe_error: Some(probe_error),
        }
    }
}

pub fn select_local_backend(
    probe: &NvidiaProbe,
    arch_set: &[&str],
    cuda_version: u32,
    trust: ArtifactTrust,
    persisted_installed_cuda: bool,
) -> BackendChoice {
    if let Some(rejection) = hardware_backend_rejection(probe, arch_set, cuda_version) {
        return rejection;
    }

    let arch = probe
        .arch
        .as_deref()
        .expect("hardware rejection checked arch");
    let driver_cuda_major = probe
        .driver_cuda_major
        .expect("hardware rejection checked driver CUDA major");
    let cuda_reason =
        format!("compute_cap {arch} covered; driver CUDA {driver_cuda_major} >= {cuda_version}");
    if trust == ArtifactTrust::Trusted
        || (trust == ArtifactTrust::Unavailable && persisted_installed_cuda)
    {
        return BackendChoice {
            backend: Backend::Cuda,
            reason: cuda_reason,
        };
    }

    let detail = match trust {
        ArtifactTrust::Absent => "CUDA runtime artifact does not cover this GPU",
        ArtifactTrust::Unavailable | ArtifactTrust::Trusted => {
            "CUDA runtime is not installed locally"
        }
    };
    BackendChoice {
        backend: Backend::Vulkan,
        reason: format!("{cuda_reason}; {detail}"),
    }
}

pub fn hardware_backend_rejection(
    probe: &NvidiaProbe,
    arch_set: &[&str],
    cuda_version: u32,
) -> Option<BackendChoice> {
    if !probe.detected {
        return Some(vulkan_choice("no NVIDIA GPU detected"));
    }
    let Some(arch) = probe.arch.as_deref() else {
        return Some(vulkan_choice("NVIDIA compute capability unreadable"));
    };

    if !arch_set
        .iter()
        .any(|candidate| base_arch(candidate) == base_arch(arch))
    {
        return Some(vulkan_choice(format!(
            "compute_cap {arch} not in CUDA image arch set"
        )));
    }

    let Some(driver_cuda_major) = probe.driver_cuda_major else {
        return Some(vulkan_choice("driver CUDA version unreadable"));
    };
    if driver_cuda_major < cuda_version {
        return Some(vulkan_choice(format!(
            "driver CUDA {driver_cuda_major} < required {cuda_version}"
        )));
    }
    None
}

fn vulkan_choice(reason: impl Into<String>) -> BackendChoice {
    BackendChoice {
        backend: Backend::Vulkan,
        reason: reason.into(),
    }
}

fn parse_nvidia_smi_row(line: &str) -> Option<NvidiaSmiRow> {
    let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
    if fields.len() != 5 {
        return None;
    }
    let gpu_index = fields[0].parse().ok()?;
    let gpu_name = fields[1].to_string();
    let compute_cap = (!fields[2].is_empty()).then(|| fields[2].to_string());
    let arch = compute_cap.as_deref().and_then(compute_cap_to_arch);
    let vram_mib = fields[4].parse().ok();
    Some(NvidiaSmiRow {
        gpu_index,
        gpu_name,
        compute_cap,
        arch,
        vram_mib,
    })
}

fn compute_cap_to_arch(compute_cap: &str) -> Option<String> {
    let mut parts = compute_cap.trim().split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    if parts.next().is_some()
        || major.is_empty()
        || minor.is_empty()
        || !major.bytes().all(|byte| byte.is_ascii_digit())
        || !minor.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(format!("sm_{major}{minor}"))
}

fn base_arch(arch: &str) -> &str {
    arch.strip_suffix('a').unwrap_or(arch)
}

fn has_unified_memory_name(gpu_name: &str) -> bool {
    gpu_name.to_ascii_uppercase().contains("GB10")
}

fn probe_driver_cuda_major() -> Result<Option<u32>, String> {
    let output = run_nvidia_smi(&["--version"])?;
    if !output.status.success() {
        return Err(format!(
            "NVIDIA driver CUDA version probe exited with status {}",
            output.status
        ));
    }
    parse_driver_cuda_major(&output.stdout)
        .map(Some)
        .ok_or_else(|| "NVIDIA driver CUDA version probe returned unparseable output".to_string())
}

fn parse_driver_cuda_major(text: &str) -> Option<u32> {
    let (_, after_marker) = text.split_once("CUDA Version")?;
    let after_colon = after_marker.trim_start();
    let version = after_colon
        .strip_prefix(':')
        .unwrap_or(after_colon)
        .trim_start();
    let major = version
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .collect::<Vec<_>>();
    (!major.is_empty())
        .then(|| std::str::from_utf8(&major).ok()?.parse().ok())
        .flatten()
}

fn read_linux_memavailable_mib() -> Result<Option<u64>, String> {
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|error| {
        format!("Linux MemAvailable probe could not read /proc/meminfo: {error}")
    })?;
    Ok(parse_memavailable_mib(&text))
}

fn parse_memavailable_mib(meminfo_text: &str) -> Option<u64> {
    for line in meminfo_text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key != "MemAvailable" {
            continue;
        }
        let mut parts = value.split_whitespace();
        let kib = parts.next()?.parse::<u64>().ok()?;
        if matches!(parts.next(), Some(unit) if unit != "kB") {
            return None;
        }
        return Some(kib / 1024);
    }
    None
}

fn first_nonblank_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

struct CommandOutput {
    status: ExitStatus,
    stdout: String,
}

fn run_nvidia_smi(arguments: &[&str]) -> Result<CommandOutput, String> {
    let mut command = Command::new("nvidia-smi");
    command
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(command_start_error)?;
    let started = Instant::now();
    loop {
        if let Some(_status) = child.try_wait().map_err(command_wait_error)? {
            let output = child.wait_with_output().map_err(command_wait_error)?;
            return Ok(CommandOutput {
                status: output.status,
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            });
        }
        if started.elapsed() >= PROBE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err("NVIDIA GPU probe timed out after 10s".to_string());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn command_start_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        "nvidia-smi binary not found".to_string()
    } else {
        format!("NVIDIA GPU probe could not start: {error}")
    }
}

fn command_wait_error(error: io::Error) -> String {
    format!("NVIDIA GPU probe could not wait for nvidia-smi: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RTX_2060_ROW: &str = "0, NVIDIA GeForce RTX 2060, 7.5, 580.142, 6144\n";
    const GB10_NO_MEMORY_ROW: &str = "0, NVIDIA GB10, 12.1, 580.142, [N/A]\n";
    const MEMINFO: &str = "MemTotal:       127601388 kB\nMemAvailable:   27365908 kB\n";
    const DRIVER_TOO_OLD: &str =
        "NVIDIA-SMI 570.86.15\nDriver Version: 570.86.15\nCUDA Version : 12.0\n";

    fn probe_from_row(
        row_text: &str,
        driver_cuda_major: Option<u32>,
        unified_memory_mib: Option<u64>,
    ) -> NvidiaProbe {
        let row = parse_nvidia_smi_row(row_text.trim()).expect("recorded nvidia-smi row");
        NvidiaProbe {
            schema: NVIDIA_PROBE_SCHEMA.to_string(),
            detected: true,
            gpu_index: Some(row.gpu_index),
            gpu_name: Some(row.gpu_name),
            compute_cap: row.compute_cap,
            arch: row.arch,
            driver_cuda_major,
            vram_mib: row.vram_mib,
            unified_memory_mib,
            probe_error: None,
        }
    }

    #[test]
    fn parses_no_reported_memory_row() {
        let probe = probe_from_row(GB10_NO_MEMORY_ROW, Some(13), None);

        assert_eq!(probe.gpu_name.as_deref(), Some("NVIDIA GB10"));
        assert_eq!(probe.compute_cap.as_deref(), Some("12.1"));
        assert_eq!(probe.arch.as_deref(), Some("sm_121"));
        assert_eq!(probe.vram_mib, None);
    }

    #[test]
    fn parses_unrecognized_architecture_row_and_rejects_it() {
        let probe = probe_from_row(RTX_2060_ROW, Some(13), None);

        assert_eq!(probe.arch.as_deref(), Some("sm_75"));
        assert_eq!(
            hardware_backend_rejection(&probe, &CUDA_EMBEDDED_ARCH_SET, CUDA_MIN_DRIVER_VERSION),
            Some(BackendChoice {
                backend: Backend::Vulkan,
                reason: "compute_cap sm_75 not in CUDA image arch set".to_string(),
            })
        );
    }

    #[test]
    fn parses_gb10_unified_memory_from_meminfo() {
        let memory_mib = parse_memavailable_mib(MEMINFO);
        let probe = probe_from_row(GB10_NO_MEMORY_ROW, Some(13), memory_mib);

        assert_eq!(memory_mib, Some(26_724));
        assert_eq!(probe.unified_memory_mib, Some(26_724));
        assert!(has_unified_memory_name(
            probe.gpu_name.as_deref().expect("GPU name")
        ));
    }

    #[test]
    fn parses_driver_too_old_and_rejects_it() {
        let driver_cuda_major = parse_driver_cuda_major(DRIVER_TOO_OLD);
        let probe = probe_from_row(
            "0, NVIDIA GeForce RTX 4090, 8.9, 580.95.05, 24564\n",
            driver_cuda_major,
            None,
        );

        assert_eq!(driver_cuda_major, Some(12));
        assert_eq!(
            hardware_backend_rejection(&probe, &CUDA_EMBEDDED_ARCH_SET, CUDA_MIN_DRIVER_VERSION),
            Some(BackendChoice {
                backend: Backend::Vulkan,
                reason: "driver CUDA 12 < required 13".to_string(),
            })
        );
    }

    #[test]
    fn covers_the_other_hardware_rejection_reasons() {
        let no_gpu = NvidiaProbe::undetected("nvidia-smi binary not found".to_string());
        assert_eq!(
            hardware_backend_rejection(&no_gpu, &CUDA_EMBEDDED_ARCH_SET, CUDA_MIN_DRIVER_VERSION)
                .expect("no GPU rejection")
                .reason,
            "no NVIDIA GPU detected"
        );

        let unreadable_compute_cap = probe_from_row(
            "0, NVIDIA GeForce RTX 4090, unreadable, 580.95.05, 24564\n",
            Some(13),
            None,
        );
        assert_eq!(
            hardware_backend_rejection(
                &unreadable_compute_cap,
                &CUDA_EMBEDDED_ARCH_SET,
                CUDA_MIN_DRIVER_VERSION
            )
            .expect("compute capability rejection")
            .reason,
            "NVIDIA compute capability unreadable"
        );

        let unreadable_driver = probe_from_row(
            "0, NVIDIA GeForce RTX 4090, 8.9, 580.95.05, 24564\n",
            None,
            None,
        );
        assert_eq!(
            hardware_backend_rejection(
                &unreadable_driver,
                &CUDA_EMBEDDED_ARCH_SET,
                CUDA_MIN_DRIVER_VERSION
            )
            .expect("driver version rejection")
            .reason,
            "driver CUDA version unreadable"
        );
    }

    #[test]
    fn selects_cuda_only_when_hardware_and_artifact_trust_allow_it() {
        let probe = probe_from_row(
            "0, NVIDIA GeForce RTX 4090, 8.9, 580.95.05, 24564\n",
            Some(13),
            None,
        );

        assert_eq!(
            select_local_backend(
                &probe,
                &CUDA_EMBEDDED_ARCH_SET,
                CUDA_MIN_DRIVER_VERSION,
                ArtifactTrust::Trusted,
                false,
            ),
            BackendChoice {
                backend: Backend::Cuda,
                reason: "compute_cap sm_89 covered; driver CUDA 13 >= 13".to_string(),
            }
        );
        assert_eq!(
            select_local_backend(
                &probe,
                &CUDA_EMBEDDED_ARCH_SET,
                CUDA_MIN_DRIVER_VERSION,
                ArtifactTrust::Unavailable,
                false,
            )
            .reason,
            "compute_cap sm_89 covered; driver CUDA 13 >= 13; CUDA runtime is not installed locally"
        );
        assert_eq!(
            select_local_backend(
                &probe,
                &CUDA_EMBEDDED_ARCH_SET,
                CUDA_MIN_DRIVER_VERSION,
                ArtifactTrust::Absent,
                false,
            )
            .reason,
            "compute_cap sm_89 covered; driver CUDA 13 >= 13; CUDA runtime artifact does not cover this GPU"
        );
    }
}
