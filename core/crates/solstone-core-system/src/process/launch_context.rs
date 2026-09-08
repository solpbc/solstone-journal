// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;

/// Child metadata and the retained resources separately authorized for its launch.
#[derive(Clone, Debug, Default)]
pub struct ChildLaunchContext {
    pub environment: BTreeMap<OsString, OsString>,
    #[cfg(windows)]
    pub read_file_grants: Vec<ReadFileGrant>,
}

#[cfg(windows)]
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum ReadFileGrantKind {
    SpeakersAnalyzeGeneration,
}

/// A noninheritable, read-only disk-file capability retained for a child launch.
#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct ReadFileGrant {
    kind: ReadFileGrantKind,
    file: std::sync::Arc<std::fs::File>,
}

#[cfg(windows)]
impl ReadFileGrant {
    pub fn try_clone_read_only(
        kind: ReadFileGrantKind,
        source: &std::fs::File,
    ) -> std::io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::Foundation::DuplicateHandle;
        use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut duplicate = std::ptr::null_mut();
        // SAFETY: the source remains borrowed through duplication, the output is writable,
        // and ownership of the successful noninheritable duplicate immediately enters File.
        #[allow(unsafe_code)]
        unsafe {
            let process = GetCurrentProcess();
            if DuplicateHandle(
                process,
                source.as_raw_handle(),
                process,
                &mut duplicate,
                FILE_GENERIC_READ,
                0,
                0,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[allow(unsafe_code)]
        let file = unsafe { std::fs::File::from_raw_handle(duplicate) };
        Self::from_received_file(kind, file)
    }

    pub fn kind(&self) -> ReadFileGrantKind {
        self.kind
    }

    /// The file remains owned by the grant; the kernel access mask restricts writes.
    pub fn file(&self) -> &std::fs::File {
        &self.file
    }

    pub(super) fn from_received_file(
        kind: ReadFileGrantKind,
        file: std::fs::File,
    ) -> std::io::Result<Self> {
        use std::os::windows::fs::MetadataExt;
        use std::os::windows::io::{AsHandle, AsRawHandle};
        require_handle_access(
            file.as_handle(),
            windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ,
            "File",
        )?;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        // SAFETY: file remains borrowed through this synchronous type query.
        #[allow(unsafe_code)]
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFileType(file.as_handle().as_raw_handle())
        } != windows_sys::Win32::Storage::FileSystem::FILE_TYPE_DISK
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "launch grant is not a disk file",
            ));
        }
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "launch grant is not an ordinary disk file",
            ));
        }
        Ok(Self {
            kind,
            file: std::sync::Arc::new(file),
        })
    }
}

#[cfg(windows)]
pub(super) fn require_handle_access(
    handle: std::os::windows::io::BorrowedHandle<'_>,
    rights: u32,
    object_type: &str,
) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        GetHandleInformation, HANDLE_FLAG_INHERIT, HANDLE_FLAG_PROTECT_FROM_CLOSE,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::System::WindowsProgramming::{
        PUBLIC_OBJECT_BASIC_INFORMATION, PUBLIC_OBJECT_TYPE_INFORMATION,
    };
    type QueryObject = unsafe extern "system" fn(
        *mut std::ffi::c_void,
        i32,
        *mut std::ffi::c_void,
        u32,
        *mut u32,
    ) -> i32;
    let refused = || {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "launch handle access or type does not match its grant",
        )
    };
    let library: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
    // SAFETY: Ntdll is loaded by Windows for this process and remains loaded. Resolve only
    // the documented NtQueryObject entry point; absence refuses rather than weakening checks.
    #[allow(unsafe_code)]
    let query: QueryObject = unsafe {
        let module = GetModuleHandleW(library.as_ptr());
        if module.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let function =
            GetProcAddress(module, c"NtQueryObject".as_ptr().cast()).ok_or_else(refused)?;
        std::mem::transmute(function)
    };
    let mut flags = 0;
    let mut basic = PUBLIC_OBJECT_BASIC_INFORMATION::default();
    let mut returned = 0;
    // SAFETY: handle is borrowed throughout; outputs have the documented size and alignment.
    #[allow(unsafe_code)]
    unsafe {
        if GetHandleInformation(handle.as_raw_handle(), &mut flags) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if flags & (HANDLE_FLAG_INHERIT | HANDLE_FLAG_PROTECT_FROM_CLOSE) != 0 {
            return Err(refused());
        }
        if query(
            handle.as_raw_handle(),
            0,
            (&mut basic as *mut PUBLIC_OBJECT_BASIC_INFORMATION).cast(),
            std::mem::size_of_val(&basic) as u32,
            &mut returned,
        ) < 0
        {
            return Err(refused());
        }
    }
    if basic.GrantedAccess != rights {
        return Err(refused());
    }
    // Pointer-aligned storage, bounded well above the two allowed type names.
    let mut storage = [0usize; 512];
    // SAFETY: the output is aligned for PUBLIC_OBJECT_TYPE_INFORMATION. Before dereferencing
    // its string pointer, check that the entire UTF-16 string lies inside this owned buffer.
    #[allow(unsafe_code)]
    unsafe {
        if query(
            handle.as_raw_handle(),
            2,
            storage.as_mut_ptr().cast(),
            std::mem::size_of_val(&storage) as u32,
            &mut returned,
        ) < 0
        {
            return Err(refused());
        }
        let info = &*storage.as_ptr().cast::<PUBLIC_OBJECT_TYPE_INFORMATION>();
        let start = storage.as_ptr() as usize;
        let end = start + std::mem::size_of_val(&storage);
        let pointer = info.TypeName.Buffer as usize;
        let length = usize::from(info.TypeName.Length);
        if !length.is_multiple_of(2)
            || !pointer.is_multiple_of(2)
            || pointer < start
            || pointer.checked_add(length).is_none_or(|last| last > end)
        {
            return Err(refused());
        }
        let name = std::slice::from_raw_parts(info.TypeName.Buffer, length / 2);
        if String::from_utf16(name).map_err(|_| refused())? != object_type {
            return Err(refused());
        }
    }
    Ok(())
}
