// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(windows)]
pub fn resolve_package_bin_and_root() -> io::Result<(PathBuf, PathBuf)> {
    let exe = env::current_exe()?;
    let bin_dir = exe.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "current executable has no parent directory",
        )
    })?;
    if bin_dir.file_name() != Some(std::ffi::OsStr::new("bin")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "current executable is not in a bin directory",
        ));
    }
    let package_root = bin_dir.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "package root directory not found")
    })?;
    Ok((bin_dir.to_path_buf(), package_root.to_path_buf()))
}

#[cfg(windows)]
pub fn verify_package_and_get_tool(subpath: &str) -> io::Result<PathBuf> {
    let (_, package_root) = resolve_package_bin_and_root()?;
    let payload =
        solstone_core_distribution::windows_payload::verify_windows_payload(&package_root)
            .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, error.to_string()))?;
    payload.declared_path(subpath).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "backup tool is not declared in admitted payload",
        )
    })
}
