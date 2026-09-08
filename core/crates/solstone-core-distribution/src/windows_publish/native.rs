// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The only unsafe producer leaf: a same-parent, local, no-replace directory move.

use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, Prefix};

pub(super) fn move_no_replace(stage: &Path, destination: &Path) -> Result<(), String> {
    for path in [stage, destination] {
        if !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
            || !path.is_absolute()
        {
            return Err("Windows producer publication requires absolute local drive paths".into());
        }
    }
    if stage.parent() != destination.parent() {
        return Err("Windows producer publication requires sibling directories".into());
    }
    let metadata = std::fs::symlink_metadata(stage).map_err(|e| format!("inspect stage: {e}"))?;
    if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
        return Err("Windows producer stage must be a regular non-reparse directory".into());
    }
    let wide = |path: &Path| -> Result<Vec<u16>, String> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err("Windows publication path contains NUL".into());
        }
        value.push(0);
        Ok(value)
    };
    let stage = wide(stage)?;
    let destination = wide(destination)?;
    // SAFETY: both vectors retain complete NUL-terminated UTF-16 paths through
    // the synchronous call. Zero flags excludes replacement, copy fallback,
    // reboot scheduling, and any unsupported directory durability assertion.
    let moved = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            stage.as_ptr(),
            destination.as_ptr(),
            0,
        )
    };
    if moved == 0 {
        return Err(format!(
            "MoveFileExW(no replacement): {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
