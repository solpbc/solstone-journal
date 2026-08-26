// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{
    InventoryBudget, JournalRoot, WindowsAcquisitionPrimitive, WindowsInventoryPrimitive,
    enumerate_windows_inventory, read_windows_inventory_file, run_with_windows_acquisition_trace,
    run_with_windows_inventory_trace,
};
use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, GetLastError, INVALID_HANDLE_VALUE, LUID,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_BACKUP_NAME,
    SE_RESTORE_NAME, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenElevation,
    TokenPrivileges,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo,
    GetFileInformationByHandleEx, GetVolumeInformationByHandleW, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const BUDGET: InventoryBudget = InventoryBudget::new(32, 8, 255, 1024, 1024);
const NESTED_MEMBER: &str = "chronicle/20260101/segment/entry.bin";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn io_error(context: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Other,
        format!("{context}: {}", io::Error::last_os_error()),
    )
}

fn current_process_token() -> io::Result<OwnedHandle> {
    let mut raw = std::ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a valid pseudo-handle, and `raw` is writable for the
    // returned token handle.
    #[allow(unsafe_code)]
    let result = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) };
    if result == 0 {
        return Err(io_error("open current process token"));
    }
    if raw.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "OpenProcessToken returned a null token handle",
        ));
    }
    // SAFETY: `raw` is a successful, uniquely owned token handle.
    #[allow(unsafe_code)]
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn lookup_privilege(name: windows_sys::core::PCWSTR) -> io::Result<LUID> {
    let mut luid = LUID::default();
    // SAFETY: `name` is a static NUL-terminated privilege name and `luid` is writable.
    #[allow(unsafe_code)]
    let result = unsafe { LookupPrivilegeValueW(std::ptr::null(), name, &mut luid) };
    if result == 0 {
        return Err(io_error("look up privilege LUID"));
    }
    Ok(luid)
}

fn token_privileges(token: &OwnedHandle) -> io::Result<Vec<LUID_AND_ATTRIBUTES>> {
    let mut required = 0u32;
    // SAFETY: this is the documented sizing call; the token is valid and the null buffer has
    // zero length.
    #[allow(unsafe_code)]
    let sizing = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenPrivileges,
            std::ptr::null_mut(),
            0,
            &mut required,
        )
    };
    // SAFETY: `GetLastError` reads the thread-local error from the sizing call above.
    #[allow(unsafe_code)]
    let last_error = unsafe { GetLastError() };
    if sizing != 0 || last_error != ERROR_INSUFFICIENT_BUFFER || required == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "size TokenPrivileges buffer",
        ));
    }

    let required = usize::try_from(required).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "TokenPrivileges buffer size exceeds address space",
        )
    })?;
    let words = required
        .checked_add(size_of::<usize>() - 1)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "TokenPrivileges size overflow")
        })?
        / size_of::<usize>();
    let mut storage = vec![0usize; words];
    let mut returned = 0u32;
    // SAFETY: `storage` is suitably aligned and at least `required` bytes long, and `returned`
    // is writable for the reported byte count.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenPrivileges,
            storage.as_mut_ptr().cast(),
            required as u32,
            &mut returned,
        )
    };
    if result == 0 {
        return Err(io_error("read TokenPrivileges"));
    }
    let returned = usize::try_from(returned).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "TokenPrivileges returned size exceeds address space",
        )
    })?;
    if returned < size_of::<u32>() || returned > storage.len() * size_of::<usize>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "TokenPrivileges returned an invalid buffer size",
        ));
    }

    let buffer = storage.as_ptr().cast::<u8>();
    // SAFETY: the successful call wrote at least the `PrivilegeCount` DWORD.
    #[allow(unsafe_code)]
    let count = unsafe { buffer.cast::<u32>().read() } as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let privileges_offset = offset_of!(TOKEN_PRIVILEGES, Privileges);
    let available = returned.checked_sub(privileges_offset).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "TokenPrivileges is shorter than its privilege array offset",
        )
    })?;
    let maximum = available / size_of::<LUID_AND_ATTRIBUTES>();
    if count > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "TokenPrivileges privilege count exceeds its returned buffer",
        ));
    }
    // SAFETY: the count was bounded by the returned buffer size, and `storage` supplies the
    // alignment required by `LUID_AND_ATTRIBUTES`.
    #[allow(unsafe_code)]
    let privileges = unsafe {
        std::slice::from_raw_parts(
            buffer.add(privileges_offset).cast::<LUID_AND_ATTRIBUTES>(),
            count,
        )
    };
    Ok(privileges.to_vec())
}

fn same_luid(left: LUID, right: LUID) -> bool {
    left.LowPart == right.LowPart && left.HighPart == right.HighPart
}

fn require_ordinary_owner_token() -> io::Result<()> {
    let token = current_process_token()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: `elevation` is writable for its exact supplied size and `returned` is writable.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    if result == 0 {
        return Err(io_error("read TokenElevation"));
    }
    if elevation.TokenIsElevated != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "ordinary-owner control requires a non-elevated token",
        ));
    }

    let backup = lookup_privilege(SE_BACKUP_NAME)?;
    let restore = lookup_privilege(SE_RESTORE_NAME)?;
    let privileges = token_privileges(&token)?;
    if privileges
        .iter()
        .any(|privilege| same_luid(privilege.Luid, backup))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "ordinary-owner control requires SeBackupPrivilege to be absent from the token",
        ));
    }
    if privileges
        .iter()
        .any(|privilege| same_luid(privilege.Luid, restore))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "ordinary-owner control requires SeRestorePrivilege to be absent from the token",
        ));
    }
    Ok(())
}

fn directory_handle(path: &Path) -> io::Result<OwnedHandle> {
    let path = wide(path.as_os_str());
    // SAFETY: `path` is NUL-terminated, and the returned handle is owned immediately after the
    // invalid-handle sentinel check.
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
    if raw == INVALID_HANDLE_VALUE {
        return Err(io_error("open filesystem-classification directory"));
    }
    // SAFETY: `raw` is a valid uniquely owned handle after the invalid sentinel check.
    #[allow(unsafe_code)]
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn filesystem_name(path: &Path) -> io::Result<String> {
    let handle = directory_handle(path)?;
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: `attributes` is writable for its exact supplied size and `handle` is valid.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io_error("query filesystem-classification attributes"));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem-classification root is a reparse point",
        ));
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem-classification root is not a directory",
        ));
    }

    let mut volume_name = [0u16; 256];
    let mut filesystem_name = [0u16; 256];
    let mut serial = 0;
    let mut maximum_component_length = 0;
    let mut flags = 0;
    // SAFETY: both UTF-16 buffers are writable for their exact supplied lengths.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetVolumeInformationByHandleW(
            handle.as_raw_handle(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            &mut serial,
            &mut maximum_component_length,
            &mut flags,
            filesystem_name.as_mut_ptr(),
            filesystem_name.len() as u32,
        )
    };
    if result == 0 {
        return Err(io_error("query filesystem-classification volume"));
    }
    let terminator = filesystem_name
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "filesystem name is not NUL-terminated",
            )
        })?;
    String::from_utf16(&filesystem_name[..terminator])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "filesystem name is not UTF-16"))
}

fn create_fixture(temporary: tempfile::TempDir) -> Fixture {
    let root = temporary.path().join("journal");
    let nested = root.join(NESTED_MEMBER);
    fs::create_dir_all(nested.parent().expect("nested fixture parent"))
        .expect("create nested fixture directories");
    fs::write(&nested, b"ordinary-owner-inventory-control").expect("write nested fixture file");
    fs::write(root.join("identity.md"), b"ordinary-owner-root-file")
        .expect("write root fixture file");
    Fixture {
        _temporary: temporary,
        root,
    }
}

fn require_trace_primitives(primitives: &[WindowsInventoryPrimitive], context: &str) {
    for primitive in [
        WindowsInventoryPrimitive::BorrowAdmittedRootForListing,
        WindowsInventoryPrimitive::BorrowAdmittedRootForRelativeOpen,
        WindowsInventoryPrimitive::BorrowAdmittedRootForWatch,
        WindowsInventoryPrimitive::WatchArm,
        WindowsInventoryPrimitive::DescendantListingOpen,
        WindowsInventoryPrimitive::DescendantListingIdentityRecheck,
        WindowsInventoryPrimitive::WitnessCancelIoEx,
        WindowsInventoryPrimitive::WitnessDrainCompleted,
    ] {
        assert!(
            primitives.contains(&primitive),
            "{context} did not successfully exercise {primitive:?}"
        );
    }
}

fn exercise_full_flow(path: &Path) {
    let (admission, acquisition_trace) =
        run_with_windows_acquisition_trace(|| JournalRoot::open(path));
    let root = admission.expect("admit ordinary-owner fixture root");
    assert!(
        acquisition_trace
            .successful
            .contains(&WindowsAcquisitionPrimitive::RequestedRootOpen),
        "admission did not successfully request the retained root handle"
    );
    assert!(
        !acquisition_trace.fault_consumed,
        "ordinary-owner admission unexpectedly consumed an injected fault"
    );

    let (result, inventory_trace) = run_with_windows_inventory_trace(|| {
        let inventory = enumerate_windows_inventory(&root, BUDGET)?;
        let entry = inventory
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new(NESTED_MEMBER))
            .expect("find nested inventory entry");
        read_windows_inventory_file(&root, entry, BUDGET)
    });
    assert_eq!(
        result.expect("ordinary-owner inventory and checked read"),
        b"ordinary-owner-inventory-control"
    );
    assert!(
        !inventory_trace.fault_consumed,
        "ordinary-owner inventory unexpectedly consumed an injected fault"
    );
    require_trace_primitives(&inventory_trace.successful, "ordinary-owner inventory");
}

fn exercise_refs_control() {
    let Ok(configured_root) = std::env::var("SOLSTONE_JOURNAL_WIN_REFS_ROOT") else {
        println!("JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=skipped");
        return;
    };
    let configured_root = PathBuf::from(configured_root);
    let filesystem = filesystem_name(&configured_root)
        .expect("classify configured ReFS fixture root without following reparses");
    assert_eq!(
        filesystem, "ReFS",
        "configured ReFS fixture root must be a non-reparse ReFS directory"
    );
    let resolved_root = configured_root
        .canonicalize()
        .expect("resolve configured ReFS fixture root after validation");
    let temporary = tempfile::Builder::new()
        .prefix("solstone-journal-ordinary-owner-")
        .tempdir_in(&resolved_root)
        .expect("create ordinary-owner ReFS fixture");
    let fixture = create_fixture(temporary);
    assert_eq!(
        filesystem_name(&fixture.root).expect("classify ordinary-owner ReFS fixture"),
        "ReFS",
        "ordinary-owner ReFS fixture must remain on ReFS"
    );
    exercise_full_flow(&fixture.root);
    println!("JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed");
    println!(
        "JOURNAL_WIN_CI_ORDINARY_OWNER_REFS_ROOT={}",
        resolved_root.display()
    );
    println!("JOURNAL_WIN_CI_ORDINARY_OWNER_REFS_FILESYSTEM=ReFS");
}

#[test]
fn ordinary_owner_inventory_control() {
    require_ordinary_owner_token()
        .expect("prove ordinary non-elevated token without backup/restore privileges");
    println!("JOURNAL_WIN_CI_ORDINARY_OWNER_TOKEN=passed");

    let fixture = create_fixture(tempfile::tempdir().expect("create ordinary-owner NTFS fixture"));
    assert_eq!(
        filesystem_name(&fixture.root).expect("classify ordinary-owner NTFS fixture"),
        "NTFS",
        "ordinary-owner fixture must be NTFS"
    );
    exercise_full_flow(&fixture.root);
    println!("JOURNAL_WIN_CI_ORDINARY_OWNER_NTFS=passed");

    exercise_refs_control();
    println!("JOURNAL_WIN_CI_ORDINARY_OWNER_CONTROL=passed");
}
