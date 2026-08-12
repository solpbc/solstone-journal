// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `solstone-core install-provider` orchestration.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_cli::InstallProviderOptions;
use solstone_core_journal_config::read_journal_config;
use solstone_core_local::install::{
    DispatchError, InstallVerb, dispatch, fingerprint, fit_report, install_parakeet_with_lease,
    lease, local_backend_choice, pins, readiness, status,
};
use solstone_core_local::{
    LocalEndpointResolution, MemorySource, detect_gpus, discrete_hardware_gpu_count, gpu_probe_ok,
    is_discrete, probe_nvidia_gpu, resolve_local_endpoint, select_device,
};
use solstone_core_system::provider_runtime::decide_parakeet_auto_placement;

use crate::{eprint_journal_path_error, resolve_process_journal_path};

const OBSERVE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const OBSERVE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const OBSERVE_PROGRESS_INTERVAL: Duration = Duration::from_secs(10);
const PARAKEET_DOWNLOAD_DISCLOSURE: &str = "parakeet-cpp fetches two external artifacts into this journal's provider cache before it can run: the parakeet.cpp server binary from github.com (MIT) and the speech model from huggingface.co (CC-BY-4.0).";
const LOCAL_DOWNLOAD_DISCLOSURE: &str = "local model assets: downloading the llama.cpp runtime (MIT; the CUDA build also carries NVIDIA-licensed runtime components) and the model (Apache-2.0) from updates.solstone.app. see THIRD_PARTY_NOTICES.md.";

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallProviderOutcome {
    exit_code: u8,
    stdout: Vec<String>,
    stderr: Vec<String>,
}

impl InstallProviderOutcome {
    fn success(stdout: Vec<String>, stderr: Vec<String>) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr,
        }
    }

    fn failure(exit_code: u8, message: impl Into<String>) -> Self {
        Self {
            exit_code,
            stdout: Vec::new(),
            stderr: vec![message.into()],
        }
    }
}

/// Collect process inputs only; command decisions live in `run_inner`.
pub fn run(options: InstallProviderOptions) -> ExitCode {
    let is_local = options.name == "local";
    let outcome = run_inner(
        options,
        || match resolve_process_journal_path() {
            Ok(journal) => Ok(journal.path),
            Err(error) => {
                eprint_journal_path_error(error);
                Err(())
            }
        },
        move |journal| {
            let input = json!({"journal": journal.display().to_string()})
                .as_object()
                .unwrap()
                .clone();
            if is_local {
                // Darwin's local provider IS MLX. Inspecting the llama.cpp
                // installation there would report on an artifact set that
                // platform never installs.
                if normalized_os(std::env::consts::OS) == "darwin" {
                    readiness::inspect_mlx(input)
                } else {
                    readiness::inspect_local(input)
                }
            } else {
                readiness::inspect_parakeet(input)
            }
        },
        install_parakeet,
        install_local,
    );
    for line in outcome.stdout {
        println!("{line}");
    }
    for line in outcome.stderr {
        eprintln!("{line}");
    }
    ExitCode::from(outcome.exit_code)
}

fn run_inner<J, R, P, L>(
    options: InstallProviderOptions,
    journal_resolver: J,
    readiness_provider: R,
    parakeet_executor: P,
    local_executor: L,
) -> InstallProviderOutcome
where
    J: FnOnce() -> Result<PathBuf, ()>,
    R: FnOnce(&Path) -> Value,
    P: FnOnce(&Path, lease::InstallLease) -> Result<Value, Box<DispatchError>>,
    L: FnOnce(&Path) -> Result<Value, Box<DispatchError>>,
{
    run_inner_with(
        options,
        journal_resolver,
        readiness_provider,
        None,
        parakeet_executor,
        local_executor,
    )
}

fn run_inner_with<J, R, P, L>(
    options: InstallProviderOptions,
    journal_resolver: J,
    readiness_provider: R,
    report_override: Option<fit_report::FitReport>,
    parakeet_executor: P,
    local_executor: L,
) -> InstallProviderOutcome
where
    J: FnOnce() -> Result<PathBuf, ()>,
    R: FnOnce(&Path) -> Value,
    P: FnOnce(&Path, lease::InstallLease) -> Result<Value, Box<DispatchError>>,
    L: FnOnce(&Path) -> Result<Value, Box<DispatchError>>,
{
    match options.name.as_str() {
        "parakeet" => {}
        "local" => {
            return run_local_inner_with_platform(
                journal_resolver,
                readiness_provider,
                report_override,
                local_executor,
                normalized_os(std::env::consts::OS),
                normalized_arch(std::env::consts::ARCH),
            );
        }
        name => {
            return InstallProviderOutcome::failure(
                2,
                format!("unsupported provider {name:?}; supported: local, parakeet"),
            );
        }
    }

    let journal = match journal_resolver() {
        Ok(journal) => journal,
        Err(()) => {
            return InstallProviderOutcome {
                exit_code: 1,
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
        }
    };
    let readiness = readiness_provider(&journal);
    let mut stderr = vec![PARAKEET_DOWNLOAD_DISCLOSURE.to_owned()];
    let readiness_status = readiness["status"].as_str().unwrap_or("proof-unavailable");
    if readiness_status == "ready" {
        let install = match status::read_status(&journal, "parakeet") {
            Ok(install) => install,
            Err(error) => return unavailable_status_error("parakeet", stderr, error),
        };
        stderr.push("parakeet already installed".to_owned());
        return InstallProviderOutcome::success(vec![render_status(&install)], stderr);
    }
    if matches!(readiness_status, "proof-unavailable" | "host-ineligible") {
        stderr.push(
            readiness["reason_code"]
                .as_str()
                .unwrap_or("readiness_unavailable")
                .to_owned(),
        );
        return InstallProviderOutcome {
            exit_code: 1,
            stdout: Vec::new(),
            stderr,
        };
    }

    let target_sha = match parakeet_target_sha(&journal) {
        Ok(target_sha) => target_sha,
        Err(error) => {
            stderr.push(error);
            return InstallProviderOutcome {
                exit_code: 1,
                stdout: Vec::new(),
                stderr,
            };
        }
    };
    let held = match lease::acquire(&journal, "parakeet") {
        Ok(held) => held,
        Err(error) => {
            stderr.push(error.to_string());
            return InstallProviderOutcome {
                exit_code: 1,
                stdout: Vec::new(),
                stderr,
            };
        }
    };
    let Some(held) = held else {
        return observe_existing(&journal, "parakeet", &target_sha, stderr);
    };

    // Unlike install-models, this owner-facing Python-parity command reports
    // all refusal and installation failures as exit 1 rather than sysexits.
    let report = report_override.unwrap_or_else(|| {
        fit_report::build_parakeet_fit_report(
            &journal,
            normalized_os(std::env::consts::OS),
            normalized_arch(std::env::consts::ARCH),
        )
    });
    stderr.push(fit_report::render_fit_report(&report));
    if report.overall() == fit_report::FitSeverity::Blocked {
        return InstallProviderOutcome {
            exit_code: 1,
            stdout: Vec::new(),
            stderr,
        };
    }

    match parakeet_executor(&journal, held) {
        Ok(result) => {
            let install = result["status"].clone();
            // The direct installer reports failure only for a terminal failed
            // status; observation below instead requires installed success.
            let exit_code = u8::from(install["install_state"] == "failed");
            InstallProviderOutcome {
                exit_code,
                stdout: vec![render_value(&install)],
                stderr,
            }
        }
        Err(error) => install_failure(&journal, "parakeet", *error, stderr),
    }
}

fn install_parakeet(
    journal: &Path,
    held: lease::InstallLease,
) -> Result<Value, Box<DispatchError>> {
    install_parakeet_with_lease(
        journal,
        normalized_os(std::env::consts::OS),
        normalized_arch(std::env::consts::ARCH),
        held,
    )
    .map_err(Box::new)
}

fn install_local(journal: &Path) -> Result<Value, Box<DispatchError>> {
    // On Darwin the local provider is an MLX snapshot, so this dispatches RunMlx
    // with no source_snapshot -- which is what makes the install fetch from sol
    // pbc's own origin rather than from a model hub.
    let (verb, payload) = if normalized_os(std::env::consts::OS) == "darwin" {
        (
            InstallVerb::RunMlx,
            json!({
                "journal": journal.display().to_string(),
                "model_id": pins::MLX_MODELS[0].0,
            }),
        )
    } else {
        (
            InstallVerb::RunLocal,
            json!({"journal": journal.display().to_string()}),
        )
    };
    let envelope = dispatch(verb, payload).map_err(Box::new)?;
    Ok(envelope
        .result
        .expect("successful install dispatch has a result"))
}

fn run_local_inner_with_platform<J, R, L>(
    journal_resolver: J,
    readiness_provider: R,
    report_override: Option<fit_report::FitReport>,
    install_executor: L,
    os_name: &str,
    arch: &str,
) -> InstallProviderOutcome
where
    J: FnOnce() -> Result<PathBuf, ()>,
    R: FnOnce(&Path) -> Value,
    L: FnOnce(&Path) -> Result<Value, Box<DispatchError>>,
{
    run_local_inner_with_platform_and_target_sha(
        journal_resolver,
        readiness_provider,
        report_override,
        install_executor,
        local_target_sha,
        os_name,
        arch,
    )
}

fn run_local_inner_with_platform_and_target_sha<J, R, L, T>(
    journal_resolver: J,
    readiness_provider: R,
    report_override: Option<fit_report::FitReport>,
    install_executor: L,
    target_sha_provider: T,
    os_name: &str,
    arch: &str,
) -> InstallProviderOutcome
where
    J: FnOnce() -> Result<PathBuf, ()>,
    R: FnOnce(&Path) -> Value,
    L: FnOnce(&Path) -> Result<Value, Box<DispatchError>>,
    T: Fn(&Path) -> Result<String, String>,
{
    let journal = match journal_resolver() {
        Ok(journal) => journal,
        Err(()) => {
            return InstallProviderOutcome {
                exit_code: 1,
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
        }
    };
    let readiness = readiness_provider(&journal);
    let mut stderr = vec![LOCAL_DOWNLOAD_DISCLOSURE.to_owned()];
    let readiness_status = readiness["status"].as_str().unwrap_or("proof-unavailable");
    if readiness_status == "ready" {
        let install = match status::read_status(&journal, "local") {
            Ok(install) => install,
            Err(error) => return unavailable_status_error("local", stderr, error),
        };
        stderr.push("local already installed".to_owned());
        return InstallProviderOutcome::success(vec![render_status(&install)], stderr);
    }
    if matches!(readiness_status, "proof-unavailable" | "host-ineligible") {
        stderr.push(
            readiness["reason_code"]
                .as_str()
                .unwrap_or("readiness_unavailable")
                .to_owned(),
        );
        return InstallProviderOutcome {
            exit_code: 1,
            stdout: Vec::new(),
            stderr,
        };
    }
    let target_sha = if report_override.is_some() {
        // The report override is a synthetic test seam, so it must not inspect
        // live local-backend facts merely to build the attempt fingerprint.
        None
    } else {
        match target_sha_provider(&journal) {
            Ok(target_sha) => Some(target_sha),
            Err(error) => {
                stderr.push(error);
                return InstallProviderOutcome {
                    exit_code: 1,
                    stdout: Vec::new(),
                    stderr,
                };
            }
        }
    };
    match lease::is_held(&journal, "local") {
        Ok(true) => {
            let target_sha = match target_sha {
                Some(target_sha) => target_sha,
                None => match target_sha_provider(&journal) {
                    Ok(target_sha) => target_sha,
                    Err(error) => {
                        stderr.push(error);
                        return InstallProviderOutcome {
                            exit_code: 1,
                            stdout: Vec::new(),
                            stderr,
                        };
                    }
                },
            };
            return observe_existing(&journal, "local", &target_sha, stderr);
        }
        Ok(false) => {}
        Err(error) => {
            stderr.push(error.to_string());
            return InstallProviderOutcome {
                exit_code: 1,
                stdout: Vec::new(),
                stderr,
            };
        }
    }
    let report = match report_override {
        Some(report) => report,
        None => match build_platform_report(&journal, os_name, arch) {
            Ok(report) => report,
            Err(error) => {
                stderr.push(error);
                return InstallProviderOutcome {
                    exit_code: 1,
                    stdout: Vec::new(),
                    stderr,
                };
            }
        },
    };
    stderr.push(fit_report::render_fit_report(&report));
    match install_executor(&journal) {
        Ok(result) => {
            let install = result["status"].clone();
            let exit_code = u8::from(install["install_state"] == "failed");
            InstallProviderOutcome {
                exit_code,
                stdout: vec![render_value(&install)],
                stderr,
            }
        }
        Err(error) if is_install_busy(&error) => {
            let target_sha = match target_sha {
                Some(target_sha) => target_sha,
                None => match target_sha_provider(&journal) {
                    Ok(target_sha) => target_sha,
                    Err(error) => {
                        stderr.push(error);
                        return InstallProviderOutcome {
                            exit_code: 1,
                            stdout: Vec::new(),
                            stderr,
                        };
                    }
                },
            };
            observe_existing(&journal, "local", &target_sha, stderr)
        }
        Err(error) => install_failure(&journal, "local", *error, stderr),
    }
}

/// Darwin's local provider is MLX; every other platform's is llama.cpp. Routing
/// here rather than at the call site keeps the one skeleton for both, which is
/// what makes the Darwin path testable by the same criteria as the Linux one.
fn build_platform_report(
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> Result<fit_report::FitReport, String> {
    if os_name == "darwin" {
        return Ok(fit_report::build_mlx_fit_report(
            pins::MLX_MODELS[0].0,
            os_name,
            arch,
            &pins::cache_root(journal),
            fit_report::free_bytes(&pins::cache_root(journal)),
            available_memory_bytes(),
            // Native cannot answer this without an interpreter, and asking one
            // would defeat the point of the native path. It degrades to Unknown.
            None,
        ));
    }
    build_local_report(journal, os_name, arch)
}

fn build_local_report(
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> Result<fit_report::FitReport, String> {
    let nvidia_probe = probe_nvidia_gpu();
    let devices = detect_gpus();
    let (override_index, brain_lane_active) =
        local_override_and_brain_lane(journal).map_err(|error| error.to_string())?;
    let selected = select_device(&devices, override_index);
    let unified_memory = nvidia_probe.memory_source() == MemorySource::SystemAvailable;
    let force_cpu = decide_parakeet_auto_placement(
        selected
            .as_ref()
            .and_then(|device| u32::try_from(device.vram_mib).ok()),
        selected.as_ref().is_some_and(is_discrete),
        discrete_hardware_gpu_count(&devices),
        unified_memory,
        brain_lane_active,
    )
    .force_cpu;
    let choice = local_backend_choice(journal, Some(nvidia_probe.clone()));
    Ok(fit_report::build_local_fit_report(
        journal,
        "local/qwen3.5-4b",
        os_name,
        arch,
        fit_report::free_bytes(&pins::cache_root(journal)),
        available_memory_bytes(),
        &nvidia_probe,
        &choice,
        gpu_probe_ok(),
        &devices,
        override_index,
        force_cpu,
    ))
}

fn local_override_and_brain_lane(
    journal: &Path,
) -> Result<(Option<u32>, bool), solstone_core_journal_config::ConfigLoadError> {
    let brain_lane_active = match read_journal_config(journal) {
        Ok(read) => {
            let config = read.config.unwrap_or_default();
            let local_needed = config
                .get("providers")
                .and_then(Value::as_object)
                .and_then(|providers| providers.get("active"))
                .and_then(Value::as_object)
                .and_then(|active| active.get("provider"))
                .and_then(Value::as_str)
                == Some("local");
            let bundled = matches!(
                resolve_local_endpoint(&config),
                LocalEndpointResolution::Bundled
            );
            local_needed && bundled
        }
        Err(_) => {
            // Python's `brain_lane_active` predicate uses `except Exception:
            // True`; keep that parity fallback only for this predicate.
            true
        }
    };
    let config = read_journal_config(journal)?.config.unwrap_or_default();
    let override_index = config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("local"))
        .and_then(Value::as_object)
        .and_then(|local| local.get("vulkan_device_index"))
        .and_then(local_vulkan_device_index);
    Ok((override_index, brain_lane_active))
}

fn local_vulkan_device_index(value: &Value) -> Option<u32> {
    match value {
        Value::Bool(value) => Some(u32::from(*value)),
        Value::String(value) => value
            .trim()
            .parse::<i64>()
            .ok()
            .and_then(|index| u64::try_from(index).ok())
            .map(saturating_vulkan_device_index),
        Value::Number(value) => {
            if let Some(index) = value.as_i64() {
                return u64::try_from(index)
                    .ok()
                    .map(saturating_vulkan_device_index);
            }
            if let Some(index) = value.as_u64() {
                return Some(saturating_vulkan_device_index(index));
            }
            let index = value.as_f64()?.trunc();
            if !index.is_finite() || index < 0.0 {
                return None;
            }
            Some(saturating_vulkan_device_index(index as u64))
        }
        _ => None,
    }
}

fn saturating_vulkan_device_index(index: u64) -> u32 {
    // No real device index is u32::MAX, so this preserves an explicit nonmatching override.
    u32::try_from(index).unwrap_or(u32::MAX)
}

fn available_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let memory = std::fs::read_to_string("/proc/meminfo").ok()?;
        let mut available = None;
        let mut total = None;
        for line in memory.lines() {
            let mut fields = line.split_whitespace();
            match (fields.next(), fields.next()) {
                (Some("MemAvailable:"), Some(value)) => available = value.parse::<u64>().ok(),
                (Some("MemTotal:"), Some(value)) => total = value.parse::<u64>().ok(),
                _ => {}
            }
        }
        let available = available?.checked_mul(1024)?;
        let total = total?.checked_mul(1024)?;
        (available > 0 && total > 0 && available <= total).then_some(available)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn parakeet_target_sha(journal: &Path) -> Result<String, String> {
    let target = solstone_core_local::install::parakeet_target_for_platform(
        journal,
        normalized_os(std::env::consts::OS),
        normalized_arch(std::env::consts::ARCH),
    )
    .map_err(dispatch_message)?;
    let text = fingerprint::canonical(target).map_err(|error| error.to_string())?;
    Ok(fingerprint::sha256(&text))
}

fn local_target_sha(journal: &Path) -> Result<String, String> {
    let envelope = dispatch(
        InstallVerb::FingerprintLocal,
        json!({"journal": journal.display().to_string(), "model_id": "local/qwen3.5-4b"}),
    )
    .map_err(dispatch_message)?;
    envelope
        .result
        .and_then(|result| {
            result
                .get("target_fingerprint_sha256")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| "local fingerprint dispatch omitted target_fingerprint_sha256".to_owned())
}

fn observe_existing(
    journal: &Path,
    provider: &str,
    target_sha: &str,
    stderr: Vec<String>,
) -> InstallProviderOutcome {
    observe_existing_with(
        journal,
        provider,
        target_sha,
        stderr,
        OBSERVE_POLL_INTERVAL,
        OBSERVE_TIMEOUT,
        OBSERVE_PROGRESS_INTERVAL,
        |state, stderr| {
            stderr.push(progress_line(provider, state));
        },
    )
}

#[allow(clippy::too_many_arguments)] // Provider parameterization plus the injected poll/timeout test seam.
fn observe_existing_with<P>(
    journal: &Path,
    provider: &str,
    target_sha: &str,
    mut stderr: Vec<String>,
    poll_interval: Duration,
    timeout: Duration,
    progress_interval: Duration,
    mut progress: P,
) -> InstallProviderOutcome
where
    P: FnMut(&status::InstallStatus, &mut Vec<String>),
{
    let current = match status::read_status(journal, provider) {
        Ok(status) => status,
        Err(error) => return unavailable_status_error(provider, stderr, error),
    };
    if !status::is_in_flight(&current.install_state)
        || current.target_fingerprint_sha256.as_deref() != Some(target_sha)
    {
        stderr.push(format!(
            "{provider} install already running for a different target"
        ));
        return InstallProviderOutcome {
            exit_code: 1,
            stdout: Vec::new(),
            stderr,
        };
    }
    match status::observe_attempt(
        journal,
        provider,
        target_sha,
        poll_interval,
        timeout,
        progress_interval,
        |state| progress(state, &mut stderr),
    ) {
        Ok(status::ObserveAttempt::Terminal(status))
        | Ok(status::ObserveAttempt::DifferentTarget(status)) => {
            // Python observation succeeds only after an installed terminal
            // record, even if a direct result reports an in-flight state.
            let exit_code = u8::from(status.install_state != "installed");
            InstallProviderOutcome {
                exit_code,
                stdout: vec![render_status(&status)],
                stderr,
            }
        }
        Ok(status::ObserveAttempt::TimedOut) => {
            stderr.push(format!("timed out observing {provider} install"));
            InstallProviderOutcome {
                exit_code: 1,
                stdout: Vec::new(),
                stderr,
            }
        }
        Err(error) => unavailable_status_error(provider, stderr, error),
    }
}

fn progress_line(provider: &str, state: &status::InstallStatus) -> String {
    let suffix = state
        .progress_bytes_received
        .map(|received| match state.progress_bytes_total {
            Some(total) => format!(" {received}/{total}"),
            None => format!(" {received}"),
        })
        .unwrap_or_default();
    format!(
        "observing {provider} install: {}{suffix}",
        state.install_state
    )
}

fn install_failure(
    journal: &Path,
    provider: &str,
    error: DispatchError,
    mut stderr: Vec<String>,
) -> InstallProviderOutcome {
    stderr.push(dispatch_message(error));
    match status::read_status(journal, provider) {
        Ok(status) => InstallProviderOutcome {
            exit_code: 1,
            stdout: vec![render_status(&status)],
            stderr,
        },
        Err(error) => unavailable_status_error(provider, stderr, error),
    }
}

fn unavailable_status_error(
    provider: &str,
    error_lines: Vec<String>,
    error: impl ToString,
) -> InstallProviderOutcome {
    let mut stderr = error_lines;
    stderr.push(format!(
        "could not read persisted {provider} install status: {}",
        error.to_string()
    ));
    InstallProviderOutcome {
        exit_code: 1,
        stdout: Vec::new(),
        stderr,
    }
}

fn is_install_busy(error: &DispatchError) -> bool {
    error
        .envelope
        .error
        .as_ref()
        .is_some_and(|error| error.reason_code == "install_busy")
}

fn dispatch_message(error: DispatchError) -> String {
    error
        .envelope
        .error
        .as_ref()
        .map(|error| error.message.clone())
        .unwrap_or_else(|| "parakeet install failed".to_owned())
}

fn render_status(status: &status::InstallStatus) -> String {
    render_value(&serde_json::to_value(status).expect("install status serializes"))
}

fn render_value(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON value serializes")
}

fn normalized_os(value: &str) -> &str {
    if value == "macos" { "darwin" } else { value }
}

fn normalized_arch(value: &str) -> &str {
    if value == "aarch64" { "arm64" } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn options(name: &str) -> InstallProviderOptions {
        InstallProviderOptions {
            name: name.to_owned(),
        }
    }

    fn missing_readiness() -> Value {
        json!({"status":"missing-or-mismatched","reason_code":"manifest_missing"})
    }

    fn status_value(state: &str) -> Value {
        let mut status = status::idle_status("parakeet");
        status.install_state = state.to_owned();
        serde_json::to_value(status).unwrap()
    }

    fn stage_ready_parakeet(journal: &Path) -> (PathBuf, PathBuf) {
        use solstone_core_local::install::{manifest, pins};

        let key = pins::parakeet_artifact_key(
            normalized_os(std::env::consts::OS),
            normalized_arch(std::env::consts::ARCH),
        )
        .unwrap();
        let paths = pins::parakeet_paths(journal, &key);
        let cpu_path = PathBuf::from(paths["binary_path_cpu"].as_str().unwrap());
        let vulkan_path = PathBuf::from(paths["binary_path_vulkan"].as_str().unwrap());
        let model_path = PathBuf::from(paths["model_path"].as_str().unwrap());
        for path in [&cpu_path, &vulkan_path, &model_path] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        fs::write(&cpu_path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::write(&vulkan_path, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::write(&model_path, b"parakeet model").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            for path in [&cpu_path, &vulkan_path] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        for (root, unit, identity, inventory) in [
            (
                cpu_path.parent().unwrap(),
                "parakeet-server",
                pins::parakeet_backend_identity(&key, "cpu").unwrap(),
                manifest::runtime_inventory(cpu_path.parent().unwrap(), &[]).unwrap(),
            ),
            (
                vulkan_path.parent().unwrap(),
                "parakeet-server",
                pins::parakeet_backend_identity(&key, "vulkan").unwrap(),
                manifest::runtime_inventory(vulkan_path.parent().unwrap(), &[]).unwrap(),
            ),
            (
                model_path.parent().unwrap(),
                "parakeet-model",
                pins::parakeet_model_identity(),
                manifest::inventory_for_tree(model_path.parent().unwrap(), "model").unwrap(),
            ),
        ] {
            let manifest = manifest::build_manifest(
                "parakeet",
                unit,
                "target",
                json!({"pin_identity": identity}),
                inventory,
                None,
                None,
            )
            .unwrap();
            manifest::write_manifest(&manifest::artifact_manifest_path(root), &manifest).unwrap();
        }
        let cpu_manifest = manifest::artifact_manifest_path(cpu_path.parent().unwrap());
        (cpu_path, cpu_manifest)
    }

    fn report(severity: fit_report::FitSeverity) -> fit_report::FitReport {
        fit_report::FitReport {
            artifact: "test".to_string(),
            checks: vec![fit_report::FitCheck {
                name: "test",
                severity,
                detail: "detail".to_owned(),
            }],
        }
    }

    #[test]
    fn ac1_fit_exit_surface_and_warning_continues() {
        let journal = tempfile::tempdir().unwrap();
        let blocked = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Blocked)),
            |_, _| panic!("blocked must not install"),
            |_| panic!("local must not install"),
        );
        assert_eq!(blocked.exit_code, 1);
        assert!(
            blocked
                .stderr
                .iter()
                .any(|line| line.contains("test fit check: blocked"))
        );

        let warning = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Warning)),
            |_, _| Ok(json!({"status": status_value("installed")})),
            |_| panic!("local must not install"),
        );
        assert_eq!(warning.exit_code, 0);
        assert!(
            warning
                .stderr
                .iter()
                .any(|line| line.contains("test fit check: warning"))
        );
    }

    #[test]
    fn ac2_direct_and_observed_status_exit_predicates_diverge_for_in_flight_status() {
        let journal = tempfile::tempdir().unwrap();
        let direct = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Ok)),
            |_, _| Ok(json!({"status": status_value("downloading")})),
            |_| panic!("local must not install"),
        );
        assert_eq!(direct.exit_code, 0);

        let target_sha = parakeet_target_sha(journal.path()).unwrap();
        let mut current = status::idle_status("parakeet");
        current.target_fingerprint_sha256 = Some(target_sha.clone());
        let current = status::transition(current, "resolving", None, None).unwrap();
        status::write_status(journal.path(), current.clone()).unwrap();
        let observed = observe_existing_with(
            journal.path(),
            "parakeet",
            &target_sha,
            Vec::new(),
            Duration::ZERO,
            OBSERVE_TIMEOUT,
            OBSERVE_PROGRESS_INTERVAL,
            |state, _| {
                let mut next = state.clone();
                next.target_fingerprint_sha256 = Some("different".to_owned());
                next.install_state = "downloading".to_owned();
                status::write_status(journal.path(), next).unwrap();
            },
        );
        assert_eq!(observed.exit_code, 1);
        assert!(!observed.stdout.is_empty(), "{observed:?}");
        assert_eq!(
            serde_json::from_str::<Value>(&observed.stdout[0]).unwrap()["install_state"],
            "downloading"
        );
    }

    #[test]
    fn ac2_direct_failed_status_exits_one() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Ok)),
            |_, _| Ok(json!({"status": status_value("failed")})),
            |_| panic!("local must not install"),
        );
        assert_eq!(outcome.exit_code, 1);
    }

    #[test]
    fn ac2_observed_terminal_installed_exits_zero_with_status() {
        let journal = tempfile::tempdir().unwrap();
        let target_sha = parakeet_target_sha(journal.path()).unwrap();
        let mut current = status::idle_status("parakeet");
        current.target_fingerprint_sha256 = Some(target_sha.clone());
        let current = status::transition(current, "resolving", None, None).unwrap();
        status::write_status(journal.path(), current).unwrap();

        let outcome = observe_existing_with(
            journal.path(),
            "parakeet",
            &target_sha,
            Vec::new(),
            Duration::ZERO,
            OBSERVE_TIMEOUT,
            OBSERVE_PROGRESS_INTERVAL,
            |state, _| {
                let installed = status::transition(state.clone(), "installed", None, None).unwrap();
                status::write_status(journal.path(), installed).unwrap();
            },
        );
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            serde_json::from_str::<Value>(&outcome.stdout[0]).unwrap()["install_state"],
            "installed"
        );
    }

    #[test]
    fn ac2_observed_timeout_exits_one() {
        let journal = tempfile::tempdir().unwrap();
        let target_sha = parakeet_target_sha(journal.path()).unwrap();
        let mut current = status::idle_status("parakeet");
        current.target_fingerprint_sha256 = Some(target_sha.clone());
        let current = status::transition(current, "resolving", None, None).unwrap();
        status::write_status(journal.path(), current).unwrap();

        let outcome = observe_existing_with(
            journal.path(),
            "parakeet",
            &target_sha,
            Vec::new(),
            Duration::ZERO,
            Duration::ZERO,
            OBSERVE_PROGRESS_INTERVAL,
            |_, _| {},
        );
        assert_eq!(outcome.exit_code, 1);
        assert!(outcome.stdout.is_empty());
        assert!(
            outcome
                .stderr
                .iter()
                .any(|line| line == "timed out observing parakeet install")
        );
    }

    #[test]
    fn ac3_delegating_and_preheld_lease_do_not_mint_attempts() {
        let journal = tempfile::tempdir().unwrap();
        assert_eq!(
            status::read_status(journal.path(), "parakeet")
                .unwrap()
                .attempt_id,
            None
        );
        let executor_entered = Arc::new(AtomicBool::new(false));
        let entered = executor_entered.clone();
        let direct = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Ok)),
            move |path, _| {
                assert_eq!(
                    status::read_status(path, "parakeet").unwrap().attempt_id,
                    None
                );
                entered.store(true, Ordering::SeqCst);
                Ok(json!({"status": status_value("installed")}))
            },
            |_| panic!("local must not install"),
        );
        assert_eq!(direct.exit_code, 0);
        assert!(executor_entered.load(Ordering::SeqCst));
        assert_eq!(
            status::read_status(journal.path(), "parakeet")
                .unwrap()
                .attempt_id,
            None
        );

        let target_sha = parakeet_target_sha(journal.path()).unwrap();
        let mut current = status::idle_status("parakeet");
        current.target_fingerprint_sha256 = Some(target_sha);
        let current = status::transition(current, "failed", Some("done".to_owned()), None).unwrap();
        status::write_status(journal.path(), current.clone()).unwrap();
        let held = lease::acquire(journal.path(), "parakeet").unwrap().unwrap();
        let observed = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Ok)),
            |_, _| panic!("held lease must not delegate"),
            |_| panic!("local must not install"),
        );
        drop(held);
        assert_eq!(observed.exit_code, 1);
        assert_eq!(
            status::read_status(journal.path(), "parakeet")
                .unwrap()
                .attempt_id,
            current.attempt_id
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ac4_ready_short_circuits_and_unsound_readiness_does_not() {
        let journal = tempfile::tempdir().unwrap();
        let (cpu_path, cpu_manifest) = stage_ready_parakeet(journal.path());
        let binary_before = fs::metadata(&cpu_path).unwrap();
        let manifest_before = fs::metadata(&cpu_manifest).unwrap();
        let outcome = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |path| {
                readiness::inspect_parakeet(
                    json!({"journal": path.display().to_string()})
                        .as_object()
                        .unwrap()
                        .clone(),
                )
            },
            Some(report(fit_report::FitSeverity::Ok)),
            |_, _| panic!("ready must not install"),
            |_| panic!("local must not install"),
        );
        let binary_after = fs::metadata(&cpu_path).unwrap();
        let manifest_after = fs::metadata(&cpu_manifest).unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            binary_before.modified().unwrap(),
            binary_after.modified().unwrap()
        );
        assert_eq!(
            manifest_before.modified().unwrap(),
            manifest_after.modified().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            assert_eq!(binary_before.ino(), binary_after.ino());
            assert_eq!(manifest_before.ino(), manifest_after.ino());
        }
        assert_no_temporary_files(&solstone_core_local::install::pins::parakeet_cache_root(
            journal.path(),
        ));

        fs::write(&cpu_path, b"not a runnable binary").unwrap();
        let unsound = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |path| {
                readiness::inspect_parakeet(
                    json!({"journal": path.display().to_string()})
                        .as_object()
                        .unwrap()
                        .clone(),
                )
            },
            Some(report(fit_report::FitSeverity::Blocked)),
            |_, _| panic!("blocked report proves non-ready reached install path"),
            |_| panic!("local must not install"),
        );
        assert_eq!(unsound.exit_code, 1);
        assert!(unsound.stderr.iter().any(|line| line.contains("fit check")));
    }

    #[test]
    fn ac5_unavailable_readiness_refuses_without_executor() {
        for (state, reason) in [
            ("proof-unavailable", "status_unavailable"),
            ("proof-unavailable", "lease_unavailable"),
            ("proof-unavailable", "manifest_io_error"),
            ("host-ineligible", "binary_exit"),
        ] {
            let journal = tempfile::tempdir().unwrap();
            let entered = Arc::new(AtomicBool::new(false));
            let called = entered.clone();
            let outcome = run_inner_with(
                options("parakeet"),
                || Ok(journal.path().to_path_buf()),
                |_| json!({"status":state,"reason_code":reason}),
                Some(report(fit_report::FitSeverity::Ok)),
                move |_, _| {
                    called.store(true, Ordering::SeqCst);
                    panic!("unavailable must not install")
                },
                |_| panic!("local must not install"),
            );
            assert_eq!(outcome.exit_code, 1, "{reason}");
            assert!(outcome.stderr.iter().any(|line| line == reason));
            assert!(!entered.load(Ordering::SeqCst));
            assert!(
                !solstone_core_local::install::pins::parakeet_cache_root(journal.path()).exists()
            );
        }
    }

    #[test]
    fn local_blocked_report_still_reaches_executor() {
        let journal = tempfile::tempdir().unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let called = entered.clone();
        let local = run_inner_with(
            options("local"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(fit_report::FitReport {
                artifact: "local provider artifacts".to_string(),
                checks: vec![fit_report::FitCheck {
                    name: "disk",
                    severity: fit_report::FitSeverity::Blocked,
                    detail: "too small".to_owned(),
                }],
            }),
            |_, _| panic!("parakeet executor must not run"),
            move |_| {
                called.store(true, Ordering::SeqCst);
                Ok(json!({"status": status_value("installed")}))
            },
        );
        assert_eq!(local.exit_code, 0);
        assert_eq!(
            local.stderr[1].lines().next(),
            Some("local provider artifacts fit check: blocked")
        );
        assert!(entered.load(Ordering::SeqCst));
    }

    #[test]
    fn local_failure_reads_persisted_status_or_names_local_status_error() {
        let journal = tempfile::tempdir().unwrap();
        let persisted = run_inner_with(
            options("local"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Ok)),
            |_, _| panic!("parakeet executor must not run"),
            |_| Err(dispatch_error("download failed")),
        );
        assert_eq!(persisted.exit_code, 1);
        assert_eq!(
            serde_json::from_str::<Value>(&persisted.stdout[0]).unwrap()["provider"],
            "local"
        );
        assert!(
            persisted
                .stderr
                .iter()
                .any(|line| line == "download failed")
        );

        fs::create_dir_all(
            status::status_path(journal.path(), "local")
                .parent()
                .unwrap(),
        )
        .unwrap();
        fs::write(status::status_path(journal.path(), "local"), b"not-json").unwrap();
        let unreadable = run_inner_with(
            options("local"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Ok)),
            |_, _| panic!("parakeet executor must not run"),
            |_| Err(dispatch_error("download failed")),
        );
        assert_eq!(unreadable.exit_code, 1);
        assert!(unreadable.stdout.is_empty());
        assert!(
            unreadable
                .stderr
                .iter()
                .any(|line| { line.starts_with("could not read persisted local install status:") })
        );
    }

    #[test]
    fn local_install_busy_observes_the_existing_local_attempt() {
        let journal = tempfile::tempdir().unwrap();
        let target_sha = "local-target".to_owned();
        let mut current = status::idle_status("local");
        current.target_fingerprint_sha256 = Some(target_sha.clone());
        let current = status::transition(current, "resolving", None, None).unwrap();
        status::write_status(journal.path(), current).unwrap();
        let status_path = journal.path().to_path_buf();
        assert!(is_install_busy(&install_busy_error()));
        let updater = std::thread::spawn(move || {
            let current = status::read_status(&status_path, "local").unwrap();
            let installed = status::transition(current, "installed", None, None).unwrap();
            status::write_status(&status_path, installed).unwrap();
        });

        let outcome = observe_existing_with(
            journal.path(),
            "local",
            &target_sha,
            Vec::new(),
            Duration::from_millis(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            |_, _| {},
        );
        updater.join().unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            serde_json::from_str::<Value>(&outcome.stdout[0]).unwrap()["install_state"],
            "installed"
        );
    }

    #[test]
    fn local_preheld_lease_observes_the_existing_local_attempt() {
        let journal = tempfile::tempdir().unwrap();
        let target_sha = "local-target";
        let mut current = status::idle_status("local");
        current.target_fingerprint_sha256 = Some(target_sha.to_owned());
        let current = status::transition(current, "resolving", None, None).unwrap();
        status::write_status(journal.path(), current).unwrap();
        let status_path = journal.path().to_path_buf();
        let held = lease::acquire(journal.path(), "local").unwrap().unwrap();
        let updater = std::thread::spawn(move || {
            let current = status::read_status(&status_path, "local").unwrap();
            let installed = status::transition(current, "installed", None, None).unwrap();
            status::write_status(&status_path, installed).unwrap();
        });

        let outcome = observe_existing_with(
            journal.path(),
            "local",
            target_sha,
            Vec::new(),
            Duration::from_millis(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            |_, _| {},
        );
        updater.join().unwrap();
        drop(held);
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            serde_json::from_str::<Value>(&outcome.stdout[0]).unwrap()["install_state"],
            "installed"
        );
    }

    #[test]
    fn local_override_accepts_python_integer_strings_but_corrupt_config_surfaces() {
        let configured = json!({"providers":{"local":{"vulkan_device_index":" 2 "}}});
        let value = configured["providers"]["local"]["vulkan_device_index"].clone();
        assert_eq!(local_vulkan_device_index(&value), Some(2));
        assert_eq!(local_vulkan_device_index(&json!("-0")), Some(0));
        assert_eq!(local_vulkan_device_index(&json!(true)), Some(1));
        assert_eq!(local_vulkan_device_index(&json!(2.0)), Some(2));
        assert_eq!(local_vulkan_device_index(&json!(-0.5)), Some(0));
        assert_eq!(
            local_vulkan_device_index(&json!(u64::from(u32::MAX) + 1)),
            Some(u32::MAX)
        );
        assert_eq!(
            local_vulkan_device_index(&json!("4294967296")),
            Some(u32::MAX)
        );
        assert_eq!(local_vulkan_device_index(&json!(-1)), None);
        assert_eq!(local_vulkan_device_index(&json!("not-an-index")), None);

        let journal = tempfile::tempdir().unwrap();
        let config = journal.path().join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("journal.json"), b"not-json").unwrap();
        // The Python-parity fallback applies only to the brain-lane predicate;
        // the separately read override must still surface corrupt settings.
        assert!(local_override_and_brain_lane(journal.path()).is_err());
    }

    #[test]
    fn local_darwin_reaches_the_mlx_installer_instead_of_refusing() {
        // This asserted the opposite until 2026-08-12: native local refused on
        // Darwin, on the stated grounds that "the native pool has no mac host".
        // That was a claim about the hopper POOL, not about the platform -- the
        // refusal made Python the only way to install a local provider on a mac,
        // and Python's route calls huggingface_hub.snapshot_download.
        let journal = tempfile::tempdir().unwrap();
        let installed = std::cell::Cell::new(false);
        let local = run_local_inner_with_platform(
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            None,
            |_| {
                installed.set(true);
                Ok(json!({"status": {"install_state": "installed"}}))
            },
            "darwin",
            "arm64",
        );
        assert!(installed.get(), "Darwin must reach the installer");
        assert_eq!(local.exit_code, 0);
        // ⛔ The refusal string must not survive anywhere in the output. Asserting
        // only on exit code would pass if the refusal moved to a later branch.
        assert!(
            !local
                .stderr
                .iter()
                .any(|line| line.contains("unavailable on Darwin")),
            "{:?}",
            local.stderr
        );
    }

    #[test]
    fn darwin_fit_report_names_the_mlx_model_and_linux_does_not() {
        // The report builder is the seam that decides which artifact set an owner
        // is told about. If Darwin fell back to the llama.cpp builder it would
        // report on artifacts that platform never installs -- and the exit code
        // would look identical.
        let journal = tempfile::tempdir().unwrap();
        let darwin =
            build_platform_report(journal.path(), "darwin", "arm64").expect("darwin report builds");
        assert!(
            darwin.artifact.contains("MLX"),
            "darwin artifact: {}",
            darwin.artifact
        );
        let linux =
            build_platform_report(journal.path(), "linux", "x86_64").expect("linux report builds");
        assert!(
            !linux.artifact.contains("MLX"),
            "linux artifact: {}",
            linux.artifact
        );
    }

    #[test]
    fn local_inactive_brain_lane_cannot_add_the_placement_suffix() {
        // Keep this composition-level check: copying the -check hardcoded
        // `true` would make the placement suffix incorrectly reappear.
        let force_cpu = decide_parakeet_auto_placement(Some(6144), true, 1, false, false).force_cpu;
        assert!(!force_cpu);
        let report = fit_report::build_local_fit_report(
            Path::new("/journal"),
            "local/qwen3.5-4b",
            "linux",
            "x86_64",
            Ok(20_u64 * 1024 * 1024 * 1024),
            Some(8_u64 * 1024 * 1024 * 1024),
            &solstone_core_local::NvidiaProbe {
                schema: "test".to_owned(),
                detected: true,
                gpu_index: None,
                gpu_name: None,
                compute_cap: None,
                arch: None,
                driver_cuda_major: None,
                vram_mib: Some(6144),
                unified_memory_mib: None,
                probe_error: None,
            },
            &solstone_core_local::BackendChoice {
                backend: solstone_core_local::Backend::Cuda,
                reason: "test choice".to_owned(),
            },
            true,
            &[solstone_core_local::VulkanDevice {
                index: 0,
                name: "Test GPU".to_owned(),
                device_type: Some(2),
                vram_mib: 6144,
            }],
            None,
            force_cpu,
        );
        let gpu = report
            .checks
            .iter()
            .find(|check| check.name == "gpu")
            .unwrap();
        assert!(!gpu.detail.contains(solstone_core_local::CPU_PLACEMENT_COPY));
    }

    #[test]
    fn ac6_unknown_provider_surface() {
        let unknown = run_inner(
            options("bogus"),
            || panic!("unknown must not resolve journal"),
            |_| panic!("unknown must not inspect"),
            |_, _| panic!("unknown must not install"),
            |_| panic!("local must not install"),
        );
        assert_eq!(unknown.exit_code, 2);
        assert_eq!(
            unknown.stderr,
            ["unsupported provider \"bogus\"; supported: local, parakeet"]
        );
    }

    #[test]
    fn ac7_failure_reads_persisted_status_or_reports_unreadable_status() {
        let journal = tempfile::tempdir().unwrap();
        let persisted = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Ok)),
            |_, _| Err(dispatch_error("download failed")),
            |_| panic!("local must not install"),
        );
        assert_eq!(persisted.exit_code, 1);
        assert_eq!(
            serde_json::from_str::<Value>(&persisted.stdout[0]).unwrap()["install_state"],
            "idle"
        );

        fs::create_dir_all(
            status::status_path(journal.path(), "parakeet")
                .parent()
                .unwrap(),
        )
        .unwrap();
        fs::write(status::status_path(journal.path(), "parakeet"), b"not-json").unwrap();
        let unreadable = run_inner_with(
            options("parakeet"),
            || Ok(journal.path().to_path_buf()),
            |_| missing_readiness(),
            Some(report(fit_report::FitSeverity::Ok)),
            |_, _| Err(dispatch_error("download failed")),
            |_| panic!("local must not install"),
        );
        assert_eq!(unreadable.exit_code, 1);
        assert!(unreadable.stdout.is_empty());
        assert!(
            unreadable
                .stderr
                .iter()
                .any(|line| line.starts_with("could not read persisted parakeet install status:"))
        );
    }

    fn dispatch_error(message: &str) -> Box<DispatchError> {
        Box::new(DispatchError {
            envelope: solstone_core_local::install::InstallEnvelope {
                schema: "solstone-local-install-v1",
                outcome: "error",
                result: None,
                error: Some(solstone_core_local::install::InstallError {
                    kind: "download".to_owned(),
                    reason_code: "download_failed".to_owned(),
                    message: message.to_owned(),
                }),
            },
            exit_code: 74,
        })
    }

    fn install_busy_error() -> Box<DispatchError> {
        Box::new(DispatchError {
            envelope: solstone_core_local::install::InstallEnvelope {
                schema: "solstone-local-install-v1",
                outcome: "error",
                result: None,
                error: Some(solstone_core_local::install::InstallError {
                    kind: "busy".to_owned(),
                    reason_code: "install_busy".to_owned(),
                    message: "local install lease is held".to_owned(),
                }),
            },
            exit_code: lease::BUSY_EXIT_CODE,
        })
    }

    fn assert_no_temporary_files(root: &Path) {
        for entry in fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            assert!(
                !entry.file_name().to_string_lossy().contains(".tmp"),
                "temporary artifact leaked under {}",
                path.display()
            );
            if path.is_dir() {
                assert_no_temporary_files(&path);
            }
        }
    }
}
