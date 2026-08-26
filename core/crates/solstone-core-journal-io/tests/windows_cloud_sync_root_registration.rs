// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::collections::BTreeMap;
use std::fs;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::{Path, PathBuf};

use solstone_core_journal_io::journal_root::WindowsRefusalCategory;
use solstone_core_journal_io::{JournalRoot, JournalRootError, ObjectIdentity};
use windows_sys::Win32::Foundation::{
    ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT, ERROR_INVALID_FUNCTION, ERROR_NOT_A_CLOUD_FILE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::CloudFilters::{
    CF_HARDLINK_POLICY_NONE, CF_HYDRATION_POLICY, CF_HYDRATION_POLICY_FULL,
    CF_HYDRATION_POLICY_MODIFIER_NONE, CF_INSYNC_POLICY_NONE,
    CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT, CF_POPULATION_POLICY,
    CF_POPULATION_POLICY_ALWAYS_FULL, CF_POPULATION_POLICY_MODIFIER_NONE, CF_REGISTER_FLAG_NONE,
    CF_SYNC_POLICIES, CF_SYNC_REGISTRATION, CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC,
    CfGetSyncRootInfoByPath, CfRegisterSyncRoot, CfUnregisterSyncRoot,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo,
    GetFileInformationByHandleEx, GetVolumeInformationByHandleW, OPEN_EXISTING,
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

#[derive(Debug, Eq, PartialEq)]
struct EntrySnapshot {
    volume_serial: u64,
    file_id: [u8; 16],
    readonly: bool,
    bytes: Option<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
struct TreeSnapshot {
    entries: BTreeMap<PathBuf, EntrySnapshot>,
}

fn path_identity(path: &Path) -> (u64, [u8; 16]) {
    let path = wide(path);
    // SAFETY: `path` is NUL-terminated, and the returned handle is immediately owned on success.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE, "open fixture identity handle");
    // SAFETY: `raw` is a valid uniquely-owned handle after the invalid sentinel check.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `info` is writable for the exact supplied structure size.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    assert_ne!(result, 0, "query fixture identity");
    (info.VolumeSerialNumber, info.FileId.Identifier)
}

fn cloud_sync_result(path: &Path) -> i32 {
    let path = wide(path);
    let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
    let mut returned = 0;
    // SAFETY: `path` is NUL-terminated and `info` is writable for its exact size.
    #[allow(unsafe_code)]
    unsafe {
        CfGetSyncRootInfoByPath(
            path.as_ptr(),
            CF_SYNC_ROOT_INFO_BASIC,
            (&mut info as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
            size_of::<CF_SYNC_ROOT_BASIC_INFO>() as u32,
            &mut returned,
        )
    }
}

const fn hresult_from_win32(error: u32) -> i32 {
    (0x8007_0000 | (error & 0x0000_FFFF)) as i32
}

fn is_explicit_nonmembership(result: i32) -> bool {
    [
        ERROR_INVALID_FUNCTION,
        ERROR_NOT_A_CLOUD_FILE,
        ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT,
    ]
    .into_iter()
    .map(hresult_from_win32)
    .any(|candidate| result == candidate)
}

fn capture_tree(root: &Path) -> TreeSnapshot {
    fn capture_entry(root: &Path, relative: &Path, entries: &mut BTreeMap<PathBuf, EntrySnapshot>) {
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path).expect("read fixture metadata");
        let is_directory = metadata.is_dir();
        let (volume_serial, file_id) = path_identity(&path);
        entries.insert(
            relative.to_path_buf(),
            EntrySnapshot {
                volume_serial,
                file_id,
                readonly: metadata.permissions().readonly(),
                bytes: metadata
                    .is_file()
                    .then(|| fs::read(&path).expect("read sentinel")),
            },
        );
        if is_directory {
            let mut children = fs::read_dir(&path)
                .expect("list fixture directory")
                .map(|entry| entry.expect("read fixture entry").file_name())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                capture_entry(root, &relative.join(child), entries);
            }
        }
    }

    let mut entries = BTreeMap::new();
    capture_entry(root, Path::new(""), &mut entries);
    TreeSnapshot { entries }
}

fn filesystem_name(root: &JournalRoot) -> String {
    let mut volume_name = [0u16; 256];
    let mut filesystem_name = [0u16; 256];
    let mut serial = 0;
    let mut maximum_component_length = 0;
    let mut flags = 0;
    // SAFETY: both UTF-16 buffers are writable for their exact supplied lengths.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetVolumeInformationByHandleW(
            root.as_handle().as_raw_handle(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            &mut serial,
            &mut maximum_component_length,
            &mut flags,
            filesystem_name.as_mut_ptr(),
            filesystem_name.len() as u32,
        )
    };
    assert_ne!(result, 0, "query fixture filesystem");
    let terminator = filesystem_name
        .iter()
        .position(|unit| *unit == 0)
        .expect("filesystem name terminator");
    String::from_utf16(&filesystem_name[..terminator]).expect("filesystem name is UTF-16")
}

fn register_sync_root(path: &Path) -> RegisteredSyncRoot {
    let path = wide(path);
    let provider_name: Vec<u16> = "solstone journal test"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let provider_version: Vec<u16> = "1.0.0".encode_utf16().chain(Some(0)).collect();
    let file_identity = b"solstone-journal-test-root";
    let registration = CF_SYNC_REGISTRATION {
        StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: provider_name.as_ptr(),
        ProviderVersion: provider_version.as_ptr(),
        SyncRootIdentity: b"solstone-journal-test".as_ptr().cast(),
        SyncRootIdentityLength: b"solstone-journal-test".len() as u32,
        FileIdentity: file_identity.as_ptr().cast(),
        FileIdentityLength: file_identity.len() as u32,
        ProviderId: windows_sys::core::GUID::from_u128(0x7f1cf64a_24d4_4c85_a662_5243c4360f86),
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY {
            Primary: CF_HYDRATION_POLICY_FULL,
            Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
        },
        Population: CF_POPULATION_POLICY {
            Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
            Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
        },
        InSync: CF_INSYNC_POLICY_NONE,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    // SAFETY: all registration pointers remain valid for this synchronous call.
    #[allow(unsafe_code)]
    let result = unsafe {
        CfRegisterSyncRoot(
            path.as_ptr(),
            &registration,
            &policies,
            CF_REGISTER_FLAG_NONE,
        )
    };
    let registered = RegisteredSyncRoot {
        path,
        active: result == 0,
    };
    assert_eq!(result, 0, "register temporary Cloud Files sync root");
    registered
}

fn admitted_identity(path: &Path) -> ObjectIdentity {
    JournalRoot::open(path)
        .expect("admit unregistered fixture path")
        .identity()
}

fn assert_registered_refusal(result: Result<JournalRoot, JournalRootError>) {
    assert!(matches!(
        result,
        Err(JournalRootError::Unsupported {
            category: Some(WindowsRefusalCategory::CloudSyncRootRegistered),
            ..
        })
    ));
}

#[test]
fn registered_cloud_sync_root_and_ordinary_child_are_refused_without_mutation() {
    let temporary = tempfile::tempdir().expect("temporary sync root");
    let child = temporary.path().join("ordinary-child");
    fs::create_dir(&child).expect("create ordinary child");
    let sentinel = child.join("sentinel.bin");
    fs::write(&sentinel, b"cloud-registration-no-write-oracle")
        .expect("write sentinel before registration");

    let raw_before = (
        cloud_sync_result(temporary.path()),
        cloud_sync_result(&child),
    );
    assert!(
        is_explicit_nonmembership(raw_before.0) && is_explicit_nonmembership(raw_before.1),
        "ordinary NTFS controls must return explicit nonmembership: {raw_before:?}"
    );
    let raw_registered = register_sync_root(temporary.path());
    let raw_during = catch_unwind(AssertUnwindSafe(|| {
        (
            cloud_sync_result(temporary.path()),
            cloud_sync_result(&child),
        )
    }));
    let raw_unregistered = raw_registered.unregister();
    assert_eq!(raw_unregistered, 0, "unregister raw Cloud Files probe");
    let raw_during = match raw_during {
        Ok(result) => result,
        Err(payload) => resume_unwind(payload),
    };
    assert_eq!(
        raw_during,
        (0, 0),
        "registered root and child must return S_OK"
    );
    let raw_after = (
        cloud_sync_result(temporary.path()),
        cloud_sync_result(&child),
    );
    assert!(
        is_explicit_nonmembership(raw_after.0) && is_explicit_nonmembership(raw_after.1),
        "unregister must restore explicit nonmembership: {raw_after:?}"
    );

    let before = JournalRoot::open(temporary.path()).expect("admit unregistered control root");
    assert_eq!(
        filesystem_name(&before),
        "NTFS",
        "native fixture must be NTFS"
    );
    let before_root_identity = before.identity();
    drop(before);
    let before_child_identity = admitted_identity(&child);
    let before_tree = capture_tree(temporary.path());

    let registered = register_sync_root(temporary.path());
    let during = catch_unwind(AssertUnwindSafe(|| {
        let immediately_before = capture_tree(temporary.path());
        let root_admission = JournalRoot::open(temporary.path());
        let child_admission = JournalRoot::open(&child);
        let immediately_after = capture_tree(temporary.path());
        (
            immediately_before,
            root_admission,
            child_admission,
            immediately_after,
        )
    }));
    let unregistered = registered.unregister();
    assert_eq!(
        unregistered, 0,
        "unregister temporary Cloud Files sync root"
    );
    let (immediately_before, root_admission, child_admission, immediately_after) = match during {
        Ok(result) => result,
        Err(payload) => resume_unwind(payload),
    };

    assert_registered_refusal(root_admission);
    assert_registered_refusal(child_admission);
    assert_eq!(immediately_after, immediately_before);

    let after = JournalRoot::open(temporary.path()).expect("admit unregistered control root");
    assert_eq!(after.identity(), before_root_identity);
    assert_eq!(admitted_identity(&child), before_child_identity);
    assert_eq!(capture_tree(temporary.path()), before_tree);

    registered_cloud_sync_root_guard_cleans_up_during_unwind();
}

fn registered_cloud_sync_root_guard_cleans_up_during_unwind() {
    let temporary = tempfile::tempdir().expect("temporary sync root");
    let child = temporary.path().join("ordinary-child");
    fs::create_dir(&child).expect("create ordinary child");
    let before_root_identity = admitted_identity(temporary.path());
    let before_child_identity = admitted_identity(&child);

    let registered = register_sync_root(temporary.path());
    let unwind = catch_unwind(AssertUnwindSafe(move || {
        let _registered = registered;
        panic!("forced unwind after Cloud Files registration");
    }));
    assert!(unwind.is_err(), "forced unwind must be observed");
    assert_eq!(admitted_identity(temporary.path()), before_root_identity);
    assert_eq!(admitted_identity(&child), before_child_identity);
}
