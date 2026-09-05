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

/// Resolve Parakeet only from the complete signed package containing the
/// running journal executable.
#[cfg(windows)]
pub fn verified_windows_parakeet_package() -> Result<WindowsParakeetPackage, String> {
    static PACKAGE: OnceLock<Result<WindowsParakeetPackage, String>> = OnceLock::new();
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
        let payload = verify_windows_payload(package_root).map_err(|error| {
            format!("could not verify the signed Parakeet app payload: {error}")
        })?;
        declared_parakeet_members(package_root, &payload)
    }) {
        Ok(package) => Ok(package.clone()),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(windows)]
fn declared_parakeet_members(
    package_root: &std::path::Path,
    payload: &VerifiedWindowsPayload,
) -> Result<WindowsParakeetPackage, String> {
    Ok(WindowsParakeetPackage {
        package_root: package_root.to_path_buf(),
        server: payload.parakeet_server_path().map_err(|error| {
            format!(
                "signed Parakeet app payload does not declare {WINDOWS_PARAKEET_SERVER}: {error}"
            )
        })?,
        model: payload.parakeet_model_path().map_err(|error| {
            format!(
                "signed Parakeet app payload does not declare {WINDOWS_PARAKEET_MODEL}: {error}"
            )
        })?,
    })
}

/// Non-Windows callers cannot establish the installed Windows package scope.
#[cfg(not(windows))]
pub fn verified_windows_parakeet_package() -> Result<WindowsParakeetPackage, String> {
    Err("Windows Parakeet package verification requires a Windows runtime".to_owned())
}
