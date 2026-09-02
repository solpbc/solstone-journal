// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Portable Windows DLL-load policy: path validation and LoadLibraryEx flag mapping.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{DllLoadError, load_dll, restrict_default_dll_directories};

use std::path::{Component, Path, PathBuf};

const LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: u32 = 0x0000_0100;
const LOAD_LIBRARY_SEARCH_APPLICATION_DIR: u32 = 0x0000_0200;
const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadPolicy {
    ApplicationDir,
    DllLoadDir,
}

#[must_use]
pub const fn flags_for(policy: LoadPolicy) -> u32 {
    match policy {
        LoadPolicy::ApplicationDir => {
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32
        }
        LoadPolicy::DllLoadDir => LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLoad {
    pub path: PathBuf,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathError {
    NotAbsolute,
    TraversalComponent,
}

impl std::fmt::Display for PathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAbsolute => formatter.write_str("unexpected:\n  path not absolute"),
            Self::TraversalComponent => {
                formatter.write_str("unexpected:\n  path contains a traversal component")
            }
        }
    }
}

impl std::error::Error for PathError {}

pub fn resolve_load(policy: LoadPolicy, path: &Path) -> Result<ResolvedLoad, PathError> {
    if !path.is_absolute() {
        return Err(PathError::NotAbsolute);
    }
    if has_traversal_component(path) {
        return Err(PathError::TraversalComponent);
    }
    Ok(ResolvedLoad {
        path: path.to_path_buf(),
        flags: flags_for(policy),
    })
}

fn has_traversal_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        || has_current_dir_component(path)
}

/// `Path::components` skips `.` except when it is the only remaining component, so
/// `/opt/app/./x.dll` would otherwise look clean. Scan the encoded path for a
/// standalone `.` component bounded by separators or string edges.
fn has_current_dir_component(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    let is_separator = |byte: u8| byte == b'/' || byte == b'\\';
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'.'
            && (index == 0 || is_separator(bytes[index - 1]))
            && (index + 1 == bytes.len() || is_separator(bytes[index + 1]))
        {
            return true;
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_dir_flags_are_application_dir_and_system32() {
        assert_eq!(flags_for(LoadPolicy::ApplicationDir), 0x0A00);
    }

    #[test]
    fn dll_load_dir_flags_are_dll_load_dir_and_system32() {
        assert_eq!(flags_for(LoadPolicy::DllLoadDir), 0x0900);
    }

    #[test]
    fn relative_path_is_not_absolute() {
        assert_eq!(
            resolve_load(LoadPolicy::ApplicationDir, Path::new("foo/bar.dll")),
            Err(PathError::NotAbsolute)
        );
    }

    #[test]
    fn bare_filename_is_not_absolute() {
        assert_eq!(
            resolve_load(LoadPolicy::ApplicationDir, Path::new("bar.dll")),
            Err(PathError::NotAbsolute)
        );
    }

    #[test]
    fn parent_dir_component_is_traversal() {
        assert_eq!(
            resolve_load(LoadPolicy::ApplicationDir, Path::new("/opt/../lib/x.dll")),
            Err(PathError::TraversalComponent)
        );
    }

    #[test]
    fn cur_dir_component_is_traversal() {
        assert_eq!(
            resolve_load(LoadPolicy::ApplicationDir, Path::new("/opt/app/./x.dll")),
            Err(PathError::TraversalComponent)
        );
    }

    #[test]
    fn clean_absolute_path_carries_application_dir_flags() {
        assert_eq!(
            resolve_load(LoadPolicy::ApplicationDir, Path::new("/opt/app/x.dll")),
            Ok(ResolvedLoad {
                path: PathBuf::from("/opt/app/x.dll"),
                flags: 0x0A00,
            })
        );
    }
}
