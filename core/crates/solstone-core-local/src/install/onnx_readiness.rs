// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for ONNX helper executables, runtime, and models.
//!
//! The owner verifies the complete installed package tree once, then returns
//! only the named members required by the two bounded helper consumers. It
//! neither loads the ONNX Runtime DLL nor launches a helper.

use std::path::PathBuf;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
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
    static PACKAGE: OnceLock<Result<WindowsOnnxPackage, String>> = OnceLock::new();
    PACKAGE
        .get_or_init(|| {
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
            let package_root = bin.parent().ok_or_else(|| {
                format!(
                    "package bin directory has no package root: {}",
                    bin.display()
                )
            })?;
            let payload = verify_windows_payload(package_root).map_err(|error| {
                format!("could not verify the signed ONNX app payload: {error}")
            })?;
            declared_onnx_members(package_root, &payload)
        })
        .as_ref()
        .map(Clone::clone)
}

#[cfg(windows)]
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

    #[cfg(not(windows))]
    #[test]
    fn non_windows_refuses_the_windows_package_boundary() {
        assert!(verified_windows_onnx_package().is_err());
    }
}
