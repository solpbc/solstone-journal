// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native owner-facing `solstone-core install-provider` orchestration.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_cli::InstallProviderOptions;
use solstone_core_local::install::{
    DispatchError, fingerprint, fit_report, install_parakeet_with_lease, lease, readiness, status,
};

use crate::{EXIT_UNAVAILABLE, eprint_journal_path_error, resolve_process_journal_path};

const OBSERVE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const OBSERVE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const OBSERVE_PROGRESS_INTERVAL: Duration = Duration::from_secs(10);
const PARAKEET_DOWNLOAD_DISCLOSURE: &str = "parakeet-cpp fetches two external artifacts into this journal's provider cache before it can run: the parakeet.cpp server binary from github.com (MIT) and the speech model from huggingface.co (CC-BY-4.0).";

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
    let outcome = run_inner(
        options,
        || match resolve_process_journal_path() {
            Ok(journal) => Ok(journal.path),
            Err(error) => {
                eprint_journal_path_error(error);
                Err(())
            }
        },
        |journal| {
            readiness::inspect_parakeet(
                json!({"journal": journal.display().to_string()})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
        },
        install_parakeet,
    );
    for line in outcome.stdout {
        println!("{line}");
    }
    for line in outcome.stderr {
        eprintln!("{line}");
    }
    ExitCode::from(outcome.exit_code)
}

fn run_inner<J, R, I>(
    options: InstallProviderOptions,
    journal_resolver: J,
    readiness_provider: R,
    install_executor: I,
) -> InstallProviderOutcome
where
    J: FnOnce() -> Result<PathBuf, ()>,
    R: FnOnce(&Path) -> Value,
    I: FnOnce(&Path, lease::InstallLease) -> Result<Value, Box<DispatchError>>,
{
    run_inner_with(
        options,
        journal_resolver,
        readiness_provider,
        None,
        install_executor,
    )
}

fn run_inner_with<J, R, I>(
    options: InstallProviderOptions,
    journal_resolver: J,
    readiness_provider: R,
    report_override: Option<fit_report::FitReport>,
    install_executor: I,
) -> InstallProviderOutcome
where
    J: FnOnce() -> Result<PathBuf, ()>,
    R: FnOnce(&Path) -> Value,
    I: FnOnce(&Path, lease::InstallLease) -> Result<Value, Box<DispatchError>>,
{
    match options.name.as_str() {
        "parakeet" => {}
        "local" => {
            return InstallProviderOutcome::failure(
                EXIT_UNAVAILABLE,
                "local provider install is not native yet",
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
            Err(error) => return unavailable_status_error(stderr, error),
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
        return observe_existing(&journal, &target_sha, stderr);
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

    match install_executor(&journal, held) {
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
        Err(error) => install_failure(&journal, *error, stderr),
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

fn observe_existing(
    journal: &Path,
    target_sha: &str,
    stderr: Vec<String>,
) -> InstallProviderOutcome {
    observe_existing_with(journal, target_sha, stderr, |state, stderr| {
        stderr.push(progress_line(state));
    })
}

fn observe_existing_with<P>(
    journal: &Path,
    target_sha: &str,
    mut stderr: Vec<String>,
    mut progress: P,
) -> InstallProviderOutcome
where
    P: FnMut(&status::InstallStatus, &mut Vec<String>),
{
    let current = match status::read_status(journal, "parakeet") {
        Ok(status) => status,
        Err(error) => return unavailable_status_error(stderr, error),
    };
    if !status::is_in_flight(&current.install_state)
        || current.target_fingerprint_sha256.as_deref() != Some(target_sha)
    {
        stderr.push("parakeet install already running for a different target".to_owned());
        return InstallProviderOutcome {
            exit_code: 1,
            stdout: Vec::new(),
            stderr,
        };
    }
    match status::observe_attempt(
        journal,
        "parakeet",
        target_sha,
        OBSERVE_POLL_INTERVAL,
        OBSERVE_TIMEOUT,
        OBSERVE_PROGRESS_INTERVAL,
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
            stderr.push("timed out observing parakeet install".to_owned());
            InstallProviderOutcome {
                exit_code: 1,
                stdout: Vec::new(),
                stderr,
            }
        }
        Err(error) => unavailable_status_error(stderr, error),
    }
}

fn progress_line(state: &status::InstallStatus) -> String {
    let suffix = state
        .progress_bytes_received
        .map(|received| match state.progress_bytes_total {
            Some(total) => format!(" {received}/{total}"),
            None => format!(" {received}"),
        })
        .unwrap_or_default();
    format!(
        "observing parakeet install: {}{suffix}",
        state.install_state
    )
}

fn install_failure(
    journal: &Path,
    error: DispatchError,
    mut stderr: Vec<String>,
) -> InstallProviderOutcome {
    stderr.push(dispatch_message(error));
    match status::read_status(journal, "parakeet") {
        Ok(status) => InstallProviderOutcome {
            exit_code: 1,
            stdout: vec![render_status(&status)],
            stderr,
        },
        Err(error) => unavailable_status_error(stderr, error),
    }
}

fn unavailable_status_error(
    error_lines: Vec<String>,
    error: impl ToString,
) -> InstallProviderOutcome {
    let mut stderr = error_lines;
    stderr.push(format!(
        "could not read persisted parakeet install status: {}",
        error.to_string()
    ));
    InstallProviderOutcome {
        exit_code: 1,
        stdout: Vec::new(),
        stderr,
    }
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
            artifact: "test",
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
        );
        assert_eq!(direct.exit_code, 0);

        let target_sha = parakeet_target_sha(journal.path()).unwrap();
        let mut current = status::idle_status("parakeet");
        current.target_fingerprint_sha256 = Some(target_sha.clone());
        let current = status::transition(current, "resolving", None, None).unwrap();
        status::write_status(journal.path(), current.clone()).unwrap();
        let observed =
            observe_existing_with(journal.path(), &target_sha, Vec::new(), |state, _| {
                let mut next = state.clone();
                next.target_fingerprint_sha256 = Some("different".to_owned());
                next.install_state = "downloading".to_owned();
                status::write_status(journal.path(), next).unwrap();
            });
        assert_eq!(observed.exit_code, 1);
        assert!(!observed.stdout.is_empty(), "{observed:?}");
        assert_eq!(
            serde_json::from_str::<Value>(&observed.stdout[0]).unwrap()["install_state"],
            "downloading"
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
        assert!(
            solstone_core_local::install::pins::parakeet_cache_root(journal.path())
                .read_dir()
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp"))
        );

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
    fn ac6_local_and_unknown_provider_surface() {
        let local = run_inner(
            options("local"),
            || panic!("local must not resolve journal"),
            |_| panic!("local must not inspect"),
            |_, _| panic!("local must not install"),
        );
        assert_eq!(local.exit_code, EXIT_UNAVAILABLE);
        assert_eq!(local.stderr, ["local provider install is not native yet"]);
        let unknown = run_inner(
            options("bogus"),
            || panic!("unknown must not resolve journal"),
            |_| panic!("unknown must not inspect"),
            |_, _| panic!("unknown must not install"),
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
}
