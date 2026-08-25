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

pub fn evaluate_rfdetr_readiness(journal: &Path, os: &str, arch: &str) -> RfdetrReadiness {
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
            evaluate_rfdetr_readiness(journal.path(), "windows", "x86_64"),
            RfdetrReadiness::Unsupported { .. }
        ));
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
