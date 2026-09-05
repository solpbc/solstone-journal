// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for the PDF worker and its private PDFium DLL.
//!
//! This module verifies the complete installed payload once, then returns the
//! two exact declared members to the process owner. It never launches or loads
//! either member.

use std::path::PathBuf;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
use solstone_core_distribution::windows_payload::{
    VerifiedWindowsPayload, WINDOWS_PDFIUM_LIBRARY, WINDOWS_PDFIUM_WORKER, verify_windows_payload,
};

/// The verified package locations required for one Windows PDF import worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsPdfiumPackage {
    pub package_root: PathBuf,
    pub worker: PathBuf,
    pub library: PathBuf,
}

/// Resolve PDF worker inputs only from the complete, signed package containing
/// the running journal executable.
#[cfg(windows)]
pub fn verified_windows_pdfium_package() -> Result<WindowsPdfiumPackage, String> {
    static PACKAGE: OnceLock<Result<WindowsPdfiumPackage, String>> = OnceLock::new();
    match PACKAGE.get_or_init(|| {
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
        let payload = verify_windows_payload(package_root)
            .map_err(|error| format!("could not verify the signed PDF app payload: {error}"))?;
        declared_pdfium_members(package_root, &payload)
    }) {
        Ok(package) => Ok(package.clone()),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(windows)]
fn declared_pdfium_members(
    package_root: &std::path::Path,
    payload: &VerifiedWindowsPayload,
) -> Result<WindowsPdfiumPackage, String> {
    let worker = payload.pdfium_worker_path().map_err(|error| {
        format!("signed PDF app payload does not declare {WINDOWS_PDFIUM_WORKER}: {error}")
    })?;
    let library = payload.pdfium_library_path().map_err(|error| {
        format!("signed PDF app payload does not declare {WINDOWS_PDFIUM_LIBRARY}: {error}")
    })?;
    Ok(WindowsPdfiumPackage {
        package_root: package_root.to_path_buf(),
        worker,
        library,
    })
}

/// Non-Windows callers cannot establish the installed Windows package scope.
#[cfg(not(windows))]
pub fn verified_windows_pdfium_package() -> Result<WindowsPdfiumPackage, String> {
    Err("Windows PDF package verification requires a Windows runtime".to_owned())
}
