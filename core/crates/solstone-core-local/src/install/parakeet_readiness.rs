// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Signed Windows package binding for the Parakeet server and bundled model.
//!
//! This boundary verifies the whole installed payload before returning the
//! package-owned executable and model. It neither starts a provider nor writes
//! a model copy into the journal's mutable cache.

use std::path::PathBuf;

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
use solstone_core_distribution::windows_payload::{
    VerifiedWindowsPayload, WINDOWS_PARAKEET_MODEL, WINDOWS_PARAKEET_SERVER, verify_windows_payload,
};

/// The signed Windows package members that a Parakeet provider owner may use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsParakeetPackage {
    pub package_root: PathBuf,
    pub server: PathBuf,
    pub model: PathBuf,
}

pub fn verified_windows_parakeet_package() -> Result<WindowsParakeetPackage, String> {
    verified_windows_parakeet_package_in(solstone_core_cli_copy::ENGLISH)
}

/// Resolve Parakeet only from the complete signed package containing the
/// running journal executable.
#[cfg(windows)]
pub fn verified_windows_parakeet_package_in(
    locale: &str,
) -> Result<WindowsParakeetPackage, String> {
    static PACKAGE: OnceLock<Result<WindowsParakeetPackage, solstone_core_cli_copy::Diagnostic>> =
        OnceLock::new();
    match PACKAGE.get_or_init(|| {
        let executable = std::env::current_exe().map_err(|error| {
            solstone_core_cli_copy::Diagnostic::new(
                "journal.cli.doctor.windows.parakeet.executable",
                [("error", &error)],
            )
        })?;
        let bin = executable.parent().ok_or_else(|| {
            solstone_core_cli_copy::Diagnostic::new(
                "journal.cli.doctor.windows.parakeet.parent",
                [("path", &executable.display())],
            )
        })?;
        if bin.file_name() != Some(OsStr::new("bin")) {
            return Err(solstone_core_cli_copy::Diagnostic::new(
                "journal.cli.doctor.windows.parakeet.layout",
                [("path", &executable.display())],
            ));
        }
        let package_root = bin.parent().ok_or_else(|| {
            solstone_core_cli_copy::Diagnostic::new(
                "journal.cli.doctor.windows.parakeet.root",
                [("path", &bin.display())],
            )
        })?;
        let payload = verify_windows_payload(package_root).map_err(|error| {
            solstone_core_cli_copy::Diagnostic::new(
                "journal.cli.doctor.windows.parakeet.verification",
                [("error", &error)],
            )
        })?;
        declared_parakeet_members(package_root, &payload)
    }) {
        Ok(package) => Ok(package.clone()),
        Err(error) => Err(error.render(locale)),
    }
}

#[cfg(windows)]
fn declared_parakeet_members(
    package_root: &std::path::Path,
    payload: &VerifiedWindowsPayload,
) -> Result<WindowsParakeetPackage, solstone_core_cli_copy::Diagnostic> {
    Ok(WindowsParakeetPackage {
        package_root: package_root.to_path_buf(),
        server: payload.parakeet_server_path().map_err(|error| {
            solstone_core_cli_copy::Diagnostic::new(
                "journal.cli.doctor.windows.parakeet.member",
                [("member", &WINDOWS_PARAKEET_SERVER), ("error", &error)],
            )
        })?,
        model: payload.parakeet_model_path().map_err(|error| {
            solstone_core_cli_copy::Diagnostic::new(
                "journal.cli.doctor.windows.parakeet.member",
                [("member", &WINDOWS_PARAKEET_MODEL), ("error", &error)],
            )
        })?,
    })
}

/// Non-Windows callers cannot establish the installed Windows package scope.
#[cfg(not(windows))]
pub fn verified_windows_parakeet_package_in(
    locale: &str,
) -> Result<WindowsParakeetPackage, String> {
    Err(solstone_core_cli_copy::render_for(
        locale,
        "journal.cli.doctor.windows.parakeet.platform",
        &[],
    ))
}
