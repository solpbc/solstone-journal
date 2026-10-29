// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde::{Deserialize, Serialize};
use solstone_core_check::{build_check_report, gather_host_inputs};
use solstone_core_local::install::status::{self, InstallStatus, is_in_flight};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupRetryError {
    pub code: &'static str,
    pub message: &'static str,
    pub detail: String,
}

impl std::fmt::Display for SetupRetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.message, self.detail)
    }
}

impl std::error::Error for SetupRetryError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupProgressError {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for SetupProgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for SetupProgressError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupProgress {
    pub downloads: SetupDownloads,
    pub thinking: SetupProgressThinking,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupProgressThinking {
    pub capability: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupDownloads {
    pub speech: SetupDownloadTask,
    pub thinking: SetupDownloadTask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupDownloadTask {
    pub state: String,
    pub attempt_id: Option<String>,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub error: Option<SetupDownloadError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetupDownloadError {
    pub code: String,
    pub detail: String,
}

pub fn project_speech(
    observed: Option<&InstallStatus>,
    lease_held: bool,
    current_verified: bool,
    catalog_total: Option<u64>,
) -> SetupDownloadTask {
    project_download(
        observed,
        lease_held,
        current_verified,
        catalog_total,
        "speech_not_verified",
    )
}

pub fn project_thinking(
    observed: Option<&InstallStatus>,
    lease_held: bool,
    current_verified: bool,
    catalog_total: Option<u64>,
) -> SetupDownloadTask {
    project_download(
        observed,
        lease_held,
        current_verified,
        catalog_total,
        "thinking_not_verified",
    )
}

pub fn packaged_speech_retry(verified: Result<(), String>) -> Result<(), SetupRetryError> {
    verified.map_err(|_| SetupRetryError {
        code: "speech_package_unavailable",
        message: "speech recognition needs repair",
        detail: "repair or reinstall the journal app, then restart your journal.".into(),
    })
}

pub fn project_packaged_speech(verified: Result<(), String>) -> SetupDownloadTask {
    match packaged_speech_retry(verified) {
        Ok(()) => SetupDownloadTask {
            state: "complete".into(),
            attempt_id: None,
            bytes_done: 0,
            bytes_total: Some(0),
            error: None,
        },
        Err(error) => SetupDownloadTask {
            state: "error".into(),
            attempt_id: None,
            bytes_done: 0,
            bytes_total: Some(0),
            error: Some(SetupDownloadError {
                code: error.code.into(),
                detail: error.detail,
            }),
        },
    }
}

fn project_download(
    observed: Option<&InstallStatus>,
    lease_held: bool,
    current_verified: bool,
    catalog_total: Option<u64>,
    not_verified_code: &str,
) -> SetupDownloadTask {
    if lease_held
        && let Some(status) = observed
        && is_in_flight(&status.install_state)
    {
        return SetupDownloadTask {
            state: "downloading".into(),
            attempt_id: status.attempt_id.clone(),
            bytes_done: status.progress_bytes_received.unwrap_or(0),
            bytes_total: status.progress_bytes_total.or(catalog_total),
            error: None,
        };
    }

    if current_verified {
        let (bytes_done, bytes_total) = match catalog_total {
            Some(n) => (n, Some(n)),
            None => (0, None),
        };
        let attempt_id = observed.and_then(|s| {
            if s.install_state == "installed" {
                s.attempt_id.clone()
            } else {
                None
            }
        });
        return SetupDownloadTask {
            state: "complete".into(),
            attempt_id,
            bytes_done,
            bytes_total,
            error: None,
        };
    }

    if let Some(status) = observed {
        if status.install_state == "failed"
            && status.error_code.as_deref() == Some("install_cancelled")
        {
            return SetupDownloadTask {
                state: "cancelled".into(),
                attempt_id: status.attempt_id.clone(),
                bytes_done: status.progress_bytes_received.unwrap_or(0),
                bytes_total: status.progress_bytes_total.or(catalog_total),
                error: None,
            };
        }

        if status.install_state == "failed" {
            return SetupDownloadTask {
                state: "error".into(),
                attempt_id: status.attempt_id.clone(),
                bytes_done: status.progress_bytes_received.unwrap_or(0),
                bytes_total: status.progress_bytes_total.or(catalog_total),
                error: Some(SetupDownloadError {
                    code: status
                        .error_code
                        .clone()
                        .unwrap_or_else(|| "install_failed".into()),
                    detail: status.install_error.clone().unwrap_or_default(),
                }),
            };
        }

        if status.install_state == "installed" {
            return SetupDownloadTask {
                state: "error".into(),
                attempt_id: status.attempt_id.clone(),
                bytes_done: status.progress_bytes_received.unwrap_or(0),
                bytes_total: status.progress_bytes_total.or(catalog_total),
                error: Some(SetupDownloadError {
                    code: status
                        .error_code
                        .clone()
                        .unwrap_or_else(|| not_verified_code.into()),
                    detail: status.install_error.clone().unwrap_or_default(),
                }),
            };
        }

        if is_in_flight(&status.install_state) {
            return SetupDownloadTask {
                state: "error".into(),
                attempt_id: status.attempt_id.clone(),
                bytes_done: status.progress_bytes_received.unwrap_or(0),
                bytes_total: status.progress_bytes_total.or(catalog_total),
                error: Some(SetupDownloadError {
                    code: status
                        .error_code
                        .clone()
                        .unwrap_or_else(|| "install_failed".into()),
                    detail: status.install_error.clone().unwrap_or_default(),
                }),
            };
        }
    }

    SetupDownloadTask {
        state: "idle".into(),
        attempt_id: None,
        bytes_done: 0,
        bytes_total: catalog_total,
        error: None,
    }
}

/// Setup's download progress together with whether this computer can run local thinking.
pub fn read_setup_progress(journal: &Path) -> Result<SetupProgress, SetupProgressError> {
    let downloads = read_setup_downloads(journal)?;
    let inputs = gather_host_inputs(journal, env!("CARGO_PKG_VERSION"));
    let report = build_check_report(&inputs);
    let capability = crate::http::local_capability(&report).overall;

    Ok(SetupProgress {
        downloads,
        thinking: SetupProgressThinking {
            capability: capability.to_string(),
        },
    })
}

/// Setup's speech and thinking download progress. It does not run this
/// computer's capability checks.
pub fn read_setup_downloads(journal: &Path) -> Result<SetupDownloads, SetupProgressError> {
    #[cfg(not(target_os = "windows"))]
    let speech_status =
        status::read_observed_status(journal, "parakeet").map_err(|err| SetupProgressError {
            code: "status_unreadable",
            detail: err.to_string(),
        })?;

    let thinking_status =
        status::read_observed_status(journal, "local").map_err(|err| SetupProgressError {
            code: "status_unreadable",
            detail: err.to_string(),
        })?;

    #[cfg(not(target_os = "windows"))]
    let speech_held =
        solstone_core_local::install::lease::is_held(journal, "parakeet").map_err(|e| {
            SetupProgressError {
                code: "lease_unavailable",
                detail: e.to_string(),
            }
        })?;

    let thinking_held =
        solstone_core_local::install::lease::is_held(journal, "local").map_err(|e| {
            SetupProgressError {
                code: "lease_unavailable",
                detail: e.to_string(),
            }
        })?;

    #[cfg(target_os = "windows")]
    let speech = project_packaged_speech(
        solstone_core_local::install::parakeet_readiness::verified_windows_parakeet_package()
            .map(|_| ()),
    );

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let speech_verified = if speech_held && is_in_flight(&speech_status.install_state) {
        false
    } else {
        let home = std::env::home_dir().unwrap_or_default();
        match solstone_core_journal_config::read_journal_config(journal) {
            Ok(config) => {
                solstone_core_local::install::coreml_install::parakeet_coreml_current_verified(
                    &home, &config,
                )
            }
            Err(_) => false,
        }
    };

    #[cfg(all(
        not(target_os = "windows"),
        not(all(target_os = "macos", target_arch = "aarch64"))
    ))]
    let speech_verified = solstone_core_local::install::parakeet_manifests_ready(
        journal,
        std::env::consts::OS,
        std::env::consts::ARCH,
    );

    #[cfg(not(target_os = "windows"))]
    let speech_catalog_total: Option<u64> = {
        let platform = match std::env::consts::OS {
            "macos" => Some(solstone_core_thinking_copy::Platform::MacosArm64),
            "linux" if std::env::consts::ARCH == "x86_64" => {
                Some(solstone_core_thinking_copy::Platform::LinuxX64)
            }
            "linux" if std::env::consts::ARCH == "aarch64" => {
                Some(solstone_core_thinking_copy::Platform::LinuxArm64)
            }
            _ => None,
        };
        solstone_core_thinking_copy::sum_download_bytes(
            &solstone_core_thinking_copy::speech_catalog_members(platform),
        )
    };

    #[cfg(not(target_os = "windows"))]
    let speech = project_speech(
        Some(&speech_status),
        speech_held,
        speech_verified,
        speech_catalog_total,
    );

    let thinking_verified = solstone_core_local::install::local_thinking_present(journal);
    let thinking_total = solstone_core_local::install::local_thinking_catalog_plan(journal, None)
        .and_then(|rows| {
            let members = rows
                .into_iter()
                .map(|row| {
                    if row.packaged {
                        solstone_core_thinking_copy::DownloadMember::Packaged
                    } else {
                        solstone_core_thinking_copy::DownloadMember::Catalog {
                            unit: row.unit,
                            platform: row.platform,
                            backend: None,
                            filename: row.filename,
                        }
                    }
                })
                .collect::<Vec<_>>();
            solstone_core_thinking_copy::sum_download_bytes(&members)
        });
    let thinking = project_thinking(
        Some(&thinking_status),
        thinking_held,
        thinking_verified,
        thinking_total,
    );

    Ok(SetupDownloads { speech, thinking })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_status(state: &str, attempt_id: Option<&str>) -> InstallStatus {
        InstallStatus {
            schema_version: 1,
            provider: "parakeet".into(),
            revision: 1,
            install_state: state.into(),
            attempt_id: attempt_id.map(str::to_string),
            admission_warning: None,
            target_fingerprint_json: None,
            target_fingerprint_sha256: None,
            started_at: None,
            last_transition_at: None,
            last_progress_at: None,
            completed_at: None,
            progress_bytes_received: None,
            progress_bytes_total: None,
            install_error: None,
            error_code: None,
            owner: None,
        }
    }

    #[test]
    fn test_project_speech_bytes_done_is_number_and_null_bytes_total() {
        let task = project_speech(None, false, false, None);
        assert_eq!(task.state, "idle");
        assert_eq!(task.bytes_done, 0);
        assert_eq!(task.bytes_total, None);
        let json = serde_json::to_value(&task).expect("json");
        assert_eq!(json["bytes_done"], 0);
        assert!(json["bytes_total"].is_null());
    }

    #[test]
    fn test_project_speech_live_lease_wins_over_verified_incumbent() {
        let mut status = dummy_status("downloading", Some("attempt-live"));
        status.progress_bytes_received = Some(1234);
        status.progress_bytes_total = Some(5678);

        let task = project_speech(Some(&status), true, true, Some(9999));
        assert_eq!(task.state, "downloading");
        assert_eq!(task.attempt_id.as_deref(), Some("attempt-live"));
        assert_eq!(task.bytes_done, 1234);
        assert_eq!(task.bytes_total, Some(5678));
        assert!(task.error.is_none());
    }

    #[test]
    fn test_project_speech_failed_current_target_with_older_incumbent_is_error() {
        let mut status = dummy_status("failed", Some("attempt-fail"));
        status.error_code = Some("download_failed".into());
        status.install_error = Some("network timeout".into());

        let task = project_speech(Some(&status), false, false, Some(5000));
        assert_eq!(task.state, "error");
        assert_eq!(task.attempt_id.as_deref(), Some("attempt-fail"));
        let err = task.error.expect("error present");
        assert_eq!(err.code, "download_failed");
        assert_eq!(err.detail, "network timeout");
    }

    #[test]
    fn test_project_speech_verified_current_target_over_stale_failed_is_complete() {
        let mut status = dummy_status("failed", Some("attempt-fail"));
        status.error_code = Some("download_failed".into());

        let task = project_speech(Some(&status), false, true, Some(5000));
        assert_eq!(task.state, "complete");
        assert_eq!(task.bytes_done, 5000);
        assert_eq!(task.bytes_total, Some(5000));
        assert!(task.error.is_none());
    }

    #[test]
    fn test_project_speech_abandoned_observed_status_is_error() {
        let status = dummy_status("downloading", Some("attempt-crashed"));
        let task = project_speech(Some(&status), false, false, Some(5000));
        assert_eq!(task.state, "error");
        assert_eq!(task.attempt_id.as_deref(), Some("attempt-crashed"));
        let err = task.error.expect("error present");
        assert_eq!(err.code, "install_failed");
    }

    #[test]
    fn test_project_speech_install_cancelled_maps_to_cancelled() {
        let mut status = dummy_status("failed", Some("attempt-cancel"));
        status.error_code = Some("install_cancelled".into());
        let task = project_speech(Some(&status), false, false, Some(5000));
        assert_eq!(task.state, "cancelled");
        assert_eq!(task.attempt_id.as_deref(), Some("attempt-cancel"));
        assert!(task.error.is_none());
    }

    #[test]
    fn test_project_speech_unverified_installed_is_error_with_speech_not_verified() {
        let status = dummy_status("installed", Some("attempt-unverified"));
        let task = project_speech(Some(&status), false, false, Some(5000));
        assert_eq!(task.state, "error");
        assert_eq!(task.attempt_id.as_deref(), Some("attempt-unverified"));
        let err = task.error.expect("error present");
        assert_eq!(err.code, "speech_not_verified");
    }

    #[test]
    fn test_project_speech_failed_install_interrupted_is_error() {
        let mut status = dummy_status("failed", Some("attempt-interrupted"));
        status.error_code = Some("install_interrupted".into());
        let task = project_speech(Some(&status), false, false, None);
        assert_eq!(task.state, "error");
        let err = task.error.expect("error present");
        assert_eq!(err.code, "install_interrupted");
    }

    #[test]
    fn test_project_speech_verified_none_observed_is_complete() {
        let task = project_speech(None, false, true, None);
        assert_eq!(task.state, "complete");
        assert_eq!(task.bytes_done, 0);
        assert_eq!(task.bytes_total, None);
        assert!(task.error.is_none());
    }

    #[test]
    fn test_setup_progress_serialization_keys_and_bytes_done_is_number() {
        let progress = SetupProgress {
            downloads: SetupDownloads {
                speech: project_speech(None, false, false, None),
                thinking: project_thinking(None, false, false, None),
            },
            thinking: SetupProgressThinking {
                capability: "ok".into(),
            },
        };
        let val = serde_json::to_value(&progress).expect("serialize");
        let obj = val.as_object().expect("top-level object");
        let mut keys: Vec<_> = obj.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["downloads", "thinking"]);

        let thinking_obj = val["thinking"].as_object().expect("thinking object");
        let thinking_keys: Vec<_> = thinking_obj.keys().cloned().collect();
        assert_eq!(thinking_keys, vec!["capability"]);

        assert!(val["downloads"]["speech"]["bytes_done"].is_number());
        assert!(val["downloads"]["thinking"]["bytes_done"].is_number());
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod full_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn absent_directory_read_setup_progress_creates_nothing() {
        let temp = TempDir::new().expect("tempdir");
        let absent = temp.path().join("nonexistent_journal");
        let progress = read_setup_progress(&absent).expect("progress read succeeds");
        assert!(!absent.exists(), "absent directory must not be created");
        assert_eq!(progress.downloads.thinking.state, "idle");
    }

    #[test]
    fn corrupt_status_read_setup_progress_is_error() {
        let temp = TempDir::new().expect("tempdir");
        let path = status::status_path(temp.path(), "local");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create status dir");
        }
        std::fs::write(&path, b"{ invalid json").expect("write corrupt status");

        let err = read_setup_progress(temp.path()).expect_err("corrupt status must error");
        assert_eq!(err.code, "status_unreadable");
    }
}
