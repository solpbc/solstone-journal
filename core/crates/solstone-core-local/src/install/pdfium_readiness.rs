// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for the PDF worker and its private PDFium DLL.
//!
//! This module verifies the complete installed payload once, then returns the
//! two exact declared members to the process owner. It never launches or loads
//! either member.

use std::path::PathBuf;

use solstone_core_cli_copy::ENGLISH;
#[cfg(windows)]
use solstone_core_cli_copy::MessageDetail;

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

#[cfg(windows)]
#[derive(Clone, Debug)]
enum WindowsPdfiumPackageError {
    ExecutableRead(String),
    ExecutableParent(String),
    ExecutableLocation(String),
    PackageRoot(String),
    PayloadVerify(MessageDetail),
    PayloadRefuses {
        member: &'static str,
        detail: MessageDetail,
    },
}

#[cfg(windows)]
impl WindowsPdfiumPackageError {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::ExecutableRead(error) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.pdfium.windows.executable.read",
                [("error", error)],
            ),
            Self::ExecutableParent(path) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.pdfium.windows.executable.parent",
                [("path", path)],
            ),
            Self::ExecutableLocation(path) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.pdfium.windows.executable.location",
                [("path", path)],
            ),
            Self::PackageRoot(path) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.install.pdfium.windows.package.root",
                [("path", path)],
            ),
            Self::PayloadVerify(detail) => {
                let error = detail.render(locale);
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.pdfium.windows.payload.verify",
                    [("error", &error)],
                )
            }
            Self::PayloadRefuses { member, detail } => {
                let error = detail.render(locale);
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.install.pdfium.windows.payload.refuses",
                    [("member", member), ("error", &error)],
                )
            }
        }
    }
}

/// Resolve PDF worker inputs only from the complete, signed package containing
/// the running journal executable.
pub fn verified_windows_pdfium_package() -> Result<WindowsPdfiumPackage, String> {
    verified_windows_pdfium_package_in(ENGLISH)
}

#[cfg(windows)]
pub fn verified_windows_pdfium_package_in(locale: &str) -> Result<WindowsPdfiumPackage, String> {
    static PACKAGE: OnceLock<Result<WindowsPdfiumPackage, WindowsPdfiumPackageError>> =
        OnceLock::new();
    match PACKAGE.get_or_init(|| {
        let executable = std::env::current_exe()
            .map_err(|error| WindowsPdfiumPackageError::ExecutableRead(error.to_string()))?;
        let bin = executable.parent().ok_or_else(|| {
            WindowsPdfiumPackageError::ExecutableParent(executable.display().to_string())
        })?;
        if bin.file_name() != Some(OsStr::new("bin")) {
            return Err(WindowsPdfiumPackageError::ExecutableLocation(
                executable.display().to_string(),
            ));
        }
        let package_root = bin
            .parent()
            .ok_or_else(|| WindowsPdfiumPackageError::PackageRoot(bin.display().to_string()))?;
        let payload = verify_windows_payload(package_root).map_err(|error| {
            WindowsPdfiumPackageError::PayloadVerify(super::windows_engine::payload_error_detail(
                &error,
            ))
        })?;
        declared_pdfium_members(package_root, &payload)
    }) {
        Ok(package) => Ok(package.clone()),
        Err(error) => Err(error.render(locale)),
    }
}

#[cfg(windows)]
fn declared_pdfium_members(
    package_root: &std::path::Path,
    payload: &VerifiedWindowsPayload,
) -> Result<WindowsPdfiumPackage, WindowsPdfiumPackageError> {
    let worker = payload.pdfium_worker_path().map_err(|error| {
        WindowsPdfiumPackageError::PayloadRefuses {
            member: WINDOWS_PDFIUM_WORKER,
            detail: super::windows_engine::payload_error_detail(&error),
        }
    })?;
    let library = payload.pdfium_library_path().map_err(|error| {
        WindowsPdfiumPackageError::PayloadRefuses {
            member: WINDOWS_PDFIUM_LIBRARY,
            detail: super::windows_engine::payload_error_detail(&error),
        }
    })?;
    Ok(WindowsPdfiumPackage {
        package_root: package_root.to_path_buf(),
        worker,
        library,
    })
}

/// Non-Windows callers cannot establish the installed Windows package scope.
#[cfg(not(windows))]
pub fn verified_windows_pdfium_package_in(locale: &str) -> Result<WindowsPdfiumPackage, String> {
    Err(solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.install.pdfium.windows.runtime",
        &[],
    ))
}
