// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whether this account may use a directory, asked without writing into it.
//!
//! Unix asks `access(2)`. Windows has no mode bits to read, so it asks the
//! object's own access check: open the directory handle requesting exactly the
//! rights in question, and the open succeeds only when the ACL grants them.
//! Nothing is created, and no privilege beyond the owner's own is used.

use std::path::Path;

/// May this account create entries in `path`?
pub(crate) fn writable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        nix::unistd::access(path, nix::unistd::AccessFlags::W_OK).is_ok()
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY};
        windows::grants(path, FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

/// May this account list, create entries in, and traverse `path`?
pub(crate) fn usable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        nix::unistd::access(
            path,
            nix::unistd::AccessFlags::R_OK
                | nix::unistd::AccessFlags::W_OK
                | nix::unistd::AccessFlags::X_OK,
        )
        .is_ok()
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_LIST_DIRECTORY, FILE_TRAVERSE,
        };
        windows::grants(
            path,
            FILE_LIST_DIRECTORY | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | FILE_TRAVERSE,
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

#[cfg(windows)]
mod windows {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

    /// Open `path` as a directory requesting `rights`. A directory handle
    /// needs `FILE_FLAG_BACKUP_SEMANTICS`; without the backup privilege
    /// enabled -- and an ordinary owner has none -- that flag only permits
    /// opening a directory and bypasses nothing. Like `access(2)`, a junction
    /// is followed and its target answers.
    pub(super) fn grants(path: &Path, rights: u32) -> bool {
        if !std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
            return false;
        }
        OpenOptions::new()
            .access_mode(rights)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .is_ok()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{usable, writable};

    #[test]
    fn an_owned_directory_is_usable_and_a_file_or_missing_path_is_not() {
        let directory =
            std::env::temp_dir().join(format!("solstone-doctor-access-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temporary directory");
        assert!(writable(&directory));
        assert!(usable(&directory));
        let file = directory.join("file");
        std::fs::write(&file, b"x").expect("file");
        assert!(!writable(&file));
        assert!(!usable(&file));
        let missing = directory.join("missing");
        assert!(!writable(&missing));
        assert!(!usable(&missing));
        std::fs::remove_dir_all(&directory).expect("cleanup");
    }
}
