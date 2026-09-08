// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for RF-DETR helper executables, runtime, and model.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

#[cfg(windows)]
use solstone_core_distribution::windows_payload::{VerifiedWindowsPayload, verify_windows_payload};
#[cfg(windows)]
use std::ffi::OsStr;

pub const WINDOWS_RFDETR_WORKER_MEMBER: &str = "bin/rfdetr-cli.exe";
pub const WINDOWS_RFDETR_MODEL_MEMBER: &str =
    "lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf";

pub const RFDETR_HELP_TIMEOUT: Duration = Duration::from_secs(10);
pub const RFDETR_DETECT_TIMEOUT: Duration = Duration::from_secs(120);
pub const RFDETR_STDIN_LIMIT_BYTES: usize = 1;
pub const RFDETR_STDOUT_LIMIT_BYTES: usize = 64 * 1024;
pub const RFDETR_STDERR_LIMIT_BYTES: usize = 64 * 1024;
pub const RFDETR_COMMITTED_MEMORY_BYTES: usize = 2 * 1024 * 1024 * 1024;
pub const RFDETR_CPU_RATE_PER_10_000: u32 = 10000;
pub const RFDETR_THRESHOLD: &str = "0.25";
pub const RFDETR_THREADS: &str = "4";

pub const RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE: &str = "Object detection is degraded because the signed RF-DETR package assets are unavailable or invalid. Reinstall or repair the application package.";

pub fn rfdetr_degraded_guidance(os_name: &str, arch: &str) -> &'static str {
    if super::rfdetr_install::rfdetr_uses_package_payload(os_name, arch) {
        RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE
    } else {
        super::rfdetr_readiness::RFDETR_UNAVAILABLE_GUIDANCE
    }
}

/// The verified package locations required for Windows RF-DETR operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsRfdetrPackage {
    pub package_root: PathBuf,
    pub binary: PathBuf,
    pub model: PathBuf,
}

/// Resolve RF-DETR only from the complete signed package containing the running
/// journal executable. Re-verified on every call.
#[cfg(windows)]
pub fn verified_windows_rfdetr_package() -> Result<WindowsRfdetrPackage, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not determine the running journal executable: {error}"))?;
    let bin = executable.parent().ok_or_else(|| {
        format!(
            "running journal executable has no containing directory: {}",
            executable.display()
        )
    })?;
    if bin.file_name() != Some(OsStr::new("bin")) {
        return Err(format!(
            "running journal executable is not in the package bin directory: {}",
            executable.display()
        ));
    }
    let package_root = bin.parent().ok_or_else(|| {
        format!(
            "package bin directory has no package root: {}",
            bin.display()
        )
    })?;
    let payload = verify_windows_payload(package_root)
        .map_err(|error| format!("could not verify the signed RF-DETR app payload: {error}"))?;
    declared_rfdetr_members(package_root, &payload)
}

/// Non-Windows callers cannot establish the installed Windows package scope.
#[cfg(not(windows))]
pub fn verified_windows_rfdetr_package() -> Result<WindowsRfdetrPackage, String> {
    Err("Windows RF-DETR package verification requires a Windows runtime".to_owned())
}

#[cfg(windows)]
pub fn declared_rfdetr_members(
    package_root: &Path,
    payload: &VerifiedWindowsPayload,
) -> Result<WindowsRfdetrPackage, String> {
    let binary = payload
        .declared_path(WINDOWS_RFDETR_WORKER_MEMBER)
        .ok_or_else(|| {
            format!("signed RF-DETR app payload does not declare {WINDOWS_RFDETR_WORKER_MEMBER}")
        })?;
    let model = payload
        .declared_path(WINDOWS_RFDETR_MODEL_MEMBER)
        .ok_or_else(|| {
            format!("signed RF-DETR app payload does not declare {WINDOWS_RFDETR_MODEL_MEMBER}")
        })?;
    Ok(WindowsRfdetrPackage {
        package_root: package_root.to_path_buf(),
        binary,
        model,
    })
}

/// Host-independent launch specification for Windows bounded helper invocations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RfdetrWindowsLaunchSpec {
    pub package_root: PathBuf,
    pub executable: PathBuf,
    pub current_directory: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<OsString, OsString>,
    pub stdin: Vec<u8>,
    pub timeout: Duration,
    pub stdin_limit_bytes: usize,
    pub stdout_limit_bytes: usize,
    pub stderr_limit_bytes: usize,
    pub cpu_rate_per_10_000: u32,
    pub committed_memory_bytes: usize,
}

fn resolve_package_bin_directory(package: &WindowsRfdetrPackage) -> PathBuf {
    if let Some(parent) = package
        .binary
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        parent.to_path_buf()
    } else {
        let root_str = package.package_root.to_string_lossy();
        if root_str.contains('\\') {
            PathBuf::from(format!("{root_str}\\bin"))
        } else {
            package.package_root.join("bin")
        }
    }
}

pub fn rfdetr_windows_help_launch(
    package: &WindowsRfdetrPackage,
    system_root: OsString,
) -> RfdetrWindowsLaunchSpec {
    let current_directory = resolve_package_bin_directory(package);
    RfdetrWindowsLaunchSpec {
        package_root: package.package_root.clone(),
        executable: package.binary.clone(),
        current_directory,
        arguments: vec!["--help".to_owned()],
        environment: BTreeMap::from([(OsString::from("SystemRoot"), system_root)]),
        stdin: Vec::new(),
        timeout: RFDETR_HELP_TIMEOUT,
        stdin_limit_bytes: RFDETR_STDIN_LIMIT_BYTES,
        stdout_limit_bytes: RFDETR_STDOUT_LIMIT_BYTES,
        stderr_limit_bytes: RFDETR_STDERR_LIMIT_BYTES,
        cpu_rate_per_10_000: RFDETR_CPU_RATE_PER_10_000,
        committed_memory_bytes: RFDETR_COMMITTED_MEMORY_BYTES,
    }
}

pub fn rfdetr_windows_detect_launch(
    package: &WindowsRfdetrPackage,
    system_root: OsString,
    input: &Path,
    output: &Path,
) -> RfdetrWindowsLaunchSpec {
    let current_directory = resolve_package_bin_directory(package);
    RfdetrWindowsLaunchSpec {
        package_root: package.package_root.clone(),
        executable: package.binary.clone(),
        current_directory,
        arguments: vec![
            "detect".to_owned(),
            "--model".to_owned(),
            package.model.to_string_lossy().into_owned(),
            "--input".to_owned(),
            input.to_string_lossy().into_owned(),
            "--output".to_owned(),
            output.to_string_lossy().into_owned(),
            "--threshold".to_owned(),
            RFDETR_THRESHOLD.to_owned(),
            "--threads".to_owned(),
            RFDETR_THREADS.to_owned(),
        ],
        environment: BTreeMap::from([(OsString::from("SystemRoot"), system_root)]),
        stdin: Vec::new(),
        timeout: RFDETR_DETECT_TIMEOUT,
        stdin_limit_bytes: RFDETR_STDIN_LIMIT_BYTES,
        stdout_limit_bytes: RFDETR_STDOUT_LIMIT_BYTES,
        stderr_limit_bytes: RFDETR_STDERR_LIMIT_BYTES,
        cpu_rate_per_10_000: RFDETR_CPU_RATE_PER_10_000,
        committed_memory_bytes: RFDETR_COMMITTED_MEMORY_BYTES,
    }
}

pub fn map_rfdetr_help_probe(
    runnable: bool,
    exit_code: Option<i32>,
    error_message: Option<&str>,
) -> Value {
    if runnable {
        json!({"runnable": true, "reason_code": Value::Null})
    } else if let Some(code) = exit_code {
        json!({"runnable": false, "reason_code": "binary_exit", "exit_code": code})
    } else if let Some(message) = error_message {
        let reason_code = if message.contains("timed out") || message.contains("DeadlineExceeded") {
            "timeout"
        } else {
            "binary_unavailable"
        };
        json!({"runnable": false, "reason_code": reason_code, "message": message})
    } else {
        json!({"runnable": false, "reason_code": "binary_unavailable"})
    }
}

pub fn map_rfdetr_detect_completion(exit_ok: bool, output_path: &Path) -> Result<Value, String> {
    if !exit_ok {
        return Err("rfdetr-cli detect failed".to_owned());
    }
    let bytes = std::fs::read(output_path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}
