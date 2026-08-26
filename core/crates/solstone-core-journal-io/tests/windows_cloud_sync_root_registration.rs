// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;

use solstone_core_journal_io::journal_root::WindowsRefusalCategory;
use solstone_core_journal_io::{JournalRoot, JournalRootError};
use windows_sys::Win32::Storage::CloudFilters::{
    CF_REGISTER_FLAG_NONE, CF_SYNC_POLICIES, CF_SYNC_REGISTRATION, CfRegisterSyncRoot,
    CfUnregisterSyncRoot,
};

fn wide(path: &std::path::Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

struct RegisteredSyncRoot {
    path: Vec<u16>,
    active: bool,
}

impl RegisteredSyncRoot {
    fn unregister(mut self) -> i32 {
        // SAFETY: `path` remains NUL-terminated for this synchronous `CfUnregisterSyncRoot` cleanup call.
        #[allow(unsafe_code)]
        let result = unsafe { CfUnregisterSyncRoot(self.path.as_ptr()) };
        if result == 0 {
            self.active = false;
        }
        result
    }
}

impl Drop for RegisteredSyncRoot {
    fn drop(&mut self) {
        if self.active {
            // SAFETY: `path` remains NUL-terminated through the guard's lifetime for `CfUnregisterSyncRoot`.
            #[allow(unsafe_code)]
            unsafe {
                let _ = CfUnregisterSyncRoot(self.path.as_ptr());
            }
        }
    }
}

#[test]
fn registered_cloud_sync_root_is_refused_and_unregistered() {
    let temporary = tempfile::tempdir().expect("temporary sync root");
    let path = wide(temporary.path());
    let before = JournalRoot::open(temporary.path()).expect("admit unregistered control root");
    let before_identity = before.identity();
    let before_children = std::fs::read_dir(temporary.path())
        .expect("list control root")
        .map(|entry| entry.expect("control entry").file_name())
        .collect::<Vec<_>>();
    let before_permissions = std::fs::metadata(temporary.path())
        .expect("control metadata")
        .permissions()
        .readonly();
    drop(before);
    let provider_name: Vec<u16> = "solstone journal test"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let registration = CF_SYNC_REGISTRATION {
        StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: provider_name.as_ptr(),
        ProviderVersion: std::ptr::null(),
        SyncRootIdentity: b"solstone-journal-test".as_ptr().cast(),
        SyncRootIdentityLength: b"solstone-journal-test".len() as u32,
        FileIdentity: std::ptr::null(),
        FileIdentityLength: 0,
        ProviderId: windows_sys::core::GUID::from_u128(0x7f1cf64a_24d4_4c85_a662_5243c4360f86),
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
        ..Default::default()
    };
    // SAFETY: all registration pointers remain valid for this synchronous `CfRegisterSyncRoot` call.
    #[allow(unsafe_code)]
    let registered = unsafe {
        CfRegisterSyncRoot(
            path.as_ptr(),
            &registration,
            &policies,
            CF_REGISTER_FLAG_NONE,
        )
    };
    assert_eq!(registered, 0, "register temporary Cloud Files sync root");

    let registered = RegisteredSyncRoot { path, active: true };

    let admission = JournalRoot::open(temporary.path());
    let unregistered = registered.unregister();
    assert_eq!(
        unregistered, 0,
        "unregister temporary Cloud Files sync root"
    );
    assert!(matches!(
        admission,
        Err(JournalRootError::Unsupported {
            category: Some(WindowsRefusalCategory::CloudSyncRootRegistered),
            ..
        })
    ));
    let after = JournalRoot::open(temporary.path()).expect("admit unregistered control root");
    let after_children = std::fs::read_dir(temporary.path())
        .expect("list restored control root")
        .map(|entry| entry.expect("control entry").file_name())
        .collect::<Vec<_>>();
    let after_permissions = std::fs::metadata(temporary.path())
        .expect("restored control metadata")
        .permissions()
        .readonly();
    assert_eq!(after.identity(), before_identity);
    assert_eq!(after_children, before_children);
    assert_eq!(after_permissions, before_permissions);
}
