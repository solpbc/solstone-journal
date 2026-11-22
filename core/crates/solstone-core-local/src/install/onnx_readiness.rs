// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for ONNX helper executables, runtime, and models.
//!
//! The owner verifies the complete installed package tree on every call, then returns
//! only the named members required by speaker/VAD helpers and their runtime,
//! also used by describe for face inference. Face model digests are verified by
//! the model-asset resolver. It
//! neither loads the ONNX Runtime DLL nor launches a helper.

use solstone_core_cli_copy::MessageDetail;
#[cfg(any(windows, test))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(any(windows, test))]
use std::ffi::OsStr;

#[cfg(any(windows, test))]
use solstone_core_distribution::windows_payload::{
    VerifiedWindowsPayload, WINDOWS_FIRERED_VAD_MODEL, WINDOWS_ONNXRUNTIME_LIBRARY,
    WINDOWS_PYANNOTE_MODEL, WINDOWS_SPEAKERS_ANALYZE_WORKER, WINDOWS_VAD_ANALYZE_WORKER,
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
    pub firered_vad_model: PathBuf,
}

pub fn verified_windows_onnx_package() -> Result<WindowsOnnxPackage, String> {
    verified_windows_onnx_package_in(solstone_core_cli_copy::ENGLISH)
}

pub fn verified_windows_onnx_package_in(locale: &str) -> Result<WindowsOnnxPackage, String> {
    verified_windows_onnx_package_detail().map_err(|detail| detail.render(locale))
}

/// Resolve ONNX helper inputs only from the complete, signed package
/// containing the running journal executable.
#[cfg(windows)]
pub fn verified_windows_onnx_package_detail() -> Result<WindowsOnnxPackage, MessageDetail> {
    let executable = std::env::current_exe().map_err(|error| {
        MessageDetail::message(
            "journal.cli.doctor.windows.parakeet.executable",
            [("error", &error)],
        )
    })?;
    verified_windows_onnx_package_at(&executable)
}

#[cfg(any(windows, test))]
fn verified_windows_onnx_package_at(
    executable: &Path,
) -> Result<WindowsOnnxPackage, MessageDetail> {
    let (root, payload) = verified_windows_helper_payload_at(executable)?;
    declared_onnx_members(root, &payload)
}

/// Signed speaker command selection for model-free discovery clustering.
#[cfg(windows)]
pub fn verified_windows_speakers_helper() -> Result<(PathBuf, PathBuf), String> {
    let executable = std::env::current_exe().map_err(|error| {
        solstone_core_cli_copy::display_for(
            solstone_core_cli_copy::ENGLISH,
            "journal.cli.doctor.windows.parakeet.executable",
            [("error", &error)],
        )
    })?;
    verified_windows_speakers_helper_at(&executable)
}

#[cfg(any(windows, all(test, feature = "test-fixture-pin")))]
fn verified_windows_speakers_helper_at(executable: &Path) -> Result<(PathBuf, PathBuf), String> {
    let (root, payload) =
        verified_windows_helper_payload_at(executable).map_err(|detail| detail.to_string())?;
    let worker = payload.speakers_analyze_worker_path().map_err(|error| {
        solstone_core_cli_copy::display_for(
            solstone_core_cli_copy::ENGLISH,
            "journal.cli.doctor.windows.onnx.member",
            [
                ("member", &(WINDOWS_SPEAKERS_ANALYZE_WORKER)),
                ("error", &(error)),
            ],
        )
    })?;
    Ok((root.to_path_buf(), worker))
}

#[cfg(any(windows, test))]
fn verified_windows_helper_payload_at(
    executable: &Path,
) -> Result<(&Path, VerifiedWindowsPayload), MessageDetail> {
    let bin = executable.parent().ok_or_else(|| {
        MessageDetail::message(
            "journal.cli.doctor.windows.parakeet.parent",
            [("path", &executable.display())],
        )
    })?;
    if bin.file_name() != Some(OsStr::new("bin")) {
        return Err(MessageDetail::message(
            "journal.cli.doctor.windows.parakeet.layout",
            [("path", &executable.display())],
        ));
    }
    let package_root = bin.parent().ok_or_else(|| {
        MessageDetail::message(
            "journal.cli.doctor.windows.parakeet.root",
            [("path", &bin.display())],
        )
    })?;
    let payload = verify_windows_payload(package_root).map_err(|error| {
        MessageDetail::message(
            "journal.cli.doctor.windows.onnx.verification",
            [("error", &(error))],
        )
    })?;
    Ok((package_root, payload))
}

#[cfg(any(windows, test))]
fn declared_onnx_members(
    package_root: &std::path::Path,
    payload: &VerifiedWindowsPayload,
) -> Result<WindowsOnnxPackage, MessageDetail> {
    Ok(WindowsOnnxPackage {
        package_root: package_root.to_path_buf(),
        speakers_worker: payload.speakers_analyze_worker_path().map_err(|error| {
            MessageDetail::message(
                "journal.cli.doctor.windows.onnx.member",
                [
                    ("member", &(WINDOWS_SPEAKERS_ANALYZE_WORKER)),
                    ("error", &(error)),
                ],
            )
        })?,
        vad_worker: payload.vad_analyze_worker_path().map_err(|error| {
            MessageDetail::message(
                "journal.cli.doctor.windows.onnx.member",
                [
                    ("member", &(WINDOWS_VAD_ANALYZE_WORKER)),
                    ("error", &(error)),
                ],
            )
        })?,
        onnxruntime_library: payload.onnxruntime_library_path().map_err(|error| {
            MessageDetail::message(
                "journal.cli.doctor.windows.onnx.member",
                [
                    ("member", &(WINDOWS_ONNXRUNTIME_LIBRARY)),
                    ("error", &(error)),
                ],
            )
        })?,
        wespeaker_model: payload.wespeaker_model_path().map_err(|error| {
            MessageDetail::message(
                "journal.cli.doctor.windows.onnx.member",
                [("member", &(WINDOWS_WESPEAKER_MODEL)), ("error", &(error))],
            )
        })?,
        pyannote_model: payload.pyannote_model_path().map_err(|error| {
            MessageDetail::message(
                "journal.cli.doctor.windows.onnx.member",
                [("member", &(WINDOWS_PYANNOTE_MODEL)), ("error", &(error))],
            )
        })?,
        firered_vad_model: payload.firered_vad_model_path().map_err(|error| {
            MessageDetail::message(
                "journal.cli.doctor.windows.onnx.member",
                [
                    ("member", &(WINDOWS_FIRERED_VAD_MODEL)),
                    ("error", &(error)),
                ],
            )
        })?,
    })
}

/// Non-Windows callers cannot establish the installed Windows package scope.
#[cfg(not(windows))]
pub fn verified_windows_onnx_package_detail() -> Result<WindowsOnnxPackage, MessageDetail> {
    Err(MessageDetail::message(
        "journal.cli.doctor.windows.onnx.platform",
        [],
    ))
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
            WINDOWS_FIRERED_VAD_MODEL,
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
