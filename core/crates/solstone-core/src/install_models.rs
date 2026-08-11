// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `solstone-core install-models` orchestration.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use solstone_core_cli::{InstallModelsOptions, InstallModelsVariant};
use solstone_core_local::install::{
    DispatchError, fingerprint, fit_report, install_parakeet_with_lease, lease, pins, status,
};
use solstone_core_transcribe::resolve_model_asset;

use crate::{
    EXIT_DATAERR, EXIT_IOERR, EXIT_TEMPFAIL, EXIT_UNAVAILABLE, EXIT_USAGE,
    eprint_journal_path_error, resolve_process_journal_path,
};

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
}

/// Collect process inputs only; all command decisions live in `run_inner`.
pub fn run(options: InstallModelsOptions) -> ExitCode {
    let host = HostPlatform {
        os_name: normalize_os(std::env::consts::OS).to_owned(),
        arch: normalize_arch(std::env::consts::ARCH).to_owned(),
        journal_variant: std::env::var("JOURNAL_VARIANT").ok(),
    };
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
        |name| {
            resolve_model_asset(name)
                .map(|_| ())
                .map_err(|error| error.to_string())
        },
        None,
        install_executor,
    )
}

fn run_inner_with<P, J, A, I>(
    host: HostPlatform,
    nvidia_probe: P,
    options: InstallModelsOptions,
    journal_resolver: J,
    mut asset_gate: A,
    report_override: Option<fit_report::FitReport>,
    install_executor: I,
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
        if let Err(error) = asset_gate(asset) {
            return InstallModelsOutcome::failure(
                variant,
                EXIT_DATAERR,
                format!("bundled asset verification failed: {error}"),
            );
        }
    }

    let Some(variant) = variant else {
        return InstallModelsOutcome::success(
            None,
            vec![format!(
                "parakeet install: unsupported platform {}/{}; supported: darwin/arm64, linux/x86_64",
                host.os_name, host.arch
            )],
        );
    };

    if variant == ResolvedVariant::Coreml {
        return InstallModelsOutcome::failure(
            Some(variant),
            EXIT_UNAVAILABLE,
            "install-models: resolved variant coreml, but CoreML model install is not implemented in the native shell",
        );
    }

    let key = match pins::parakeet_artifact_key(&host.os_name, &host.arch) {
        Ok(key) => key,
        Err(error) => {
            return InstallModelsOutcome::failure(Some(variant), EXIT_DATAERR, error.to_string());
        }
    };
    let journal = match journal_resolver() {
        Ok(journal) => journal,
        Err(()) => {
            return InstallModelsOutcome {
                resolved_variant: Some(variant),
                exit_code: EXIT_TEMPFAIL,
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
        }
    };
    if options.check {
        return match parakeet_ready(&journal, &key) {
            Ok(path) => InstallModelsOutcome::success(Some(variant), vec![ready_line(&path)]),
            Err(message) => InstallModelsOutcome::failure(Some(variant), EXIT_DATAERR, message),
        };
    }
    if !options.force
        && let Ok(path) = parakeet_ready(&journal, &key)
    {
        return InstallModelsOutcome::success(Some(variant), vec![ready_line(&path)]);
    }

    let report = report_override.unwrap_or_else(|| {
        fit_report::build_parakeet_fit_report(&journal, &host.os_name, &host.arch)
    });
    let rendered = fit_report::render_fit_report(&report);
    if report.overall() == fit_report::FitSeverity::Blocked {
        return InstallModelsOutcome::failure(Some(variant), EXIT_UNAVAILABLE, rendered);
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
    InstallModelsOutcome {
        resolved_variant: Some(variant),
        exit_code: 0,
        stdout: vec![ready_line(&model)],
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

fn normalize_os(os_name: &str) -> &str {
    if os_name == "macos" {
        "darwin"
    } else {
        os_name
    }
}

fn normalize_arch(arch: &str) -> &str {
    if arch == "aarch64" { "arm64" } else { arch }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use std::process::Command;
    use std::thread;

    const ASSET_GATE_JOURNAL_ENV: &str = "SOLSTONE_CORE_ASSET_GATE_JOURNAL";

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
        let outcome = run_inner_with(
            host("linux", "arm64", None),
            || false,
            options(InstallModelsVariant::Cuda),
            || Err(()),
            |_| panic!("asset gate must not run"),
            None,
            |_, _, _| panic!("installer must not run"),
        );
        assert_eq!(outcome.exit_code, EXIT_USAGE);
        assert_eq!(
            outcome.stderr,
            ["variant 'cuda' not supported on linux/arm64"]
        );
    }

    #[test]
    fn normalization_maps_platform_names_and_raw_values_fall_through() {
        assert_eq!(normalize_os("macos"), "darwin");
        assert_eq!(normalize_arch("aarch64"), "arm64");
        assert_eq!(normalize_os("linux"), "linux");
        assert_eq!(normalize_arch("x86_64"), "x86_64");

        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with(
            host("macos", "aarch64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _| panic!("installer must not run"),
        );
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            outcome.stdout,
            [
                "parakeet install: unsupported platform macos/aarch64; supported: darwin/arm64, linux/x86_64"
            ]
        );
    }

    #[test]
    fn darwin_resolves_coreml_after_the_asset_gate_without_probing_nvidia() {
        let journal = tempfile::tempdir().unwrap();
        let outcome = run_inner_with(
            host("darwin", "arm64", None),
            || panic!("probe must not run"),
            options(InstallModelsVariant::Auto),
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            None,
            |_, _, _| panic!("installer must not run"),
        );
        assert_eq!(outcome.resolved_variant, Some(ResolvedVariant::Coreml));
        assert_eq!(outcome.exit_code, EXIT_UNAVAILABLE);
        assert_eq!(
            outcome.stderr,
            [
                "install-models: resolved variant coreml, but CoreML model install is not implemented in the native shell"
            ]
        );
    }

    #[test]
    fn bundled_asset_gate_names_only_speaker_assets_and_never_enters_the_installer() {
        let journal = tempfile::tempdir().unwrap();
        let mut seen = Vec::new();
        let outcome = run_inner_with(
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
    fn real_asset_gate_reports_wespeaker_digest_mismatch_before_installing() {
        if let Some(journal) = env::var_os(ASSET_GATE_JOURNAL_ENV) {
            let journal = PathBuf::from(journal);
            let outcome = run_inner(
                host("linux", "x86_64", None),
                || false,
                options(InstallModelsVariant::Auto),
                || Ok(journal.clone()),
                |_, _, _| panic!("installer must not run"),
            );
            assert_eq!(outcome.exit_code, EXIT_DATAERR);
            assert!(outcome.stderr[0].contains("wespeaker-resnet34-256.onnx"));
            assert!(outcome.stderr[0].contains("has sha256"));
            assert!(outcome.stderr[0].contains(
                "expected 5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94"
            ));
            assert_journal_empty(&journal);
            return;
        }

        let journal = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        fs::write(
            assets.path().join("wespeaker-resnet34-256.onnx"),
            b"wrong digest",
        )
        .unwrap();
        // The resolver reads a process-global environment variable. The workspace
        // forbids unsafe in-process mutation, so a serial (`--test-threads=1`, as
        // used by `make check-rust-test`) child test process owns the override.
        let output = Command::new(env::current_exe().unwrap())
            .arg(
                "install_models::tests::real_asset_gate_reports_wespeaker_digest_mismatch_before_installing",
            )
            .arg("--exact")
            .arg("--test-threads=1")
            .env("SOLSTONE_TRANSCRIBE_MODEL_ASSETS_DIR", assets.path())
            .env(ASSET_GATE_JOURNAL_ENV, journal.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child test failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        // A stale libtest filter exits successfully after running zero tests.
        assert!(
            stdout.contains("test result: ok. 1 passed;") && !stdout.contains("0 passed;"),
            "child did not run exactly one test:\n{stdout}",
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
        let outcome = run_inner_with(
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
        let outcome = run_inner_with(
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
            |_, _, _| {
                installer_entered = true;
                Ok(model)
            },
        );
        assert!(installer_entered);
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.stdout[0].starts_with("model ready: "));
        assert_eq!(outcome.stderr, [rendered]);
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
        let outcome = run_inner_with(
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
            |_, delegated_host, _| {
                assert_eq!(delegated_host.os_name, "linux");
                assert_eq!(delegated_host.arch, "arm64");
                Ok(model)
            },
        );
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
    }

    #[test]
    fn same_target_held_lease_is_observed_to_ready() {
        let journal = tempfile::tempdir().unwrap();
        let host = host("linux", "x86_64", None);
        let key = pins::parakeet_artifact_key(&host.os_name, &host.arch).unwrap();
        let paths = pins::parakeet_paths(journal.path(), &key);
        for field in ["binary_path_cpu", "binary_path_vulkan", "model_path"] {
            let path = PathBuf::from(paths[field].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"ready").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let sha = parakeet_target_sha(journal.path(), &host).unwrap();
        let mut attempt = status::idle_status("parakeet");
        attempt.target_fingerprint_sha256 = Some(sha);
        let attempt = status::transition(attempt, "resolving", None, None).unwrap();
        status::write_status(journal.path(), attempt).unwrap();
        let held = lease::acquire(journal.path(), "parakeet").unwrap().unwrap();
        let writer_journal = journal.path().to_path_buf();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(400));
            let current = status::read_status(&writer_journal, "parakeet").unwrap();
            let installed = status::transition(current, "installed", None, None).unwrap();
            status::write_status(&writer_journal, installed).unwrap();
        });
        let report = fit_report::build_parakeet_fit_report_with_free_bytes(
            journal.path(),
            "linux",
            "x86_64",
            Ok(u64::MAX),
        );
        let outcome = run_inner_with(
            host,
            || false,
            InstallModelsOptions {
                check: false,
                force: true,
                variant: InstallModelsVariant::Auto,
            },
            || Ok(journal.path().to_path_buf()),
            |_| Ok(()),
            Some(report),
            |_, _, _| panic!("installer must not run"),
        );
        writer.join().unwrap();
        drop(held);
        assert_eq!(outcome.exit_code, 0, "{outcome:?}");
        assert!(outcome.stdout[0].starts_with("model ready: "));
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
