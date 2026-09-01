// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows-only DLL search-path restriction and LoadLibraryEx loader.

use std::path::Path;

use crate::{LoadPolicy, PathError, flags_for, resolve_load};

#[derive(Debug)]
pub enum DllLoadError {
    Path(PathError),
    RestrictFailed { win32: u32 },
    LoadFailed(libloading::Error),
}

impl std::fmt::Display for DllLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(error) => error.fmt(formatter),
            Self::RestrictFailed { win32 } => {
                write!(
                    formatter,
                    "unexpected:\n  SetDefaultDllDirectories failed ({win32})"
                )
            }
            Self::LoadFailed(source) => {
                write!(formatter, "unexpected:\n  load failed: {source}")
            }
        }
    }
}

impl std::error::Error for DllLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LoadFailed(source) => Some(source),
            Self::Path(_) | Self::RestrictFailed { .. } => None,
        }
    }
}

impl From<PathError> for DllLoadError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

pub fn restrict_default_dll_directories() -> Result<(), DllLoadError> {
    let flags = flags_for(LoadPolicy::ApplicationDir);
    // SAFETY: SetDefaultDllDirectories is safe to call with a documented flag combination;
    // it has no side effects beyond process-global DLL search-path state.
    let ok = unsafe { windows_sys::Win32::System::LibraryLoader::SetDefaultDllDirectories(flags) };
    if ok == 0 {
        // SAFETY: retrieves the failure from the SetDefaultDllDirectories call above.
        let win32 = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(DllLoadError::RestrictFailed { win32 });
    }
    Ok(())
}

pub fn load_dll(policy: LoadPolicy, path: &Path) -> Result<libloading::Library, DllLoadError> {
    let resolved = resolve_load(policy, path)?;
    // SAFETY: DLL initialization code (DllMain, TLS callbacks) runs as part of this call;
    // caller is trusted to load only vetted product DLLs from validated paths.
    let os = unsafe {
        libloading::os::windows::Library::load_with_flags(&resolved.path, resolved.flags)
    }
    .map_err(DllLoadError::LoadFailed)?;
    Ok(libloading::Library::from(os))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restrict_default_dll_directories_is_idempotent() {
        assert!(restrict_default_dll_directories().is_ok());
        assert!(restrict_default_dll_directories().is_ok());
    }
}
