// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `solstone-core install-models` orchestration.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use solstone_core_cli::{InstallModelsOptions, InstallModelsVariant};
use solstone_core_journal_config::{
    JournalConfigRead,
    parakeet_coreml::{parakeet_coreml_cache_dir, parakeet_coreml_model_root},
    read_journal_config,
};
/// The native ced installer resolves both artifacts through the catalog's
/// `origin_key` against a single-element host allowlist, revalidated per
/// redirect hop -- so naming github.com and huggingface.co here named two
/// parties this path never contacts, on the surface whose whole job is to say
/// where bytes come from.
///
/// ⛔ The REFERENCE's copy of this sentence still names both hosts and that is
/// still TRUE, because `ced_install.py` still builds the upstream URLs. The
/// string and the code it introduces drift in both directions; the check is
/// per-site -- trace the sentence to the code that follows it and read what
/// THAT code fetches.
fn ced_download_disclosure() -> String {
    format!(
        "ced assets: downloading the ced.cpp {} engine (MIT) and the ced-tiny-q8_0 model (Apache-2.0) from updates.solstone.app. see THIRD_PARTY_NOTICES.md.",
        ced_install::ENGINE_VERSION
    )
}

/// RF-DETR assets are verified from the release tree, so this disclosure must
/// describe the bundled payload rather than an upstream or mirror endpoint.
fn rfdetr_bundled_asset_disclosure() -> String {
    format!(
        "rf-detr assets: verifying the bundled rf-detr.cpp {} engine (Apache-2.0) and RF-DETR nano GGUF weights (Apache-2.0). see THIRD_PARTY_NOTICES.md.",
        rfdetr_install::ENGINE_VERSION
    )
}

use solstone_core_assets::canonical_host_pair;
use solstone_core_local::install::ced_readiness::{
    CED_UNAVAILABLE_GUIDANCE, CedReadiness, evaluate_ced_readiness,
};
use solstone_core_local::install::rfdetr_readiness::{RfdetrReadiness, evaluate_rfdetr_readiness};
use solstone_core_local::install::{
    DispatchError, ced_install, coreml_install, fingerprint, fit_report,
    install_parakeet_with_lease, lease, pins, rfdetr_install, status,
};
use solstone_core_transcribe::resolve_model_asset;

use crate::{
    EXIT_DATAERR, EXIT_IOERR, EXIT_TEMPFAIL, EXIT_UNAVAILABLE, EXIT_USAGE,
    eprint_journal_path_error, resolve_process_journal_path,
};

fn rfdetr_ready_record(
    journal: &Path,
    os_name: &str,
    arch: &str,
    record: rfdetr_install::RfdetrInstallRecord,
) -> Result<rfdetr_install::RfdetrInstallRecord, rfdetr_install::RfdetrInstallError> {
    match evaluate_rfdetr_readiness(journal, os_name, arch) {
        RfdetrReadiness::Ready { .. } => Ok(record),
        RfdetrReadiness::Unsupported { .. } => {
            Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable)
        }
        RfdetrReadiness::Degraded { detail, .. } => Err(rfdetr_install::RfdetrInstallError::new(
            "unrunnable",
            detail,
            69,
        )),
    }
}

const OBSERVE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const OBSERVE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const OBSERVE_PROGRESS_INTERVAL: Duration = Duration::from_secs(10);

// Python reports all failures as stderr/exit 1. This native-only command has no
// caller that parses that status, so use the bin's sysexits convention instead:
// invalid variants use EX_USAGE, asset/ready-state corruption EX_DATAERR,
// unavailable host or fit EX_UNAVAILABLE, and lease waits EX_TEMPFAIL.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostPlatform {
    os_name: String,
    arch: String,
    journal_variant: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedVariant {
    Cpu,
    Cuda,
    Coreml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallerAction {
    Check,
    Install { force: bool },
}

type CedInstaller<'a> = dyn FnMut(
        &Path,
        &str,
        &str,
        InstallerAction,
    ) -> Result<Option<ced_install::CedRecord>, ced_install::CedInstallError>
    + 'a;
type RfdetrInstaller<'a> = dyn FnMut(
        &Path,
        &str,
        &str,
        InstallerAction,
    ) -> Result<rfdetr_install::RfdetrInstallRecord, rfdetr_install::RfdetrInstallError>
    + 'a;
type CoremlInstaller<'a> = dyn FnMut(
        &Path,
        &JournalConfigRead,
        InstallerAction,
    ) -> Result<PathBuf, coreml_install::CoremlInstallError>
    + 'a;

struct ProviderInstallers<'a> {
    ced: Box<CedInstaller<'a>>,
    rfdetr: Box<RfdetrInstaller<'a>>,
    coreml: Box<CoremlInstaller<'a>>,
}

struct InstallModelsHooks<A> {
    asset_gate: A,
    report_override: Option<fit_report::FitReport>,
    ced_verdict: fn(&Path, &str, &str) -> CedReadiness,
}

fn install_models_hooks<A>(
    asset_gate: A,
    report_override: Option<fit_report::FitReport>,
) -> InstallModelsHooks<A>
where
    A: FnMut(&str) -> Result<(), String>,
{
    InstallModelsHooks {
        asset_gate,
        report_override,
        ced_verdict: evaluate_ced_readiness,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InstallModelsOutcome {
    resolved_variant: Option<ResolvedVariant>,
    exit_code: u8,
    stdout: Vec<String>,
    stderr: Vec<String>,
}

impl InstallModelsOutcome {
    fn success(variant: Option<ResolvedVariant>, stdout: Vec<String>) -> Self {
        Self {
            resolved_variant: variant,
            exit_code: 0,
            stdout,
            stderr: Vec::new(),
        }
    }

    fn failure(
        variant: Option<ResolvedVariant>,
        exit_code: u8,
        message: impl Into<String>,
    ) -> Self {
        Self {
            resolved_variant: variant,
            exit_code,
            stdout: Vec::new(),
            stderr: vec![message.into()],
        }
    }

    fn failure_with_stdout(
        variant: Option<ResolvedVariant>,
        exit_code: u8,
        message: impl Into<String>,
        stdout: Vec<String>,
    ) -> Self {
        Self {
            resolved_variant: variant,
            exit_code,
            stdout,
            stderr: vec![message.into()],
        }
    }
}

/// Collect process inputs only; all command decisions live in `run_inner`.
pub fn run(options: InstallModelsOptions) -> ExitCode {
    let (os_name, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    let host = HostPlatform {
        os_name: os_name.to_owned(),
        arch: arch.to_owned(),
        journal_variant: std::env::var("JOURNAL_VARIANT").ok(),
    };
    let home_dir = std::env::home_dir().unwrap_or_default();
    let outcome = run_inner(
        host,
        || solstone_core_local::probe_nvidia_gpu().detected,
        options,
        || match resolve_process_journal_path() {
            Ok(journal) => Ok(journal.path),
            Err(error) => {
                eprint_journal_path_error(error);
                Err(())
            }
        },
        install_model,
        &home_dir,
    );
    for line in outcome.stdout {
        println!("{line}");
    }
    for line in outcome.stderr {
        eprintln!("{line}");
    }
    ExitCode::from(outcome.exit_code)
}

fn run_inner<P, J, I>(
    host: HostPlatform,
    nvidia_probe: P,
    options: InstallModelsOptions,
    journal_resolver: J,
    install_executor: I,
    home_dir: &Path,
) -> InstallModelsOutcome
where
    P: FnOnce() -> bool,
    J: FnOnce() -> Result<PathBuf, ()>,
    I: FnOnce(&Path, &HostPlatform, lease::InstallLease) -> Result<PathBuf, Box<DispatchError>>,
{
    run_inner_with(
        host,
        nvidia_probe,
        options,
        journal_resolver,
        install_models_hooks(
            |name: &str| {
                resolve_model_asset(name)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            },
            None,
        ),
        ProviderInstallers {
            ced: Box::new(|journal, os_name, arch, action| match action {
                InstallerAction::Check => ced_install::check_ced_assets(journal, os_name, arch),
                InstallerAction::Install { force } => {
                    ced_install::install_ced_assets(journal, os_name, arch, force)
                }
            }),
            rfdetr: Box::new(|journal, os_name, arch, action| match action {
                InstallerAction::Check => {
                    rfdetr_install::check_rfdetr_model(journal, os_name, arch)
                        .and_then(|record| rfdetr_ready_record(journal, os_name, arch, record))
                }
                InstallerAction::Install { force } => {
                    rfdetr_install::install_rfdetr(journal, os_name, arch, force)
                        .and_then(|record| rfdetr_ready_record(journal, os_name, arch, record))
                }
            }),
            coreml: Box::new(|home_dir, config, action| match action {
                InstallerAction::Check => {
                    coreml_install::check_parakeet_coreml_install(home_dir, config).map(|()| {
                        parakeet_coreml_model_root(&parakeet_coreml_cache_dir(config, home_dir))
                    })
                }
                InstallerAction::Install { force } => {
                    coreml_install::install_parakeet_coreml_model(home_dir, config, force)
                }
            }),
        },
        install_executor,
        home_dir,
    )
}

#[allow(clippy::too_many_arguments)] // The injected home is a deliberate test-safety seam.
fn run_inner_with<'a, P, J, A, I>(
    host: HostPlatform,
    nvidia_probe: P,
    options: InstallModelsOptions,
    journal_resolver: J,
    mut hooks: InstallModelsHooks<A>,
    mut providers: ProviderInstallers<'a>,
    install_executor: I,
    home_dir: &Path,
) -> InstallModelsOutcome
where
    P: FnOnce() -> bool,
    J: FnOnce() -> Result<PathBuf, ()>,
    A: FnMut(&str) -> Result<(), String>,
    I: FnOnce(&Path, &HostPlatform, lease::InstallLease) -> Result<PathBuf, Box<DispatchError>>,
{
    let variant = match resolve_variant(&options, &host, nvidia_probe) {
        Ok(variant) => variant,
        Err(message) => return InstallModelsOutcome::failure(None, EXIT_USAGE, message),
    };

    // Intentionally no native equivalent of `solstone/think/provider_cache_seed.py` here:
    // it is a developer-worktree hardlink convenience, and this owner-facing subcommand
    // has no developer-worktree cache-seeding path.
    for asset in [
        "wespeaker-resnet34-256.onnx",
        "pyannote-segmentation-3.0.onnx",
    ] {
        if let Err(error) = (hooks.asset_gate)(asset) {
            return InstallModelsOutcome::failure(
                variant,
                EXIT_DATAERR,
                format!("bundled asset verification failed: {error}"),
            );
        }
    }

    let journal = match journal_resolver() {
        Ok(journal) => journal,
        Err(()) => {
            return InstallModelsOutcome {
                resolved_variant: variant,
                exit_code: EXIT_TEMPFAIL,
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
        }
    };
    let mut provider_stdout = Vec::new();
    match (hooks.ced_verdict)(&journal, &host.os_name, &host.arch) {
        CedReadiness::Unsupported { os, arch } => {
            provider_stdout.push(format!(
                "ced install: unsupported platform {os}/{arch}; skipping ced sound-tag assets"
            ));
        }
        CedReadiness::Ready { .. } if options.check || !options.force => {
            provider_stdout.push(ready_line(&ced_install::ced_model_path(&journal)));
        }
        CedReadiness::Degraded { .. } if options.check => {
            return InstallModelsOutcome::failure_with_stdout(
                variant,
                EXIT_DATAERR,
                CED_UNAVAILABLE_GUIDANCE.to_owned(),
                provider_stdout,
            );
        }
        CedReadiness::Ready { .. } | CedReadiness::Degraded { .. } => {
            provider_stdout.push(ced_download_disclosure());
            match (providers.ced)(
                &journal,
                &host.os_name,
                &host.arch,
                InstallerAction::Install {
                    force: options.force,
                },
            ) {
                Ok(None) => provider_stdout.push(format!(
                    "ced install: unsupported platform {}/{}; skipping ced sound-tag assets",
                    host.os_name, host.arch
                )),
                Err(error) => {
                    return InstallModelsOutcome::failure_with_stdout(
                        variant,
                        error.exit_code,
                        error.to_string(),
                        provider_stdout,
                    );
                }
                Ok(Some(_)) => match (hooks.ced_verdict)(&journal, &host.os_name, &host.arch) {
                    CedReadiness::Ready { .. } => {
                        provider_stdout.push(ready_line(&ced_install::ced_model_path(&journal)));
                    }
                    CedReadiness::Degraded { .. } => {
                        return InstallModelsOutcome::failure_with_stdout(
                            variant,
                            EXIT_DATAERR,
                            CED_UNAVAILABLE_GUIDANCE.to_owned(),
                            provider_stdout,
                        );
                    }
                    CedReadiness::Unsupported { os, arch } => {
                        provider_stdout.push(format!(
                            "ced install: unsupported platform {os}/{arch}; skipping ced sound-tag assets"
                        ));
                    }
                },
            }
        }
    }
    if rfdetr_install::rfdetr_artifact_key(&host.os_name, &host.arch).is_none() {
        provider_stdout.push(format!(
            "rf-detr install: unsupported platform {}/{}; skipping rf-detr object-detection assets",
            host.os_name, host.arch
        ));
    } else if options.check {
        if let Err(error) =
            (providers.rfdetr)(&journal, &host.os_name, &host.arch, InstallerAction::Check)
        {
            return InstallModelsOutcome::failure_with_stdout(
                variant,
                error.exit_code,
                error.to_string(),
                provider_stdout,
            );
        }
    } else {
        let ready = if options.force {
            false
        } else {
            match (providers.rfdetr)(&journal, &host.os_name, &host.arch, InstallerAction::Check) {
                Ok(rfdetr_install::RfdetrInstallRecord::Installed) => true,
                Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable) | Err(_) => false,
            }
        };
        if !ready {
            provider_stdout.push(rfdetr_bundled_asset_disclosure());
            if let Err(error) = (providers.rfdetr)(
                &journal,
                &host.os_name,
                &host.arch,
                InstallerAction::Install {
                    force: options.force,
                },
            ) {
                return InstallModelsOutcome::failure_with_stdout(
                    variant,
                    error.exit_code,
                    error.to_string(),
                    provider_stdout,
                );
            }
        }
    }

    let Some(variant) = variant else {
        return InstallModelsOutcome::success(
            None,
            [provider_stdout, vec![format!(
                "parakeet install: unsupported platform {}/{}; supported: darwin/arm64, linux/x86_64",
                host.os_name, host.arch
            )]].concat(),
        );
    };

    if variant == ResolvedVariant::Coreml {
        let config = match read_journal_config(&journal) {
            Ok(config) => config,
            Err(error) => {
                return InstallModelsOutcome::failure_with_stdout(
                    Some(variant),
                    EXIT_DATAERR,
                    error.to_string(),
                    provider_stdout,
                );
            }
        };
        let action = if options.check {
            InstallerAction::Check
        } else {
            InstallerAction::Install {
                force: options.force,
            }
        };
        return match (providers.coreml)(home_dir, &config, action) {
            Ok(path) => InstallModelsOutcome {
                resolved_variant: Some(variant),
                exit_code: 0,
                stdout: [provider_stdout, vec![ready_line(&path)]].concat(),
                stderr: (!options.check)
                    .then(|| coreml_install::PARAKEET_COREML_DOWNLOAD_DISCLOSURE.to_owned())
                    .into_iter()
                    .collect(),
            },
            Err(error) => InstallModelsOutcome::failure_with_stdout(
                Some(variant),
                error.exit_code,
                error.to_string(),
                provider_stdout,
            ),
        };
    }

    let key = match pins::parakeet_artifact_key(&host.os_name, &host.arch) {
        Ok(key) => key,
        Err(error) => {
            return InstallModelsOutcome::failure_with_stdout(
                Some(variant),
                EXIT_DATAERR,
                error.to_string(),
                provider_stdout,
            );
        }
    };
    if options.check {
        return match parakeet_ready(&journal, &key) {
            Ok(path) => InstallModelsOutcome::success(
                Some(variant),
                [provider_stdout, vec![ready_line(&path)]].concat(),
            ),
            Err(message) => InstallModelsOutcome::failure_with_stdout(
                Some(variant),
                EXIT_DATAERR,
                message,
                provider_stdout,
            ),
        };
    }
    if !options.force
        && let Ok(path) = parakeet_ready(&journal, &key)
    {
        return InstallModelsOutcome::success(
            Some(variant),
            [provider_stdout, vec![ready_line(&path)]].concat(),
        );
    }

    let report = hooks.report_override.unwrap_or_else(|| {
        fit_report::build_parakeet_fit_report(&journal, &host.os_name, &host.arch)
    });
    let rendered = fit_report::render_fit_report(&report);
    if report.overall() == fit_report::FitSeverity::Blocked {
        return InstallModelsOutcome::failure_with_stdout(
            Some(variant),
            EXIT_UNAVAILABLE,
            rendered,
            provider_stdout,
        );
    }
    let mut stderr = Vec::new();
    if report.overall() == fit_report::FitSeverity::Warning {
        stderr.push(rendered);
    }

    let target_sha = match parakeet_target_sha(&journal, &host) {
        Ok(value) => value,
        Err(error) => {
            return InstallModelsOutcome::failure(Some(variant), EXIT_DATAERR, error.to_string());
        }
    };
    let held = match lease::acquire(&journal, "parakeet") {
        Ok(value) => value,
        Err(error) => {
            return InstallModelsOutcome::failure(Some(variant), EXIT_IOERR, error.to_string());
        }
    };
    let model = match held {
        Some(held) => match install_executor(&journal, &host, held) {
            Ok(path) => path,
            Err(error) => {
                return InstallModelsOutcome::failure(
                    Some(variant),
                    error.exit_code,
                    install_error_message(*error),
                );
            }
        },
        None => {
            let current = match status::read_status(&journal, "parakeet") {
                Ok(status) => status,
                Err(error) => {
                    return InstallModelsOutcome::failure(
                        Some(variant),
                        EXIT_IOERR,
                        error.to_string(),
                    );
                }
            };
            if !status::is_in_flight(&current.install_state)
                || current.target_fingerprint_sha256.as_deref() != Some(&target_sha)
            {
                return InstallModelsOutcome::failure(
                    Some(variant),
                    EXIT_TEMPFAIL,
                    "parakeet install already running for a different target",
                );
            }
            match status::observe_attempt(
                &journal,
                "parakeet",
                &target_sha,
                OBSERVE_POLL_INTERVAL,
                OBSERVE_TIMEOUT,
                OBSERVE_PROGRESS_INTERVAL,
                |state| {
                    let suffix = state
                        .progress_bytes_received
                        .map(|received| match state.progress_bytes_total {
                            Some(total) => format!(" {received}/{total}"),
                            None => format!(" {received}"),
                        })
                        .unwrap_or_default();
                    eprintln!(
                        "observing parakeet install: {}{suffix}",
                        state.install_state
                    );
                },
            ) {
                Ok(status::ObserveAttempt::Terminal(state))
                    if state.install_state == "installed" =>
                {
                    match parakeet_ready(&journal, &key) {
                        Ok(path) => path,
                        Err(message) => {
                            return InstallModelsOutcome::failure(
                                Some(variant),
                                EXIT_DATAERR,
                                message,
                            );
                        }
                    }
                }
                Ok(status::ObserveAttempt::Terminal(state)) => {
                    return InstallModelsOutcome::failure(
                        Some(variant),
                        EXIT_DATAERR,
                        state
                            .install_error
                            .unwrap_or_else(|| "parakeet install failed".to_owned()),
                    );
                }
                Ok(status::ObserveAttempt::DifferentTarget(_)) => {
                    return InstallModelsOutcome::failure(
                        Some(variant),
                        EXIT_TEMPFAIL,
                        "parakeet install already running for a different target",
                    );
                }
                Ok(status::ObserveAttempt::TimedOut) => {
                    return InstallModelsOutcome::failure(
                        Some(variant),
                        EXIT_TEMPFAIL,
                        "timed out observing parakeet install",
                    );
                }
                Err(error) => {
                    return InstallModelsOutcome::failure(
                        Some(variant),
                        EXIT_IOERR,
                        error.to_string(),
                    );
                }
            }
        }
    };
    provider_stdout.push(ready_line(&model));
    InstallModelsOutcome {
        resolved_variant: Some(variant),
        exit_code: 0,
        stdout: provider_stdout,
        stderr,
    }
}

fn resolve_variant<P>(
    options: &InstallModelsOptions,
    host: &HostPlatform,
    nvidia_probe: P,
) -> Result<Option<ResolvedVariant>, String>
where
    P: FnOnce() -> bool,
{
    match options.variant {
        InstallModelsVariant::Cpu => {
            if host.os_name != "linux" {
                return Err(format!("variant 'cpu' not supported on {}", host.os_name));
            }
            if !matches!(host.arch.as_str(), "x86_64" | "arm64") {
                return Err(format!(
                    "variant 'cpu' not supported on {}/{}",
                    host.os_name, host.arch
                ));
            }
            Ok(Some(ResolvedVariant::Cpu))
        }
        InstallModelsVariant::Cuda => {
            if host.os_name != "linux" {
                return Err(format!("variant 'cuda' not supported on {}", host.os_name));
            }
            if host.arch != "x86_64" {
                return Err(format!(
                    "variant 'cuda' not supported on {}/{}",
                    host.os_name, host.arch
                ));
            }
            Ok(Some(ResolvedVariant::Cuda))
        }
        InstallModelsVariant::Coreml => {
            if host.os_name != "darwin" {
                return Err(format!(
                    "variant 'coreml' not supported on {}",
                    host.os_name
                ));
            }
            if host.arch != "arm64" {
                return Err(format!(
                    "variant 'coreml' not supported on {}/{}",
                    host.os_name, host.arch
                ));
            }
            Ok(Some(ResolvedVariant::Coreml))
        }
        InstallModelsVariant::Auto => {
            if host.os_name == "darwin" && host.arch == "arm64" {
                return Ok(Some(ResolvedVariant::Coreml));
            }
            if host.os_name == "linux" && matches!(host.arch.as_str(), "x86_64" | "arm64") {
                if let Some(value) = host
                    .journal_variant
                    .as_deref()
                    .filter(|value| !value.is_empty())
                {
                    match value {
                        "cpu" => return Ok(Some(ResolvedVariant::Cpu)),
                        "cuda" if host.arch == "x86_64" => return Ok(Some(ResolvedVariant::Cuda)),
                        "cuda" => {
                            return Err(format!(
                                "invalid JOURNAL_VARIANT='cuda'; use 'cpu' on {}/{}",
                                host.os_name, host.arch
                            ));
                        }
                        _ => {
                            return Err(format!(
                                "invalid JOURNAL_VARIANT='{value}'; use 'cpu' or 'cuda'"
                            ));
                        }
                    }
                }
                if host.arch != "x86_64" {
                    return Ok(Some(ResolvedVariant::Cpu));
                }
                return Ok(Some(if nvidia_probe() {
                    ResolvedVariant::Cuda
                } else {
                    ResolvedVariant::Cpu
                }));
            }
            Ok(None)
        }
    }
}

fn parakeet_target_sha(journal: &Path, host: &HostPlatform) -> Result<String, String> {
    let target = solstone_core_local::install::parakeet_target_for_platform(
        journal,
        &host.os_name,
        &host.arch,
    )
    .map_err(install_error_message)?;
    let text = fingerprint::canonical(target).map_err(|error| error.to_string())?;
    Ok(fingerprint::sha256(&text))
}

fn parakeet_ready(journal: &Path, key: &str) -> Result<PathBuf, String> {
    let paths = pins::parakeet_paths(journal, key);
    for (name, path) in [
        ("binary_cpu", &paths["binary_path_cpu"]),
        ("binary_vulkan", &paths["binary_path_vulkan"]),
    ] {
        let path = PathBuf::from(path.as_str().ok_or("parakeet path missing")?);
        if !path.is_file() {
            return Err(format!(
                "parakeet-cpp check failed: {name} missing at {}",
                path.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::metadata(&path)
                .map_err(|error| error.to_string())?
                .permissions()
                .mode()
                & 0o111
                == 0
            {
                return Err(format!(
                    "parakeet-cpp check failed: {name} not executable at {}",
                    path.display()
                ));
            }
        }
    }
    let model = PathBuf::from(
        paths["model_path"]
            .as_str()
            .ok_or("parakeet model path missing")?,
    );
    if !model.is_file() {
        return Err(format!(
            "parakeet-cpp check failed: model missing at {}",
            model.display()
        ));
    }
    Ok(model)
}

fn install_model(
    journal: &Path,
    host: &HostPlatform,
    held: lease::InstallLease,
) -> Result<PathBuf, Box<DispatchError>> {
    let result =
        install_parakeet_with_lease(journal, &host.os_name, &host.arch, held).map_err(Box::new)?;
    Ok(PathBuf::from(
        result["install"]["model_path"]
            .as_str()
            .expect("parakeet install returned a model path"),
    ))
}

fn install_error_message(error: DispatchError) -> String {
    error
        .envelope
        .error
        .as_ref()
        .map(|value| value.message.clone())
        .unwrap_or_else(|| "parakeet install failed".to_owned())
}

fn ready_line(path: &Path) -> String {
    format!("model ready: {}", path.display())
}

#[cfg(test)]
mod disclosure_tests {
    use super::{ced_download_disclosure, rfdetr_bundled_asset_disclosure};
    use solstone_core_local::install::coreml_install::PARAKEET_COREML_DOWNLOAD_DISCLOSURE;

    /// Every artifact this verb fetches resolves through one primitive with a
    /// single-element host allowlist, so no disclosure it prints may name a
    /// third-party host. Asserting the origin IS named would pass on a sentence
    /// that named all three.
    #[test]
    fn native_download_disclosures_name_only_the_fetching_origin() {
        for line in [
            ced_download_disclosure().as_str(),
            PARAKEET_COREML_DOWNLOAD_DISCLOSURE,
        ] {
            assert!(line.contains("updates.solstone.app"), "{line}");
            for third_party in ["github.com", "huggingface.co", "hf.co", "githubusercontent"] {
                assert!(!line.contains(third_party), "{line} names {third_party}");
            }
        }
        let rfdetr = rfdetr_bundled_asset_disclosure();
        assert!(rfdetr.contains("verifying the bundled"));
        assert!(!rfdetr.contains("downloading"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_local::install::ced_readiness::evaluate_ced_readiness_against;
    use std::fs;

    macro_rules! run_inner_with_test {
        (
            $host:expr,
            $probe:expr,
            $options:expr,
            $journal:expr,
            $asset_gate:expr,
            $report:expr,
            $ced:expr,
            $rfdetr:expr,
            $coreml:expr,
            $executor:expr $(,)?
        ) => {{
            let home = tempfile::tempdir().unwrap();
            run_inner_with(
                $host,
                $probe,
                $options,
                $journal,
                {
                    let mut hooks = install_models_hooks($asset_gate, $report);
                    hooks.ced_verdict = fixture_ced_verdict;
                    hooks
                },
                ProviderInstallers {
                    ced: Box::new($ced),
                    rfdetr: Box::new($rfdetr),
                    coreml: Box::new($coreml),
                },
                $executor,
                home.path(),
            )
        }};
    }

    fn options(variant: InstallModelsVariant) -> InstallModelsOptions {
        InstallModelsOptions {
            check: false,
            force: false,
            variant,
        }
    }

    fn host(os_name: &str, arch: &str, journal_variant: Option<&str>) -> HostPlatform {
        HostPlatform {
            os_name: os_name.to_owned(),
            arch: arch.to_owned(),
            journal_variant: journal_variant.map(ToOwned::to_owned),
        }
    }

    fn seed_ready_ced(journal: &Path, os: &str, arch: &str) {
        let key = ced_install::ced_artifact_key(os, arch).expect("supported CED host");
        assert!(
            solstone_core_local::install::ced_fixture::write_ready_ced_install(journal, key),
            "C compiler required for CED ready fixture"
        );
    }

    fn fixture_ced_verdict(journal: &Path, os: &str, arch: &str) -> CedReadiness {
        match solstone_core_local::install::ced_fixture::ced_model_digest(journal) {
            Ok(digest) => evaluate_ced_readiness_against(journal, os, arch, &digest),
            Err(_) => evaluate_ced_readiness(journal, os, arch),
        }
    }

    #[test]
    fn variant_matrix_normalizes_at_the_inner_operation() {
        let probe = || panic!("probe must not run");
        assert_eq!(
            resolve_variant(
                &options(InstallModelsVariant::Cpu),
                &host("darwin", "arm64", None),
                probe
            ),
            Err("variant 'cpu' not supported on darwin".to_owned())
        );
        assert_eq!(
            resolve_variant(
                &options(InstallModelsVariant::Cpu),
                &host("linux", "riscv64", None),
                || false
            ),
            Err("variant 'cpu' not supported on linux/riscv64".to_owned())
        );
        assert_eq!(
            resolve_variant(
                &options(InstallModelsVariant::Cuda),
                &host("linux", "arm64", None),
                || false
            ),
            Err("variant 'cuda' not supported on linux/arm64".to_owned())
        );
        assert_eq!(
            resolve_variant(
                &options(InstallModelsVariant::Auto),
                &host("linux", "arm64", Some("cuda")),
                || false
            ),
            Err("invalid JOURNAL_VARIANT='cuda'; use 'cpu' on linux/arm64".to_owned())
        );
        assert_eq!(
            resolve_variant(
                &options(InstallModelsVariant::Auto),
                &host("linux", "arm64", None),
                || panic!("probe must not run")
            ),
            Ok(Some(ResolvedVariant::Cpu))
        );
        assert_eq!(
            resolve_variant(
                &options(InstallModelsVariant::Auto),
                &host("linux", "x86_64", Some("")),
                || false
            ),
            Ok(Some(ResolvedVariant::Cpu))
        );
        assert_eq!(
            resolve_variant(
                &options(InstallModelsVariant::Auto),
                &host("linux", "x86_64", Some("foo")),
                || panic!("probe must not run")
            ),
            Err("invalid JOURNAL_VARIANT='foo'; use 'cpu' or 'cuda'".to_owned())
        );
    }

    #[test]
    fn variant_refusal_precedes_journal_resolution() {
        let outcome = run_inner_with_test!(
            host("linux", "arm64", None),
            || false,
            options(InstallModelsVariant::Cuda),
            || Err(()),
            |_| panic!("asset gate must not run"),
            None,
            |_, _, _, _| Ok(None),
            |_, _, _, _| panic!("rf-detr installer must not run"),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| panic!("installer must not run"),
        );
        assert_eq!(outcome.exit_code, EXIT_USAGE);
        assert_eq!(
            outcome.stderr,
            ["variant 'cuda' not supported on linux/arm64"]
        );
    }

    #[test]
    fn raw_host_injected_into_run_inner_is_skipped_not_mapped() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with_test!(
            host("macos", "aarch64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| Ok(None),
            |_, _, _, _| panic!("rf-detr installer must not run on an unmapped host"),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| panic!("installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            outcome.stdout,
            [
                "ced install: unsupported platform macos/aarch64; skipping ced sound-tag assets",
                "rf-detr install: unsupported platform macos/aarch64; skipping rf-detr object-detection assets",
                "parakeet install: unsupported platform macos/aarch64; supported: darwin/arm64, linux/x86_64"
            ]
        );
    }

    #[test]
    fn rfdetr_install_record_requires_a_launchable_supported_payload() {
        let journal = tempfile::tempdir().unwrap();
        let error = rfdetr_ready_record(
            journal.path(),
            "linux",
            "x86_64",
            rfdetr_install::RfdetrInstallRecord::Installed,
        )
        .expect_err("a supported host with no launchable payload is not ready");
        assert_eq!(error.reason_code, "unrunnable");
        assert_eq!(error.exit_code, 69);
    }

    #[test]
    fn rfdetr_not_ready_supported_host_discloses_bundled_assets() {
        let journal = tempfile::tempdir().unwrap();
        seed_ready_ced(journal.path(), "darwin", "arm64");
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| Ok(None),
            |_, _, _, action| match action {
                InstallerAction::Check => {
                    Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable)
                }
                InstallerAction::Install { force } => {
                    assert!(!force);
                    Ok(rfdetr_install::RfdetrInstallRecord::Installed)
                }
            },
            |_, _, action| {
                assert_eq!(action, InstallerAction::Install { force: false });
                Ok(journal.path().join("parakeet-tdt-0.6b-v3"))
            },
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.stdout.contains(&rfdetr_bundled_asset_disclosure()));
        assert!(
            outcome
                .stdout
                .iter()
                .any(|line| line.contains(rfdetr_install::ENGINE_VERSION))
        );
    }

    #[test]
    fn rfdetr_check_on_installed_model_has_no_bundled_asset_disclosure() {
        let journal = tempfile::tempdir().unwrap();
        seed_ready_ced(journal.path(), "darwin", "arm64");
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            InstallModelsOptions {
                check: true,
                ..options(InstallModelsVariant::Auto)
            },
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, action| {
                assert_eq!(action, InstallerAction::Check);
                Ok(None)
            },
            |_, _, _, action| {
                assert_eq!(action, InstallerAction::Check);
                Ok(rfdetr_install::RfdetrInstallRecord::Installed)
            },
            |_, _, action| {
                assert_eq!(action, InstallerAction::Check);
                Ok(journal.path().join("parakeet-tdt-0.6b-v3"))
            },
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(!outcome.stdout.contains(&rfdetr_bundled_asset_disclosure()));
    }

    #[test]
    fn rfdetr_ready_model_skips_reinstall() {
        let journal = tempfile::tempdir().unwrap();
        seed_ready_ced(journal.path(), "darwin", "arm64");
        let mut actions = Vec::new();
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| Ok(None),
            |_, _, _, action| {
                actions.push(action);
                Ok(rfdetr_install::RfdetrInstallRecord::Installed)
            },
            |_, _, action| {
                assert_eq!(action, InstallerAction::Install { force: false });
                Ok(journal.path().join("parakeet-tdt-0.6b-v3"))
            },
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(!outcome.stdout.contains(&rfdetr_bundled_asset_disclosure()));
        assert_eq!(actions, vec![InstallerAction::Check]);
    }

    #[test]
    fn rfdetr_probe_error_falls_through_to_install() {
        let journal = tempfile::tempdir().unwrap();
        seed_ready_ced(journal.path(), "darwin", "arm64");
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| Ok(None),
            |_, _, _, action| match action {
                InstallerAction::Check => Err(rfdetr_install::RfdetrInstallError::new(
                    "sidecar_missing",
                    "rf-detr sidecar missing",
                    65,
                )),
                InstallerAction::Install { force } => {
                    assert!(!force);
                    Ok(rfdetr_install::RfdetrInstallRecord::Installed)
                }
            },
            |_, _, action| {
                assert_eq!(action, InstallerAction::Install { force: false });
                Ok(journal.path().join("parakeet-tdt-0.6b-v3"))
            },
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.stdout.contains(&rfdetr_bundled_asset_disclosure()));
    }

    #[test]
    fn rfdetr_force_discloses_even_when_installed() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            InstallModelsOptions {
                force: true,
                ..options(InstallModelsVariant::Auto)
            },
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |journal, os, arch, action| {
                assert!(matches!(action, InstallerAction::Install { force: true }));
                seed_ready_ced(journal, os, arch);
                Ok(Some(ced_install::CedRecord {
                    artifact_key: "macos-metal-arm64".to_owned(),
                    engine_version: ced_install::ENGINE_VERSION.to_owned(),
                    files: std::collections::BTreeMap::new(),
                    model_repo: "mudler/ced-gguf".to_owned(),
                    model_revision: "test".to_owned(),
                }))
            },
            |_, _, _, action| {
                assert_eq!(action, InstallerAction::Install { force: true });
                Ok(rfdetr_install::RfdetrInstallRecord::Installed)
            },
            |_, _, action| {
                assert_eq!(action, InstallerAction::Install { force: true });
                Ok(journal.path().join("parakeet-tdt-0.6b-v3"))
            },
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.stdout.contains(&rfdetr_bundled_asset_disclosure()));
    }

    #[test]
    fn windows_skips_rfdetr_between_ced_and_parakeet() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with_test!(
            host("windows", "x86_64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| panic!("windows must skip ced"),
            |_, _, _, _| panic!("windows must skip rf-detr"),
            |_, _, _| panic!("windows must skip coreml"),
            |_, _, _| panic!("windows must skip parakeet"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            outcome.stdout,
            [
                "ced install: unsupported platform windows/x86_64; skipping ced sound-tag assets",
                "rf-detr install: unsupported platform windows/x86_64; skipping rf-detr object-detection assets",
                "parakeet install: unsupported platform windows/x86_64; supported: darwin/arm64, linux/x86_64"
            ]
        );
    }

    #[test]
    fn ced_orchestration_uses_pre_normalized_host_values() {
        let journal = tempfile::tempdir().unwrap();
        let mut called = false;
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, os_name, arch, action| {
                called = true;
                assert_eq!(
                    ced_install::ced_artifact_key(os_name, arch),
                    Some("macos-metal-arm64")
                );
                match action {
                    InstallerAction::Check => Err(ced_install::CedInstallError::new(
                        "sidecar_missing",
                        "not ready",
                        EXIT_DATAERR,
                    )),
                    InstallerAction::Install { .. } => Ok(Some(ced_install::CedRecord {
                        artifact_key: "macos-metal-arm64".to_owned(),
                        engine_version: "v0.1.0".to_owned(),
                        files: std::collections::BTreeMap::new(),
                        model_repo: "mudler/ced-gguf".to_owned(),
                        model_revision: "test".to_owned(),
                    })),
                }
            },
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable),
            |_, _, _| Ok(journal.path().join("coreml")),
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert!(called);
        assert!(
            !outcome
                .stdout
                .iter()
                .any(|line| line.starts_with("ced install: unsupported"))
        );

        let raw_journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with_test!(
            host("macos", "aarch64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(raw_journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| panic!("raw platform must skip ced"),
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| panic!("unsupported platform must not install parakeet"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(
            outcome.stdout.contains(
                &"ced install: unsupported platform macos/aarch64; skipping ced sound-tag assets"
                    .to_owned()
            )
        );
    }

    #[test]
    fn darwin_resolves_coreml_after_the_asset_gate_without_probing_nvidia() {
        let journal = tempfile::tempdir().unwrap();
        seed_ready_ced(journal.path(), "darwin", "arm64");
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| Ok(None),
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable),
            |_, _, action| {
                assert_eq!(action, InstallerAction::Install { force: false });
                Ok(journal.path().join("parakeet-tdt-0.6b-v3"))
            },
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert_eq!(outcome.resolved_variant, Some(ResolvedVariant::Coreml));
        assert_eq!(
            outcome.stderr,
            [coreml_install::PARAKEET_COREML_DOWNLOAD_DISCLOSURE]
        );
        assert_eq!(outcome.exit_code, 0);
    }

    #[test]
    fn bundled_asset_gate_names_only_speaker_assets_and_never_enters_the_installer() {
        let journal = tempfile::tempdir().unwrap();
        let mut seen = Vec::new();
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |asset| {
                seen.push(asset.to_owned());
                if asset == "pyannote-segmentation-3.0.onnx" {
                    Err("corrupt".to_owned())
                } else {
                    Ok(())
                }
            },
            None,
            |_, _, _, _| Ok(None),
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| panic!("installer must not run"),
        );
        assert_eq!(outcome.exit_code, EXIT_DATAERR);
        assert_eq!(
            seen,
            [
                "wespeaker-resnet34-256.onnx",
                "pyannote-segmentation-3.0.onnx"
            ]
        );
        assert_journal_empty(journal.path());
    }

    #[test]
    fn blocked_fit_report_does_not_enter_the_installer() {
        let journal = tempfile::tempdir().unwrap();
        let report = fit_report::build_parakeet_fit_report_with_free_bytes(
            journal.path(),
            "linux",
            "x86_64",
            Ok(0),
        );
        let outcome = run_inner_with_test!(
            host("linux", "x86_64", None),
            || false,
            InstallModelsOptions {
                check: false,
                force: true,
                variant: InstallModelsVariant::Auto,
            },
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            Some(report),
            |journal, os, arch, action| {
                assert!(matches!(action, InstallerAction::Install { force: true }));
                seed_ready_ced(journal, os, arch);
                Ok(Some(ced_install::CedRecord {
                    artifact_key: "linux-cpu-x64".to_owned(),
                    engine_version: ced_install::ENGINE_VERSION.to_owned(),
                    files: std::collections::BTreeMap::new(),
                    model_repo: "mudler/ced-gguf".to_owned(),
                    model_revision: "test".to_owned(),
                }))
            },
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| panic!("installer must not run"),
        );
        assert_eq!(outcome.exit_code, EXIT_UNAVAILABLE);
    }

    #[test]
    fn warning_fit_report_is_stderr_and_allows_install() {
        let journal = tempfile::tempdir().unwrap();
        let report = fit_report::build_parakeet_fit_report_with_free_bytes(
            journal.path(),
            "linux",
            "x86_64",
            Ok(u64::MAX),
        );
        let rendered = fit_report::render_fit_report(&report);
        let model = journal.path().join("model.gguf");
        let mut installer_entered = false;
        let outcome = run_inner_with_test!(
            host("linux", "x86_64", None),
            || false,
            InstallModelsOptions {
                check: false,
                force: true,
                variant: InstallModelsVariant::Auto,
            },
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            Some(report),
            |journal, os, arch, action| {
                assert!(matches!(action, InstallerAction::Install { force: true }));
                seed_ready_ced(journal, os, arch);
                Ok(Some(ced_install::CedRecord {
                    artifact_key: "linux-cpu-x64".to_owned(),
                    engine_version: ced_install::ENGINE_VERSION.to_owned(),
                    files: std::collections::BTreeMap::new(),
                    model_repo: "mudler/ced-gguf".to_owned(),
                    model_revision: "test".to_owned(),
                }))
            },
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| {
                installer_entered = true;
                Ok(model)
            },
        );
        assert!(installer_entered);
        assert_eq!(outcome.exit_code, 0);
        assert!(
            outcome
                .stdout
                .iter()
                .any(|line| line.starts_with("model ready: "))
        );
        assert_eq!(outcome.stderr, [rendered]);
        // This exists to catch the Core ML disclosure leaking into a Linux
        // run. It keyed on "updates.solstone.app", which was unique to that
        // sentence when it was written and stopped being unique the moment a
        // second native disclosure named the same origin -- so it fired on a
        // line it was never about. Name the subject instead of a substring the
        // subject happens to share; that cannot be weakened by a third one.
        assert!(
            !outcome
                .stdout
                .iter()
                .chain(outcome.stderr.iter())
                .any(|line| line == coreml_install::PARAKEET_COREML_DOWNLOAD_DISCLOSURE)
        );
    }

    #[test]
    fn delegated_installer_receives_the_simulated_host() {
        let journal = tempfile::tempdir().unwrap();
        let report = fit_report::build_parakeet_fit_report_with_free_bytes(
            journal.path(),
            "linux",
            "arm64",
            Ok(u64::MAX),
        );
        let model = journal.path().join("model.gguf");
        let outcome = run_inner_with_test!(
            host("linux", "arm64", None),
            || panic!("probe must not run"),
            InstallModelsOptions {
                check: false,
                force: true,
                variant: InstallModelsVariant::Auto,
            },
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            Some(report),
            |journal, os, arch, action| {
                assert!(matches!(action, InstallerAction::Install { force: true }));
                seed_ready_ced(journal, os, arch);
                Ok(Some(ced_install::CedRecord {
                    artifact_key: "linux-cpu-arm64".to_owned(),
                    engine_version: ced_install::ENGINE_VERSION.to_owned(),
                    files: std::collections::BTreeMap::new(),
                    model_repo: "mudler/ced-gguf".to_owned(),
                    model_revision: "test".to_owned(),
                }))
            },
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::PlatformUnavailable),
            |_, _, _| panic!("coreml installer must not run"),
            |_, delegated_host, _| {
                assert_eq!(delegated_host.os_name, "linux");
                assert_eq!(delegated_host.arch, "arm64");
                Ok(model)
            },
        );
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    }

    #[test]
    fn posix_default_runs_ced_and_parakeet() {
        let journal = tempfile::tempdir().unwrap();
        seed_ready_ced(journal.path(), "linux", "x86_64");
        let mut parakeet_called = false;
        let outcome = run_inner_with_test!(
            host("linux", "x86_64", None),
            || false,
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| panic!("ready CED must not install"),
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::Installed),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| {
                parakeet_called = true;
                Ok(journal.path().join("model.gguf"))
            },
        );
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        assert!(parakeet_called);
        assert!(
            outcome
                .stdout
                .iter()
                .any(|line| line.contains("ced-tiny-q8_0.gguf"))
        );
    }

    #[test]
    fn posix_check_runs_ced_and_coreml() {
        let journal = tempfile::tempdir().unwrap();
        seed_ready_ced(journal.path(), "darwin", "arm64");
        let mut coreml_called = false;
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            InstallModelsOptions {
                check: true,
                ..options(InstallModelsVariant::Auto)
            },
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| panic!("ready CED must not install"),
            |_, _, _, action| {
                assert_eq!(action, InstallerAction::Check);
                Ok(rfdetr_install::RfdetrInstallRecord::Installed)
            },
            |_, _, action| {
                coreml_called = true;
                assert_eq!(action, InstallerAction::Check);
                Ok(journal.path().join("coreml"))
            },
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        assert!(coreml_called);
        assert!(
            outcome
                .stdout
                .iter()
                .any(|line| line.contains("ced-tiny-q8_0.gguf"))
        );
    }

    #[test]
    fn darwin_default_runs_ced_and_coreml() {
        let journal = tempfile::tempdir().unwrap();
        seed_ready_ced(journal.path(), "darwin", "arm64");
        let mut coreml_called = false;
        let outcome = run_inner_with_test!(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| panic!("ready CED must not install"),
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::Installed),
            |_, _, action| {
                coreml_called = true;
                assert_eq!(action, InstallerAction::Install { force: false });
                Ok(journal.path().join("coreml"))
            },
            |_, _, _| panic!("parakeet installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        assert!(coreml_called);
    }

    #[test]
    fn check_degraded_is_exit_dataerr_and_short_circuits() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with_test!(
            host("linux", "x86_64", None),
            || false,
            InstallModelsOptions {
                check: true,
                ..options(InstallModelsVariant::Auto)
            },
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, _| panic!("--check must not install CED"),
            |_, _, _, _| panic!("degraded CED must short-circuit rf-detr"),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| panic!("degraded CED must short-circuit parakeet"),
        );
        assert_eq!(outcome.exit_code, EXIT_DATAERR);
        assert_eq!(outcome.stderr, [CED_UNAVAILABLE_GUIDANCE]);
    }

    #[test]
    fn failed_repair_never_prints_ready() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with_test!(
            host("linux", "x86_64", None),
            || false,
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _, action| {
                assert!(matches!(action, InstallerAction::Install { .. }));
                Err(ced_install::CedInstallError::new(
                    "download_failed",
                    "ced download failed",
                    EXIT_IOERR,
                ))
            },
            |_, _, _, _| panic!("failed CED repair must not reach rf-detr"),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| panic!("failed CED repair must not reach parakeet"),
        );
        assert_eq!(outcome.exit_code, EXIT_IOERR);
        assert!(
            !outcome
                .stdout
                .iter()
                .any(|line| line.starts_with("model ready:"))
        );
    }

    #[test]
    fn successful_repair_reprimes_verdict_before_ready_line() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with_test!(
            host("linux", "x86_64", None),
            || false,
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |journal, os, arch, action| {
                assert!(matches!(action, InstallerAction::Install { .. }));
                seed_ready_ced(journal, os, arch);
                Ok(Some(ced_install::CedRecord {
                    artifact_key: "linux-cpu-x64".to_owned(),
                    engine_version: ced_install::ENGINE_VERSION.to_owned(),
                    files: std::collections::BTreeMap::new(),
                    model_repo: "mudler/ced-gguf".to_owned(),
                    model_revision: "test".to_owned(),
                }))
            },
            |_, _, _, _| Ok(rfdetr_install::RfdetrInstallRecord::Installed),
            |_, _, _| panic!("coreml installer must not run"),
            |_, _, _| Ok(journal.path().join("model.gguf")),
        );
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        assert!(
            outcome
                .stdout
                .iter()
                .any(|line| line.contains("ced-tiny-q8_0.gguf"))
        );
    }

    fn assert_journal_empty(journal: &Path) {
        fn collect_entries(directory: &Path, entries: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                entries.push(path.clone());
                if path.is_dir() {
                    collect_entries(&path, entries);
                }
            }
        }

        let mut entries = Vec::new();
        collect_entries(journal, &mut entries);
        assert!(entries.is_empty(), "journal was mutated: {entries:?}");
    }
}
