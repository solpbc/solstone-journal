// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host verdict for the pinned ced.cpp sound-tagging assets.

use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::sync::OnceLock;

use solstone_core_ced_sys::CedLibrary;

#[cfg(windows)]
use solstone_core_distribution::windows_payload::{
    VerifiedWindowsPayload, WINDOWS_CED_LIBRARY, verify_windows_payload,
};

use super::capability_status::CapabilityStatus;
use super::ced_install::{
    ced_artifact_key, ced_library_path, ced_model_path, ced_uses_package_engine, check_ced_assets,
    check_ced_model, model_artifact,
};
use super::manifest::sha256_file;

/// Owner-facing sentence for a degraded CED verdict, identical on every surface.
pub const CED_UNAVAILABLE_GUIDANCE: &str = "Sound tagging is degraded because its CED assets are unavailable. Transcription will continue. Use `journal install-models` to check or repair the CED assets. If the signed CED app payload is unavailable on Windows, reinstall the journal app.";

/// Short ready detail for `journal check` and `journal health`.
pub const CED_READY_DETAIL: &str = "ced.cpp sound-tag engine and model are ready";

/// Capability identifier carried on every CED-constructed non-ready status.
pub const CED_CAPABILITY: &str = "ced";

/// Result of probing CED assets on a host.
///
/// `os` and `arch` must already be canonical (`linux`/`x86_64`, `linux`/`arm64`,
/// `darwin`/`arm64`). Callers canonicalize at their own boundary.
///
/// `Degraded` is never constructed with [`CapabilityStatus::Ready`]; a ready
/// host is [`CedVerdict::Ready`] directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CedVerdict {
    Ready { library: PathBuf, model: PathBuf },
    Unsupported { os: String, arch: String },
    Degraded(CapabilityStatus),
}

/// Production verdict: catalog model digest, then [`evaluate_ced_readiness_against`].
///
/// `os` and `arch` must already be canonical.
pub fn evaluate_ced_readiness(journal: &Path, os: &str, arch: &str) -> CedVerdict {
    match model_artifact() {
        Ok(artifact) => evaluate_ced_readiness_against(journal, os, arch, artifact.sha256),
        Err(error) => CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid {
            capability: CED_CAPABILITY.to_owned(),
            detail: error.to_string(),
        }),
    }
}

/// Verdict against an explicit model digest.
///
/// Production [`evaluate_ced_readiness`] supplies the catalog sha256. Tests
/// supply a digest that matches a fixture so the load probe can run.
///
/// `os` and `arch` must already be canonical.
pub fn evaluate_ced_readiness_against(
    journal: &Path,
    os: &str,
    arch: &str,
    expected_model_sha256: &str,
) -> CedVerdict {
    if ced_uses_package_engine(os, arch) {
        return probe_windows_package_engine(journal, expected_model_sha256);
    }
    let Some(key) = ced_artifact_key(os, arch) else {
        return CedVerdict::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        };
    };
    match check_ced_assets(journal, os, arch) {
        Ok(None) => CedVerdict::Unsupported {
            os: os.to_owned(),
            arch: arch.to_owned(),
        },
        Err(error) => CedVerdict::Degraded(ced_install_status(&error)),
        Ok(Some(_)) => probe_integrity_and_load(journal, key, expected_model_sha256),
    }
}

fn probe_windows_package_engine(journal: &Path, expected_model_sha256: &str) -> CedVerdict {
    let library = match windows_package_ced_library() {
        Ok(library) => library,
        Err(detail) => {
            return CedVerdict::Degraded(CapabilityStatus::ResourceOrOwnerScopeUnavailable {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            });
        }
    };
    if let Err(error) = check_ced_model(journal) {
        return CedVerdict::Degraded(ced_install_status(&error));
    }
    let model = ced_model_path(journal);
    probe_model_and_library_at_paths(&model, expected_model_sha256, &library)
}

#[cfg(windows)]
fn windows_package_ced_library() -> Result<PathBuf, String> {
    static PAYLOAD: OnceLock<Result<VerifiedWindowsPayload, String>> = OnceLock::new();
    let payload = PAYLOAD.get_or_init(|| {
        let executable = std::env::current_exe().map_err(|error| {
            format!("could not determine the running journal executable: {error}")
        })?;
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
        let root = bin.parent().ok_or_else(|| {
            format!(
                "package bin directory has no package root: {}",
                bin.display()
            )
        })?;
        verify_windows_payload(root)
            .map_err(|error| format!("could not verify the signed CED app payload: {error}"))
    });
    payload
        .as_ref()
        .map_err(Clone::clone)?
        .ced_library_path()
        .map_err(|error| {
            format!("signed CED app payload does not declare {WINDOWS_CED_LIBRARY}: {error}")
        })
}

#[cfg(not(windows))]
fn windows_package_ced_library() -> Result<PathBuf, String> {
    Err("Windows CED package verification requires a Windows runtime".to_owned())
}

fn ced_install_status(error: &super::ced_install::CedInstallError) -> CapabilityStatus {
    match error.reason_code.as_str() {
        "sidecar_missing" | "file_missing" => CapabilityStatus::Absent {
            capability: CED_CAPABILITY.to_owned(),
            detail: error.to_string(),
        },
        _ => CapabilityStatus::IntegrityInvalid {
            capability: CED_CAPABILITY.to_owned(),
            detail: error.to_string(),
        },
    }
}

fn probe_integrity_and_load(journal: &Path, key: &str, expected_model_sha256: &str) -> CedVerdict {
    let model = ced_model_path(journal);
    let library = ced_library_path(journal, key);
    probe_model_and_library_at_paths(&model, expected_model_sha256, &library)
}

fn probe_model_and_library_at_paths(
    model: &Path,
    expected_model_sha256: &str,
    library: &Path,
) -> CedVerdict {
    let actual = match sha256_file(model) {
        Ok(actual) => actual,
        Err(detail) => {
            return CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid {
                capability: CED_CAPABILITY.to_owned(),
                detail,
            });
        }
    };
    if actual != expected_model_sha256 {
        return CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid {
            capability: CED_CAPABILITY.to_owned(),
            detail: format!(
                "sha256 mismatch for {}: expected {expected_model_sha256}, got {actual}",
                model.display()
            ),
        });
    }
    let loaded = match CedLibrary::open(library) {
        Ok(engine) => engine,
        Err(error) => {
            return CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable {
                capability: CED_CAPABILITY.to_owned(),
                detail: error.to_string(),
            });
        }
    };
    if let Err(error) = loaded.load_model(model) {
        return CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable {
            capability: CED_CAPABILITY.to_owned(),
            detail: error.to_string(),
        });
    }
    CedVerdict::Ready {
        library: library.to_path_buf(),
        model: model.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::ced_fixture::{
        compile_load_stub, write_ced_model_bytes, write_complete_ced_install,
    };
    use crate::install::ced_install::{ced_library_path, ced_model_path, model_artifact};

    #[test]
    fn windows_requires_a_verified_package_engine() {
        let journal = tempfile::tempdir().unwrap();
        match evaluate_ced_readiness(journal.path(), "windows", "x86_64") {
            CedVerdict::Degraded(CapabilityStatus::ResourceOrOwnerScopeUnavailable { .. }) => {}
            other => panic!("expected package-engine refusal, got {other:?}"),
        }
        match evaluate_ced_readiness(journal.path(), "macos", "aarch64") {
            CedVerdict::Unsupported { os, arch } => {
                assert_eq!(os, "macos");
                assert_eq!(arch, "aarch64");
            }
            other => panic!("expected unsupported raw macos, got {other:?}"),
        }
    }

    #[test]
    fn absent_sidecar_is_absent() {
        let journal = tempfile::tempdir().unwrap();
        match evaluate_ced_readiness(journal.path(), "linux", "x86_64") {
            CedVerdict::Degraded(CapabilityStatus::Absent { .. }) => {}
            other => panic!("expected absent, got {other:?}"),
        }
    }

    #[test]
    fn size_ok_but_digest_mismatch_is_integrity_invalid() {
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        match evaluate_ced_readiness(journal.path(), "linux", "x86_64") {
            CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid { .. }) => {}
            other => panic!("expected integrity-invalid, got {other:?}"),
        }
    }

    #[test]
    fn against_digest_allows_unloadable_garbage_library() {
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        let digest = sha256_file(&ced_model_path(journal.path())).unwrap();
        match evaluate_ced_readiness_against(journal.path(), "linux", "x86_64", &digest) {
            CedVerdict::Degraded(CapabilityStatus::UnloadableOrUnrunnable { .. }) => {}
            other => panic!("expected unloadable, got {other:?}"),
        }
    }

    #[test]
    fn production_wrapper_reads_catalog() {
        let expected = model_artifact().unwrap().sha256;
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        write_ced_model_bytes(journal.path(), b"not-the-pin").unwrap();
        match evaluate_ced_readiness(journal.path(), "linux", "x86_64") {
            CedVerdict::Degraded(CapabilityStatus::IntegrityInvalid { detail, .. }) => {
                assert!(detail.contains(expected), "{detail}");
            }
            other => panic!("expected catalog-digest mismatch, got {other:?}"),
        }
    }

    #[test]
    fn loadable_stub_is_ready_against_its_own_digest() {
        let journal = tempfile::tempdir().unwrap();
        write_complete_ced_install(journal.path(), "linux-cpu-x64").unwrap();
        let library = ced_library_path(journal.path(), "linux-cpu-x64");
        if !compile_load_stub(&library) {
            return;
        }
        let digest = sha256_file(&ced_model_path(journal.path())).unwrap();
        match evaluate_ced_readiness_against(journal.path(), "linux", "x86_64", &digest) {
            CedVerdict::Ready { .. } => {}
            other => panic!("expected ready, got {other:?}"),
        }
    }
}
