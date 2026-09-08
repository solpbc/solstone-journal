// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for ONNX helper executables, runtime, and models.
//!
//! The owner verifies the complete installed package tree on every call, then returns
//! only the named members required by the two bounded helper consumers. It
//! neither loads the ONNX Runtime DLL nor launches a helper.

#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(any(windows, test))]
use std::ffi::OsStr;

#[cfg(any(windows, test))]
use solstone_core_distribution::windows_payload::{
    VerifiedWindowsPayload, WINDOWS_ONNXRUNTIME_LIBRARY, WINDOWS_PYANNOTE_MODEL,
    WINDOWS_SILERO_VAD_MODEL, WINDOWS_SPEAKERS_ANALYZE_WORKER, WINDOWS_VAD_ANALYZE_WORKER,
    WINDOWS_WESPEAKER_MODEL, verify_windows_payload,
};

/// The complete signed Windows payload required for ONNX helper work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsOnnxPackage {
    pub package_root: PathBuf,
    pub speakers_worker: PathBuf,
    pub vad_worker: PathBuf,
    pub onnxruntime_library: PathBuf,
    pub wespeaker_model: PathBuf,
    pub pyannote_model: PathBuf,
    pub silero_vad_model: PathBuf,
}

/// Resolve ONNX helper inputs only from the complete, signed package
/// containing the running journal executable.
#[cfg(windows)]
pub fn verified_windows_onnx_package() -> Result<WindowsOnnxPackage, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not determine the running journal executable: {error}"))?;
    verified_windows_onnx_package_at(&executable)
}

#[cfg(any(windows, test))]
fn verified_windows_onnx_package_at(executable: &Path) -> Result<WindowsOnnxPackage, String> {
    let (root, payload) = verified_windows_helper_payload_at(executable)?;
    declared_onnx_members(root, &payload)
}

/// Signed speaker command selection for model-free discovery clustering.
#[cfg(windows)]
pub fn verified_windows_speakers_helper() -> Result<(PathBuf, PathBuf), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not determine the running journal executable: {error}"))?;
    verified_windows_speakers_helper_at(&executable)
}

#[cfg(any(windows, all(test, feature = "test-fixture-pin")))]
fn verified_windows_speakers_helper_at(executable: &Path) -> Result<(PathBuf, PathBuf), String> {
    let (root, payload) = verified_windows_helper_payload_at(executable)?;
    let worker = payload.speakers_analyze_worker_path().map_err(|error| {
        format!(
            "signed ONNX app payload does not declare {WINDOWS_SPEAKERS_ANALYZE_WORKER}: {error}"
        )
    })?;
    Ok((root.to_path_buf(), worker))
}

#[cfg(any(windows, test))]
fn verified_windows_helper_payload_at(
    executable: &Path,
) -> Result<(&Path, VerifiedWindowsPayload), String> {
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
        .map_err(|error| format!("could not verify the signed ONNX app payload: {error}"))?;
    Ok((package_root, payload))
}

#[cfg(any(windows, test))]
fn declared_onnx_members(
    package_root: &std::path::Path,
    payload: &VerifiedWindowsPayload,
) -> Result<WindowsOnnxPackage, String> {
    Ok(WindowsOnnxPackage {
        package_root: package_root.to_path_buf(),
        speakers_worker: payload.speakers_analyze_worker_path().map_err(|error| {
            format!(
                "signed ONNX app payload does not declare {WINDOWS_SPEAKERS_ANALYZE_WORKER}: {error}"
            )
        })?,
        vad_worker: payload.vad_analyze_worker_path().map_err(|error| {
            format!("signed ONNX app payload does not declare {WINDOWS_VAD_ANALYZE_WORKER}: {error}")
        })?,
        onnxruntime_library: payload.onnxruntime_library_path().map_err(|error| {
            format!("signed ONNX app payload does not declare {WINDOWS_ONNXRUNTIME_LIBRARY}: {error}")
        })?,
        wespeaker_model: payload.wespeaker_model_path().map_err(|error| {
            format!("signed ONNX app payload does not declare {WINDOWS_WESPEAKER_MODEL}: {error}")
        })?,
        pyannote_model: payload.pyannote_model_path().map_err(|error| {
            format!("signed ONNX app payload does not declare {WINDOWS_PYANNOTE_MODEL}: {error}")
        })?,
        silero_vad_model: payload.silero_vad_model_path().map_err(|error| {
            format!("signed ONNX app payload does not declare {WINDOWS_SILERO_VAD_MODEL}: {error}")
        })?,
    })
}

/// Non-Windows callers cannot establish the installed Windows package scope.
#[cfg(not(windows))]
pub fn verified_windows_onnx_package() -> Result<WindowsOnnxPackage, String> {
    Err("Windows ONNX package verification requires a Windows runtime".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_refuses_wrong_layout_and_missing_manifest() {
        let root = tempfile::tempdir().unwrap();
        assert!(verified_windows_onnx_package_at(&root.path().join("journal.exe")).is_err());
        assert!(verified_windows_onnx_package_at(&root.path().join("bin/journal.exe")).is_err());
    }

    #[cfg(feature = "test-fixture-pin")]
    #[test]
    fn signed_onnx_package_rechecks_runtime_models_and_helpers() {
        use solstone_core_distribution::windows_payload::{
            WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, render_windows_payload_manifest,
        };
        let root = tempfile::tempdir().unwrap();
        let members = [
            "bin/journal.exe",
            WINDOWS_SPEAKERS_ANALYZE_WORKER,
            WINDOWS_VAD_ANALYZE_WORKER,
            WINDOWS_ONNXRUNTIME_LIBRARY,
            WINDOWS_WESPEAKER_MODEL,
            WINDOWS_PYANNOTE_MODEL,
            WINDOWS_SILERO_VAD_MODEL,
        ];
        for member in members {
            let path = root.path().join(member);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, member.as_bytes()).unwrap();
        }
        let manifest =
            render_windows_payload_manifest(root.path(), &"a".repeat(40), &"b".repeat(64)).unwrap();
        let minisign::KeyPair { pk, sk } = super::super::windows_payload_test_keys();
        let signature = minisign::sign(
            Some(pk),
            sk,
            std::io::Cursor::new(&manifest),
            None,
            Some("ONNX consumer fixture"),
        )
        .unwrap();
        let manifest_path = root.path().join(WINDOWS_PAYLOAD_MANIFEST);
        std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        std::fs::write(&manifest_path, manifest).unwrap();
        std::fs::write(
            root.path().join(WINDOWS_PAYLOAD_SIGNATURE),
            signature.into_string(),
        )
        .unwrap();
        let executable = root.path().join("bin/journal.exe");
        let package = verified_windows_onnx_package_at(&executable).unwrap();
        assert_eq!(
            package.vad_worker,
            root.path().join(WINDOWS_VAD_ANALYZE_WORKER)
        );
        assert_eq!(
            package.onnxruntime_library,
            root.path().join(WINDOWS_ONNXRUNTIME_LIBRARY)
        );
        for member in members {
            let path = root.path().join(member);
            std::fs::write(&path, b"poisoned").unwrap();
            assert!(
                verified_windows_onnx_package_at(&executable).is_err(),
                "tampered {member}"
            );
            std::fs::write(&path, member.as_bytes()).unwrap();
            assert!(
                verified_windows_onnx_package_at(&executable).is_ok(),
                "restored {member}"
            );
            std::fs::remove_file(&path).unwrap();
            assert!(
                verified_windows_onnx_package_at(&executable).is_err(),
                "missing {member}"
            );
            std::fs::write(&path, member.as_bytes()).unwrap();
        }
    }
    #[cfg(feature = "test-fixture-pin")]
    #[test]
    fn signed_discovery_helper_selection_does_not_require_model_declarations() {
        use solstone_core_distribution::windows_payload::{
            WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE, render_windows_payload_manifest,
        };
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("bin")).unwrap();
        let executable = root.path().join("bin/journal.exe");
        let worker = root.path().join(WINDOWS_SPEAKERS_ANALYZE_WORKER);
        std::fs::write(&executable, b"fixture caller").unwrap();
        std::fs::write(&worker, b"fixture helper").unwrap();
        let manifest =
            render_windows_payload_manifest(root.path(), &"a".repeat(40), &"b".repeat(64)).unwrap();
        let minisign::KeyPair { pk, sk } = super::super::windows_payload_test_keys();
        let signature = minisign::sign(
            Some(pk),
            sk,
            std::io::Cursor::new(&manifest),
            None,
            Some("discovery fixture"),
        )
        .unwrap();
        let manifest_path = root.path().join(WINDOWS_PAYLOAD_MANIFEST);
        std::fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        std::fs::write(&manifest_path, manifest).unwrap();
        std::fs::write(
            root.path().join(WINDOWS_PAYLOAD_SIGNATURE),
            signature.into_string(),
        )
        .unwrap();
        assert_eq!(
            verified_windows_speakers_helper_at(&executable).unwrap(),
            (root.path().to_path_buf(), worker.clone())
        );
        assert!(verified_windows_onnx_package_at(&executable).is_err());
        std::fs::write(worker, b"tampered").unwrap();
        assert!(verified_windows_speakers_helper_at(&executable).is_err());
    }
}
