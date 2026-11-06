// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows: the journal root's DACL, and the one pass that makes it private.
//!
//! A root that already reaches only this account, SYSTEM and Administrators is
//! never rewritten, which is the usual profile location. Any other root gets a
//! protected owner-only DACL, after one walk resets whatever under it keeps its
//! own broad grant, because Windows passes that grant over when it propagates.
//! The root is written last, so an interrupted pass runs again at next start.
//!
//! Reparse points are skipped and never followed: a junction's target isn't the
//! journal's to change. A broad file moved in later keeps its permissions, and
//! reading it takes its exact path, since the folders above it can't be listed.
#![allow(unsafe_code)]

use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACE_HEADER, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorDacl,
    InitializeAcl, PROTECTED_DACL_SECURITY_INFORMATION, PSID,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, GetVolumeInformationW, GetVolumePathNameW,
};
use windows_sys::Win32::System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE};

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// Applies to whoever creates a child, which here is the journal itself.
const CREATOR_OWNER: &str = "S-1-3-0";
const OWNER_RIGHTS: &str = "S-1-3-4";

const FILE_READ_DATA: u32 = 0x0000_0001;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_READ: u32 = 0x8000_0000;

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Whether a SID is one that may read a private journal.
fn may_read(sid: &str, owner: &str) -> bool {
    sid == owner || matches!(sid, SYSTEM | ADMINISTRATORS | CREATOR_OWNER | OWNER_RIGHTS)
}

fn grants_read(mask: u32) -> bool {
    mask & (FILE_READ_DATA | GENERIC_READ | GENERIC_ALL) != 0
}

fn sid_text(sid: PSID) -> io::Result<String> {
    let mut text = null_mut();
    // SAFETY: sid points into a live ACE, and text points to writable storage.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a NUL-terminated UTF-16 LocalAlloc buffer, released once after the copy.
    let result = unsafe {
        let length = (0..).take_while(|index| *text.add(*index) != 0).count();
        let copied = String::from_utf16(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        copied
    };
    result.map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "SID is not UTF-16"))
}

/// Whether anything but this account, SYSTEM and Administrators can read `path`.
fn reaches_others(path: &Path, owner: &str) -> io::Result<bool> {
    let name = wide(path);
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor = null_mut();
    // SAFETY: name is NUL-terminated; dacl and descriptor point to writable storage.
    let result = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let scanned = scan_dacl(dacl, owner);
    // SAFETY: GetNamedSecurityInfoW allocated the descriptor with LocalAlloc; dacl points into
    // it and isn't used past this point.
    unsafe {
        LocalFree(descriptor);
    }
    scanned
}

fn scan_dacl(dacl: *const ACL, owner: &str) -> io::Result<bool> {
    // A null DACL grants everyone everything.
    if dacl.is_null() {
        return Ok(true);
    }
    // SAFETY: dacl is non-null and valid while its descriptor lives, which outlasts this call.
    let count = unsafe { (*dacl).AceCount };
    for index in 0..u32::from(count) {
        let mut ace = null_mut();
        // SAFETY: index is below AceCount, and ace points to writable storage.
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetAce returned a valid ACE, which starts with an ACE_HEADER.
        let kind = u32::from(unsafe { (*ace.cast::<ACE_HEADER>()).AceType });
        if kind == ACCESS_DENIED_ACE_TYPE {
            continue;
        }
        // Any other kind of grant is one this check can't read, so it counts as broad.
        if kind != ACCESS_ALLOWED_ACE_TYPE {
            return Ok(true);
        }
        let body = ace.cast::<u8>();
        // SAFETY: an ACCESS_ALLOWED_ACE is a 4-byte header, a 4-byte mask, then the SID.
        let (mask, sid) = unsafe {
            (
                body.add(size_of::<ACE_HEADER>())
                    .cast::<u32>()
                    .read_unaligned(),
                body.add(size_of::<ACE_HEADER>() + size_of::<u32>()).cast(),
            )
        };
        if grants_read(mask) && !may_read(&sid_text(sid)?, owner) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// True when only this account, SYSTEM and Administrators can read the root.
pub(super) fn root_is_private(root: &Path) -> io::Result<bool> {
    let owner = solstone_core_local_endpoint::owner_sid()?;
    Ok(!reaches_others(root, &owner)?)
}

/// Drop what `path` was granted of its own, so it holds only what it inherits.
fn reset_to_inherited(path: &Path) -> io::Result<()> {
    // InitializeAcl needs room for the ACL header, and an ACL has no alignment beyond u32.
    let mut storage = [0_u32; 4];
    let acl = storage.as_mut_ptr().cast::<ACL>();
    // SAFETY: storage holds more than an empty ACL and outlives both calls.
    if unsafe { InitializeAcl(acl, size_of::<[u32; 4]>() as u32, ACL_REVISION) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let name = wide(path);
    // SAFETY: name is NUL-terminated and acl is initialized; the other pointers are optional.
    let result = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        )
    };
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result as i32))
    }
}

fn set_private_root(root: &Path, owner: &str) -> io::Result<()> {
    let sddl = format!("D:P(A;OICI;FA;;;{owner})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
    let text: Vec<u16> = std::ffi::OsStr::new(&sddl)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut descriptor = null_mut();
    // SAFETY: text is NUL-terminated, and descriptor points to writable storage.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let (mut present, mut defaulted) = (0, 0);
    let mut dacl: *mut ACL = null_mut();
    // SAFETY: descriptor is valid until freed below, and the outputs are writable.
    let read =
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) };
    let result = if read == 0 || present == 0 {
        Err(io::Error::last_os_error())
    } else {
        let name = wide(root);
        // SAFETY: name is NUL-terminated; dacl lives in descriptor, which outlives this call.
        match unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null_mut(),
            )
        } {
            ERROR_SUCCESS => Ok(()),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    };
    // SAFETY: allocated by ConvertString...W with LocalAlloc, and released once here.
    unsafe {
        LocalFree(descriptor);
    }
    result
}

/// Reset every child below `directory` that keeps a grant of its own. One file
/// that can't be changed doesn't stop the rest. Returns how many were left.
fn reset_descendants(directory: &Path, owner: &str) -> usize {
    let mut left = 0;
    let mut pending = vec![directory.to_owned()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            left += 1;
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                left += 1;
                continue;
            };
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path.clone());
            }
            match reaches_others(&path, owner) {
                Ok(false) => {}
                Ok(true) => {
                    if let Err(error) = reset_to_inherited(&path) {
                        log::debug!("journal privacy: {}: {error}", path.display());
                        left += 1;
                    }
                }
                Err(error) => {
                    log::debug!("journal privacy: {}: {error}", path.display());
                    left += 1;
                }
            }
        }
    }
    left
}

/// The one pass: every descendant, then the root. Returns descendants left broad.
pub(super) fn make_private(root: &Path) -> io::Result<usize> {
    let owner = solstone_core_local_endpoint::owner_sid()?;
    let root = std::fs::canonicalize(root)?;
    let left = reset_descendants(&root, &owner);
    set_private_root(&root, &owner)?;
    Ok(left)
}

/// The file system's name, which a storage consent is bound to.
pub(super) fn filesystem_type(root: &Path) -> io::Result<String> {
    let name = wide(root);
    let mut volume = vec![0_u16; 1024];
    // SAFETY: name is NUL-terminated, and volume holds the length passed.
    if unsafe { GetVolumePathNameW(name.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut filesystem = [0_u16; 64];
    // SAFETY: volume is NUL-terminated, the buffer's length is passed, and the rest are optional.
    if unsafe {
        GetVolumeInformationW(
            volume.as_ptr(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let length = filesystem.iter().position(|unit| *unit == 0).unwrap_or(0);
    Ok(String::from_utf16_lossy(&filesystem[..length]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adds a grant for Everyone directly on `path`, protected from its parent.
    fn make_broad(path: &Path) {
        let sddl: Vec<u16> = std::ffi::OsStr::new("D:P(A;OICI;FA;;;WD)")
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut descriptor = null_mut();
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = null_mut();
        let name = wide(path);
        // SAFETY: test-only; each pointer is valid for its call, and the descriptor is
        // intentionally left allocated until the test process exits.
        unsafe {
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut(),
                ),
                0
            );
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted);
            assert_eq!(
                SetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    dacl,
                    null_mut(),
                ),
                ERROR_SUCCESS
            );
        }
    }

    #[test]
    fn the_pass_reaches_protected_children_and_leaves_a_private_root() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let protected = folder.join("protected.txt");
        std::fs::write(&protected, "x").unwrap();
        make_broad(root.path());
        make_broad(&protected);
        assert!(!root_is_private(root.path()).unwrap());
        assert_eq!(make_private(root.path()).unwrap(), 0);
        let owner = solstone_core_local_endpoint::owner_sid().unwrap();
        assert!(root_is_private(root.path()).unwrap());
        assert!(!reaches_others(&protected, &owner).unwrap());
        // The owner can still read what they own.
        assert_eq!(std::fs::read_to_string(&protected).unwrap(), "x");
    }

    #[test]
    fn who_may_read_a_private_journal() {
        for sid in [
            SYSTEM,
            ADMINISTRATORS,
            CREATOR_OWNER,
            OWNER_RIGHTS,
            "S-1-5-21-1-2-3-1000",
        ] {
            assert!(may_read(sid, "S-1-5-21-1-2-3-1000"));
        }
        for sid in ["S-1-1-0", "S-1-5-11", "S-1-5-32-545", "S-1-5-21-1-2-3-1001"] {
            assert!(!may_read(sid, "S-1-5-21-1-2-3-1000"));
        }
    }

    #[test]
    fn only_read_rights_count_as_reaching() {
        assert!(grants_read(FILE_READ_DATA));
        assert!(grants_read(GENERIC_READ));
        assert!(grants_read(GENERIC_ALL));
        // Traverse alone reads nothing.
        assert!(!grants_read(0x0000_0020));
    }

    #[test]
    fn the_file_system_has_a_name() {
        let root = tempfile::tempdir().unwrap();
        assert!(!filesystem_type(root.path()).unwrap().is_empty());
    }
}
