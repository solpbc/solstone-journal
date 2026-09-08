// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Operator-facing readiness verdict for bundled rf-detr.cpp assets.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::readiness::probe_binary_with_arg;
use super::rfdetr_install::{
    RfdetrInstallError, RfdetrInstallRecord, binary_path, check_rfdetr_model, model_path,
    rfdetr_artifact_key,
};

pub use super::rfdetr_windows::{
    RFDETR_PACKAGE_UNAVAILABLE_GUIDANCE, RfdetrWindowsLaunchSpec, WindowsRfdetrPackage,
    map_rfdetr_detect_completion, rfdetr_degraded_guidance, rfdetr_windows_detect_launch,
    rfdetr_windows_help_launch, verified_windows_rfdetr_package,
};

pub const RFDETR_READY_DETAIL: &str = "rf-detr.cpp object-detection engine and model are ready";
pub const RFDETR_UNAVAILABLE_GUIDANCE: &str = "Object detection is degraded because its RF-DETR assets are unavailable. Screen descriptions will continue. Use `journal install-models` to check or repair the RF-DETR assets.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfdetrDegradedCause {
    Absent,
    IntegrityInvalid,
    Unrunnable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RfdetrReadiness {
    Ready {
        binary: PathBuf,
        model: PathBuf,
    },
    Unsupported {
        os: String,
        arch: String,
    },
    Degraded {
        cause: RfdetrDegradedCause,
        detail: String,
    },
}

pub fn evaluate_windows_rfdetr_readiness_from(
    package: Result<WindowsRfdetrPackage, super::rfdetr_windows::WindowsRfdetrPackageError>,
    probe: impl FnOnce(&WindowsRfdetrPackage) -> Value,
) -> RfdetrReadiness {
    let package = match package {
        Ok(package) => package,
        Err(error) => {
            let cause = match &error {
                super::rfdetr_windows::WindowsRfdetrPackageError::Missing(_) => {
                    RfdetrDegradedCause::Absent
                }
                super::rfdetr_windows::WindowsRfdetrPackageError::Invalid(_) => {
                    RfdetrDegradedCause::IntegrityInvalid
                }
            };
            return RfdetrReadiness::Degraded {
                cause,
                detail: format!("Windows RF-DETR package verification failed: {error}"),
            };
        }
    };
    let result = probe(&package);
    if result
        .get("runnable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        RfdetrReadiness::Ready {
            binary: package.binary,
            model: package.model,
        }
    } else {
        RfdetrReadiness::Degraded {
            cause: RfdetrDegradedCause::Unrunnable,
            detail: format!("rf-detr engine probe failed: {result}"),
        }
    }
}

pub fn evaluate_windows_rfdetr_readiness(
    probe: impl FnOnce(&WindowsRfdetrPackage) -> Value,
) -> RfdetrReadiness {
    evaluate_windows_rfdetr_readiness_from(verified_windows_rfdetr_package(), probe)
}

pub fn evaluate_rfdetr_readiness(journal: &Path, os: &str, arch: &str) -> RfdetrReadiness {
    if super::rfdetr_install::rfdetr_uses_package_payload(os, arch) {
        return evaluate_windows_rfdetr_readiness_from(
            verified_windows_rfdetr_package(),
            |_| json!({"runnable": false, "reason_code": "bounded_helper_probe_required"}),
        );
    }
    let key = rfdetr_artifact_key(os, arch);
    let checked = match key {
        Some(_) => check_rfdetr_model(journal, os, arch),
        None => Ok(RfdetrInstallRecord::PlatformUnavailable),
    };
    evaluate_rfdetr_readiness_from(journal, os, arch, key, checked, probe_rfdetr)
}

fn evaluate_rfdetr_readiness_from(
    journal: &Path,
    os: &str,
    arch: &str,
    key: Option<&str>,
    checked: Result<RfdetrInstallRecord, RfdetrInstallError>,
    probe: impl FnOnce(&Path) -> Value,
) -> RfdetrReadiness {
    let Some(key) = key else {
        return RfdetrReadiness::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        };
    };
    match checked {
        Ok(RfdetrInstallRecord::PlatformUnavailable) => RfdetrReadiness::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        },
        Err(error) => RfdetrReadiness::Degraded {
            cause: cause_for(&error),
            detail: error.to_string(),
        },
        Ok(RfdetrInstallRecord::Installed) => {
            let binary = binary_path(journal, key);
            let model = model_path(journal);
            let result = probe(&binary);
            if result["runnable"] == json!(true) {
                RfdetrReadiness::Ready { binary, model }
            } else {
                RfdetrReadiness::Degraded {
                    cause: RfdetrDegradedCause::Unrunnable,
                    detail: format!("rf-detr engine probe failed: {result}"),
                }
            }
        }
    }
}

fn cause_for(error: &RfdetrInstallError) -> RfdetrDegradedCause {
    match error.reason_code.as_str() {
        "sidecar_missing" | "file_missing" => RfdetrDegradedCause::Absent,
        _ => RfdetrDegradedCause::IntegrityInvalid,
    }
}

fn probe_rfdetr(path: &Path) -> Value {
    probe_binary_with_arg(path.to_string_lossy().as_ref(), "--help")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::rfdetr_install::{
        EngineSpec, ModelSpec, check_rfdetr_model_with_artifacts,
    };
    use sha2::{Digest, Sha256};

    #[test]
    fn unsupported_platform_is_unsupported() {
        let journal = tempfile::tempdir().unwrap();
        assert!(matches!(
            evaluate_rfdetr_readiness(journal.path(), "windows", "arm64"),
            RfdetrReadiness::Unsupported { .. }
        ));
    }

    #[test]
    fn windows_package_readiness_injected_ok_is_ready() {
        let package = Ok(WindowsRfdetrPackage {
            package_root: PathBuf::from("/pkg"),
            binary: PathBuf::from("/pkg/bin/rfdetr-cli.exe"),
            model: PathBuf::from(
                "/pkg/lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf",
            ),
        });
        let readiness =
            evaluate_windows_rfdetr_readiness_from(package, |_| json!({"runnable": true}));
        assert_eq!(
            readiness,
            RfdetrReadiness::Ready {
                binary: PathBuf::from("/pkg/bin/rfdetr-cli.exe"),
                model: PathBuf::from(
                    "/pkg/lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf"
                ),
            }
        );
    }

    #[test]
    fn windows_package_readiness_refuses_corrupt_payload_before_probe() {
        let package: Result<
            WindowsRfdetrPackage,
            super::super::rfdetr_windows::WindowsRfdetrPackageError,
        > = Err(
            super::super::rfdetr_windows::WindowsRfdetrPackageError::Invalid(
                "signed RF-DETR app payload digest mismatch".to_owned(),
            ),
        );
        let readiness = evaluate_windows_rfdetr_readiness_from(package, |_| {
            unreachable!("must not probe corrupt payload")
        });
        assert_eq!(
            readiness,
            RfdetrReadiness::Degraded {
                cause: RfdetrDegradedCause::IntegrityInvalid,
                detail: "Windows RF-DETR package verification failed: signed RF-DETR app payload digest mismatch".to_owned(),
            }
        );
    }

    #[test]
    fn windows_package_readiness_unrunnable_probe() {
        let package = Ok(WindowsRfdetrPackage {
            package_root: PathBuf::from("/pkg"),
            binary: PathBuf::from("/pkg/bin/rfdetr-cli.exe"),
            model: PathBuf::from(
                "/pkg/lib/solstone_journal_models/assets/rfdetr/rfdetr-nano-f16.gguf",
            ),
        });
        let readiness = evaluate_windows_rfdetr_readiness_from(
            package,
            |_| json!({"runnable": false, "reason_code": "binary_exit", "exit_code": 1}),
        );
        assert!(matches!(
            readiness,
            RfdetrReadiness::Degraded {
                cause: RfdetrDegradedCause::Unrunnable,
                ..
            }
        ));
    }

    #[test]
    fn windows_launch_spec_construction_and_budgets() {
        let package = WindowsRfdetrPackage {
            package_root: PathBuf::from(r"C:\Program Files\Solstone"),
            binary: PathBuf::from(r"C:\Program Files\Solstone\bin\rfdetr-cli.exe"),
            model: PathBuf::from(
                r"C:\Program Files\Solstone\lib\solstone_journal_models\assets\rfdetr\rfdetr-nano-f16.gguf",
            ),
        };
        let help_spec =
            rfdetr_windows_help_launch(&package, std::ffi::OsString::from(r"C:\Windows"));
        assert_eq!(
            help_spec.package_root,
            PathBuf::from(r"C:\Program Files\Solstone")
        );
        assert_eq!(
            help_spec.executable,
            PathBuf::from(r"C:\Program Files\Solstone\bin\rfdetr-cli.exe")
        );
        assert_eq!(
            help_spec.current_directory,
            package.package_root.join("bin")
        );
        assert_eq!(help_spec.arguments, vec!["--help"]);
        assert_eq!(help_spec.environment.len(), 1);
        assert_eq!(
            help_spec
                .environment
                .get(&std::ffi::OsString::from("SystemRoot")),
            Some(&std::ffi::OsString::from(r"C:\Windows"))
        );
        assert!(help_spec.stdin.is_empty());
        assert_eq!(help_spec.timeout, std::time::Duration::from_secs(10));
        assert_eq!(help_spec.stdin_limit_bytes, 1);
        assert_eq!(help_spec.stdout_limit_bytes, 64 * 1024);
        assert_eq!(help_spec.stderr_limit_bytes, 64 * 1024);
        assert_eq!(help_spec.cpu_rate_per_10_000, 10000);
        assert_eq!(help_spec.committed_memory_bytes, 2 * 1024 * 1024 * 1024);

        let input_path = PathBuf::from(r"C:\temp\input.jpg");
        let output_path = PathBuf::from(r"C:\temp\output.json");
        let detect_spec = rfdetr_windows_detect_launch(
            &package,
            std::ffi::OsString::from(r"C:\Windows"),
            &input_path,
            &output_path,
        );
        assert_eq!(
            detect_spec.arguments,
            vec![
                "detect",
                "--model",
                r"C:\Program Files\Solstone\lib\solstone_journal_models\assets\rfdetr\rfdetr-nano-f16.gguf",
                "--input",
                r"C:\temp\input.jpg",
                "--output",
                r"C:\temp\output.json",
                "--threshold",
                "0.25",
                "--threads",
                "4",
            ]
        );
        assert_eq!(detect_spec.timeout, std::time::Duration::from_secs(120));
        assert_eq!(detect_spec.committed_memory_bytes, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn missing_sidecar_is_absent() {
        let journal = tempfile::tempdir().unwrap();
        assert!(matches!(
            evaluate_rfdetr_readiness(journal.path(), "linux", "x86_64"),
            RfdetrReadiness::Degraded {
                cause: RfdetrDegradedCause::Absent,
                ..
            }
        ));
    }

    #[test]
    fn digest_failure_is_integrity_invalid() {
        let journal = tempfile::tempdir().unwrap();
        let error = RfdetrInstallError::new("sha256_mismatch", "model mismatch", 65);
        let verdict = evaluate_rfdetr_readiness_from(
            journal.path(),
            "linux",
            "x86_64",
            Some("linux-cpu-x64"),
            Err(error),
            |_| unreachable!("a failed verification must not launch the binary"),
        );
        assert!(matches!(
            verdict,
            RfdetrReadiness::Degraded {
                cause: RfdetrDegradedCause::IntegrityInvalid,
                ..
            }
        ));
    }

    #[test]
    fn launch_failure_is_unrunnable_after_byte_verification() {
        let journal = tempfile::tempdir().unwrap();
        let key = "linux-cpu-x64";
        let binary = binary_path(journal.path(), key);
        let binary_bytes = b"not an executable";
        let model_bytes = b"model";
        let binary_sha256 =
            Box::leak(format!("{:x}", Sha256::digest(binary_bytes)).into_boxed_str());
        let model_sha256 = Box::leak(format!("{:x}", Sha256::digest(model_bytes)).into_boxed_str());
        let engine = EngineSpec {
            filename: "fixture.tar.gz",
            tarball_sha256: binary_sha256,
            tarball_size: binary_bytes.len() as u64,
            binary_sha256,
            expected_member_path: None,
        };
        let model = ModelSpec {
            sha256: model_sha256,
            size: model_bytes.len() as u64,
        };
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, binary_bytes).unwrap();
        let model_path = model_path(journal.path());
        std::fs::create_dir_all(model_path.parent().unwrap()).unwrap();
        std::fs::write(&model_path, model_bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let sidecar = journal
            .path()
            .join("cache/providers/rfdetr/.rfdetr-install.json");
        std::fs::write(
            &sidecar,
            serde_json::json!({
                "artifact_key": key,
                "engine_version": "v0.1.0-solpbc.5",
                "engine_sha256": binary_sha256,
                "model_file": "rfdetr-nano-f16.gguf",
                "model_repo": "mudler/rfdetr-cpp-nano",
                "model_revision": "c3dc0c037df499f5503545247df6618415fca643",
                "model_sha256": model_sha256,
                "status": "installed",
            })
            .to_string(),
        )
        .unwrap();
        let verdict = evaluate_rfdetr_readiness_from(
            journal.path(),
            "linux",
            "x86_64",
            Some(key),
            check_rfdetr_model_with_artifacts(journal.path(), key, &engine, &model),
            probe_rfdetr,
        );
        assert!(matches!(
            verdict,
            RfdetrReadiness::Degraded {
                cause: RfdetrDegradedCause::Unrunnable,
                ..
            }
        ));
    }
}
