// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_DISPOSITION_INFORMATION, FILE_NON_DIRECTORY_FILE,
    FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
    FILE_SYNCHRONOUS_IO_NONALERT, FileDispositionInformation, FileFsFullSizeInformation,
    FileRenameInformation, NtQueryVolumeInformationFile, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, RtlNtStatusToDosError, STATUS_SUCCESS};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_BASIC_INFO, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
    FILE_WRITE_DATA, FileAttributeTagInfo, FileBasicInfo, GetFileInformationByHandleEx, ReOpenFile,
    SYNCHRONIZE,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

use super::removal::ClaimFailure;
use super::{JournalRoot, MediaEntry, MediaIdentity, MediaPrimitive, changed, invalid, step};
use crate::journal_root::JournalEntryKind;
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::{
    nt_create_relative, nt_create_relative_deny_delete_sharing, nt_create_relative_exact,
    nt_create_relative_share_read,
};
use crate::windows_publication_path::{
    AncestorAdmission, WindowsPublicationPath, admit_publication_path,
};

const OPTIONS: u32 =
    FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_FOR_BACKUP_INTENT;
const DIRECTORY_ACCESS: u32 =
    FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE;

struct Node {
    name: OsString,
    file: File,
    // None retains the actual created handle if the initial identity query fails.
    identity: Option<MediaIdentity>,
}

pub(super) struct Directory {
    root: Arc<JournalRoot>,
    anchor: Arc<WindowsPublicationPath>,
    nodes: Vec<Arc<Node>>,
    path: PathBuf,
}

impl Directory {
    pub(super) fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }

    pub(super) fn open(root: Arc<JournalRoot>, names: &[OsString]) -> io::Result<Self> {
        root.revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let mut path = root.canonical_path().to_path_buf();
        path.extend(names);
        // The dummy leaf is never opened. ExistingOnly retains every directory
        // from the volume root through the requested terminal directory.
        let locator = path.join("media-admission");
        let anchor = admit_publication_path(
            locator
                .to_str()
                .ok_or_else(|| invalid("invalid Windows media path"))?,
            &[],
        )
        .and_then(|admitted| admitted.retain_ancestors(AncestorAdmission::ExistingOnly))
        .map_err(io::Error::other)?;
        let directory = Self {
            root,
            anchor: Arc::new(anchor),
            nodes: Vec::new(),
            path,
        };
        directory.revalidate()?;
        Ok(directory)
    }

    fn handle(&self) -> RawHandle {
        self.nodes.last().map_or_else(
            || self.anchor.terminal_parent().as_raw_handle(),
            |node| node.file.as_raw_handle(),
        )
    }

    /// Only owned stage directories have a node with DELETE access.
    pub(super) fn file(&self) -> &File {
        &self.nodes.last().expect("owned stage directory").file
    }

    pub(super) fn identity(&self) -> MediaIdentity {
        self.nodes.last().map_or_else(
            || witness(self.anchor.terminal_parent_identity()),
            |node| node.identity.expect("admitted directory identity"),
        )
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn open_child(&self, name: &OsStr) -> io::Result<Option<Self>> {
        self.open_child_with_access(name, DIRECTORY_ACCESS)
    }

    fn open_child_with_access(&self, name: &OsStr, access: u32) -> io::Result<Option<Self>> {
        self.revalidate()?;
        let file = match nt_create_relative_deny_delete_sharing(
            self.handle(),
            name,
            access,
            FILE_OPEN,
            OPTIONS | FILE_DIRECTORY_FILE,
        ) {
            Ok(file) => File::from(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        step(MediaPrimitive::AfterChildOpen)?;
        require_kind(file.as_raw_handle(), true)?;
        let identity = identity(&file)?;
        step(MediaPrimitive::AfterChildObservation)?;
        let mut nodes = self.nodes.clone();
        nodes.push(Arc::new(Node {
            name: name.to_owned(),
            file,
            identity: Some(identity),
        }));
        let child = Self {
            root: self.root.clone(),
            anchor: self.anchor.clone(),
            nodes,
            path: self.path.join(name),
        };
        child.revalidate()?;
        Ok(Some(child))
    }

    pub(super) fn stamp(&self) -> io::Result<Stamp> {
        let file = match self.nodes.last() {
            Some(node) => node.file.try_clone()?,
            None => File::from(self.anchor.terminal_parent().try_clone()?),
        };
        stamp(&file)
    }

    pub(super) fn revalidate(&self) -> io::Result<()> {
        self.root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        self.anchor.revalidate().map_err(io::Error::other)?;
        let mut parent = self.anchor.terminal_parent().as_raw_handle();
        for node in &self.nodes {
            let expected = node.identity.ok_or_else(changed)?;
            require_kind(node.file.as_raw_handle(), true)?;
            if identity(&node.file)? != expected {
                return Err(changed());
            }
            let current = nt_create_relative(
                parent,
                &node.name,
                FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                FILE_OPEN,
                OPTIONS | FILE_DIRECTORY_FILE,
            )?;
            require_kind(current.as_raw_handle(), true)?;
            if witness(file_identity(current.as_raw_handle())?) != expected {
                return Err(changed());
            }
            parent = node.file.as_raw_handle();
        }
        Ok(())
    }

    pub(super) fn available_bytes(&self) -> io::Result<u64> {
        let mut info = FullSizeInformation::default();
        let mut status = IO_STATUS_BLOCK::default();
        // SAFETY: the retained synchronous directory handle and aligned fixed
        // buffer remain live through this call. No asynchronous completion is accepted.
        #[allow(unsafe_code)]
        let result = unsafe {
            NtQueryVolumeInformationFile(
                self.handle(),
                &mut status,
                (&mut info as *mut FullSizeInformation).cast(),
                size_of::<FullSizeInformation>() as u32,
                FileFsFullSizeInformation,
            )
        };
        native_status(result)?;
        if status.Information < size_of::<FullSizeInformation>() {
            return Err(invalid("incomplete retained volume capacity"));
        }
        info.available_bytes()
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows directory durability is unproven",
        ))
    }
}

#[repr(C, align(8))]
#[derive(Default)]
struct FullSizeInformation {
    total: i64,
    caller_available: i64,
    actual_available: i64,
    sectors_per_unit: u32,
    bytes_per_sector: u32,
}

impl FullSizeInformation {
    fn available_bytes(&self) -> io::Result<u64> {
        if self.sectors_per_unit == 0 || self.bytes_per_sector == 0 {
            return Err(invalid("invalid retained volume geometry"));
        }
        u64::try_from(self.caller_available)
            .ok()
            .and_then(|units| units.checked_mul(u64::from(self.sectors_per_unit)))
            .and_then(|sectors| sectors.checked_mul(u64::from(self.bytes_per_sector)))
            .ok_or_else(|| invalid("retained volume capacity overflow"))
    }
}

pub(super) fn supported_mutation() -> io::Result<()> {
    Ok(())
}

pub(super) fn entries(
    directory: &Directory,
    maximum: usize,
) -> io::Result<Option<Vec<MediaEntry>>> {
    // An empty native name reopens the actual retained directory object with a
    // new file object and enumeration cursor; no namespace path is resolved.
    // Native NtCreateFile rejects dot; duplicated handles share cursor state.
    let cursor = nt_create_relative_exact(
        directory.handle(),
        OsStr::new(""),
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        OPTIONS | FILE_DIRECTORY_FILE,
    )
    .map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("open independent directory cursor: {error}"),
        )
    })?;
    require_kind(cursor.as_raw_handle(), true)?;
    if witness(file_identity(cursor.as_raw_handle())?) != directory.identity() {
        return Err(changed());
    }
    let Some(listed) = crate::windows_sync_dir::list_directory_bounded(
        cursor.as_raw_handle(),
        directory.path(),
        maximum,
    )
    .map_err(io::Error::other)?
    else {
        return Ok(None);
    };
    let mut result = Vec::with_capacity(listed.len());
    for record in listed {
        step(MediaPrimitive::BeforeEntryObservation)?;
        let observed = entry(directory, &record.name)?.ok_or_else(changed)?;
        let MediaIdentity::Windows { file_id, .. } = observed.identity else {
            return Err(changed());
        };
        if file_id != record.file_id {
            return Err(changed());
        }
        result.push(observed);
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Some(result))
}

/// Windows cannot hold a colon in a name, the case this exists for, so every
/// listed name is still admitted here.
pub(super) fn entries_keeping_any_name(
    directory: &Directory,
    maximum: usize,
) -> io::Result<Option<Vec<MediaEntry>>> {
    entries(directory, maximum)
}

pub(super) fn entry(directory: &Directory, name: &OsStr) -> io::Result<Option<MediaEntry>> {
    super::plain_name(name)?;
    let handle = match nt_create_relative_exact(
        directory.handle(),
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        OPTIONS,
    ) {
        Ok(handle) => handle,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let file = File::from(handle);
    let metadata = file.metadata()?;
    use std::os::windows::fs::MetadataExt;
    let kind = if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        JournalEntryKind::Symlink
    } else if metadata.is_dir() {
        JournalEntryKind::Directory
    } else {
        JournalEntryKind::RegularFile
    };
    let stamp = stamp(&file)?;
    Ok(Some(MediaEntry {
        name: name.to_owned(),
        kind,
        identity: identity(&file)?,
        size: stamp.0,
        stamp,
    }))
}

pub(super) fn entry_from_file(name: &OsStr, file: &File) -> io::Result<MediaEntry> {
    let stamp = stamp(file)?;
    Ok(MediaEntry {
        name: name.to_owned(),
        kind: JournalEntryKind::RegularFile,
        identity: identity(file)?,
        size: stamp.0,
        stamp,
    })
}

pub(super) fn archive_path(directory: &Directory, name: &OsStr) -> io::Result<PathBuf> {
    let mut path = PathBuf::from(directory.anchor.move_spelling());
    for node in &directory.nodes {
        path.push(&node.name);
    }
    path.push(name);
    // The extended-length Win32 spelling includes its terminal NUL.
    if path.as_os_str().encode_wide().count() >= 32767 {
        return Err(invalid("Windows archive input path exceeds native budget"));
    }
    Ok(path)
}

fn reopen(file: &File, access: u32, sharing: u32) -> io::Result<File> {
    let expected = identity(file)?;
    // SAFETY: synchronous reopen of a retained plain object; the actual prior
    // handle stays live until the new handle's full identity has been checked.
    #[allow(unsafe_code)]
    let handle = unsafe {
        ReOpenFile(
            file.as_raw_handle(),
            access,
            sharing,
            FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful ReOpenFile returned this uniquely owned native handle.
    #[allow(unsafe_code)]
    let reopened = unsafe { File::from_raw_handle(handle) };
    require_kind(reopened.as_raw_handle(), false)?;
    if identity(&reopened)? != expected {
        return Err(changed());
    }
    Ok(reopened)
}

pub(super) fn transition_access(file: &mut File, mutation: bool) -> io::Result<()> {
    let read = FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
    let bridge = reopen(
        file,
        read,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )?;
    // Retain the actual object through the sharing gap. The caller revalidates
    // leaf/chain and prepared stamp after final restrictive admission.
    drop(std::mem::replace(file, bridge));
    step(MediaPrimitive::ArchiveAccessGap)?;
    let access = if mutation {
        read | FILE_WRITE_DATA | DELETE
    } else {
        read
    };
    let restrictive = reopen(file, access, FILE_SHARE_READ)?;
    *file = restrictive;
    Ok(())
}

fn witness(identity: WindowsFileIdentity) -> MediaIdentity {
    MediaIdentity::Windows {
        volume: identity.volume_serial(),
        file_id: identity.file_id(),
    }
}

pub(super) fn identity(file: &File) -> io::Result<MediaIdentity> {
    file_identity(file.as_raw_handle()).map(witness)
}

pub(super) type Stamp = (u64, i64, i64);
pub(super) fn stamp(file: &File) -> io::Result<Stamp> {
    let mut info = FILE_BASIC_INFO::default();
    // SAFETY: exact initialized output buffer and retained handle for synchronous query.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            (&mut info as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((file.metadata()?.len(), info.LastWriteTime, info.ChangeTime))
}

fn require_kind(handle: RawHandle, directory: bool) -> io::Result<()> {
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: exact initialized output buffer and retained handle for synchronous query.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(invalid("media entry is not the admitted plain kind"));
    }
    Ok(())
}

pub(super) fn check_leaf(
    directory: &Directory,
    name: &OsStr,
    expected: MediaIdentity,
) -> io::Result<()> {
    check_entry(directory, name, expected, false)
}

fn check_entry(
    directory: &Directory,
    name: &OsStr,
    expected: MediaIdentity,
    is_directory: bool,
) -> io::Result<()> {
    let file = nt_create_relative(
        directory.handle(),
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        OPTIONS,
    )?;
    require_kind(file.as_raw_handle(), is_directory)?;
    if witness(file_identity(file.as_raw_handle())?) != expected {
        return Err(changed());
    }
    Ok(())
}

fn open_file(
    directory: &Directory,
    name: &OsStr,
    exclude_writes: bool,
    delete: bool,
) -> io::Result<Option<File>> {
    // Classify before opening for media reads, and refuse all reparse/non-files.
    let admission = match nt_create_relative(
        directory.handle(),
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        OPTIONS,
    ) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    require_kind(admission.as_raw_handle(), false)?;
    let expected = witness(file_identity(admission.as_raw_handle())?);
    let open = if exclude_writes {
        nt_create_relative_share_read
    } else {
        nt_create_relative
    };
    let file = File::from(open(
        directory.handle(),
        name,
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE | if delete { DELETE } else { 0 },
        FILE_OPEN,
        OPTIONS | FILE_NON_DIRECTORY_FILE,
    )?);
    require_kind(file.as_raw_handle(), false)?;
    if identity(&file)? != expected {
        return Err(changed());
    }
    Ok(Some(file))
}

pub(super) fn open_plain(directory: &Directory, name: &OsStr) -> io::Result<Option<File>> {
    open_file(directory, name, false, false)
}
pub(super) fn open_source(directory: &Directory, name: &OsStr) -> io::Result<Option<File>> {
    open_file(directory, name, true, false)
}

pub(super) fn open_removal(directory: &Directory, name: &OsStr) -> io::Result<Option<File>> {
    open_file(directory, name, true, true)
}

pub(super) fn open_removal_directory(
    parent: &Directory,
    name: &OsStr,
) -> io::Result<Option<Directory>> {
    parent.open_child_with_access(name, DIRECTORY_ACCESS | DELETE)
}

pub(super) fn claim(
    source: &Directory,
    source_name: &OsStr,
    file: &File,
    destination: &Directory,
    name: &OsStr,
) -> Result<(), ClaimFailure> {
    step(MediaPrimitive::NativeClaimCut)?;
    publish(source, source_name, file, destination, name).map_err(ClaimFailure::native)?;
    step(MediaPrimitive::ClaimAppliedError)?;
    Ok(())
}

/// Move `from_name` to `to_name` in `parent` only when `to_name` is absent.
///
/// Windows has no no-replace directory publication here and does not rename.
pub fn publish_directory_noreplace(
    parent: &Path,
    from_name: &OsStr,
    to_name: &OsStr,
) -> io::Result<()> {
    let _ = (parent, from_name, to_name);
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "native no-replace publication unavailable",
    ))
}

pub(super) fn delete_claim(_directory: &Directory, _name: &OsStr, file: &File) -> io::Result<()> {
    mark_delete(file)?;
    step(MediaPrimitive::DeleteAppliedError)
}

pub(super) fn create_directory(parent: &Directory, name: &OsStr) -> io::Result<Directory> {
    let file = File::from(nt_create_relative_deny_delete_sharing(
        parent.handle(),
        name,
        DIRECTORY_ACCESS | DELETE,
        FILE_CREATE,
        OPTIONS | FILE_DIRECTORY_FILE,
    )?);
    // Retain the same create handle even on initial identity-query refusal.
    let id = identity(&file).ok();
    let mut nodes = parent.nodes.clone();
    nodes.push(Arc::new(Node {
        name: name.to_os_string(),
        file,
        identity: id,
    }));
    Ok(Directory {
        root: parent.root.clone(),
        anchor: parent.anchor.clone(),
        nodes,
        path: parent.path.join(name),
    })
}

pub(super) fn create_file(parent: &Directory, name: &OsStr) -> io::Result<File> {
    Ok(File::from(nt_create_relative_share_read(
        parent.handle(),
        name,
        FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
        FILE_CREATE,
        OPTIONS | FILE_NON_DIRECTORY_FILE,
    )?))
}

pub(super) fn publish(
    _source: &Directory,
    _source_name: &OsStr,
    file: &File,
    destination: &Directory,
    name: &OsStr,
) -> io::Result<()> {
    let wide: Vec<u16> = name.encode_wide().collect();
    let bytes = wide
        .len()
        .checked_mul(size_of::<u16>())
        .ok_or_else(|| invalid("media rename name overflow"))?;
    let length = offset_of!(FILE_RENAME_INFORMATION, FileName)
        .checked_add(bytes)
        .ok_or_else(|| invalid("media rename buffer overflow"))?;
    let native_length =
        u32::try_from(length).map_err(|_| invalid("media rename buffer overflow"))?;
    let name_length = u32::try_from(bytes).map_err(|_| invalid("media rename name overflow"))?;
    // u64 storage provides header alignment on supported Windows architectures;
    // zero padding includes ReplaceIfExists=false and no flags.
    let mut buffer = vec![
        0u64;
        length
            .max(size_of::<FILE_RENAME_INFORMATION>())
            .div_ceil(size_of::<u64>())
    ];
    let header = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: buffer has header alignment and enough bytes for the variable UTF-16
    // name; name and both handles remain live through the synchronous native call.
    #[allow(unsafe_code)]
    unsafe {
        (*header).RootDirectory = destination.handle();
        (*header).FileNameLength = name_length;
        std::ptr::copy_nonoverlapping(
            wide.as_ptr(),
            std::ptr::addr_of_mut!((*header).FileName).cast::<u16>(),
            wide.len(),
        );
    }
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: aligned initialized buffer with checked native length; synchronous retained handle.
    #[allow(unsafe_code)]
    let result = unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            header.cast(),
            native_length,
            FileRenameInformation,
        )
    };
    native_status(result)
}

pub(super) fn mark_delete(file: &File) -> io::Result<()> {
    let info = FILE_DISPOSITION_INFORMATION { DeleteFile: true };
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: retained owned DELETE handle and initialized fixed-size input;
    // caller closes its handles then independently confirms name absence.
    #[allow(unsafe_code)]
    let result = unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            (&info as *const FILE_DISPOSITION_INFORMATION).cast(),
            size_of::<FILE_DISPOSITION_INFORMATION>() as u32,
            FileDispositionInformation,
        )
    };
    native_status(result)
}

/// Remove one empty child directory that still has `expected` identity. False
/// when it is absent or another object; delete disposition refuses a directory
/// holding anything.
pub(super) fn remove_empty_child(
    parent: &Directory,
    name: &OsStr,
    expected: MediaIdentity,
) -> io::Result<bool> {
    super::plain_name(name)?;
    parent.revalidate()?;
    let file = match nt_create_relative_deny_delete_sharing(
        parent.handle(),
        name,
        DIRECTORY_ACCESS | DELETE,
        FILE_OPEN,
        OPTIONS | FILE_DIRECTORY_FILE,
    ) {
        Ok(handle) => File::from(handle),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    require_kind(file.as_raw_handle(), true)?;
    if identity(&file)? != expected {
        return Ok(false);
    }
    mark_delete(&file)?;
    drop(file); // Close our handle before confirming absence.
    ensure_absent(parent, name)?;
    Ok(true)
}

// The caller closes the owned handle, then confirms name absence.
pub(super) fn remove_leaf(_parent: &Directory, _name: &OsStr, file: &File) -> io::Result<()> {
    mark_delete(file)
}

pub(super) fn remove_directory(
    parent: &Directory,
    name: &OsStr,
    directory: &Directory,
) -> io::Result<()> {
    check_entry(parent, name, directory.identity(), true)?;
    step(MediaPrimitive::NativeDirectoryRemoveCut)?;
    mark_delete(directory.file())
}

pub(super) fn ensure_absent(parent: &Directory, name: &OsStr) -> io::Result<()> {
    parent.revalidate()?;
    match nt_create_relative(
        parent.handle(),
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        OPTIONS,
    ) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error), // Delete-pending/sharing/query failure stays pending.
        Ok(_) => Err(changed()),  // Never delete a replacement from saved evidence.
    }
}

fn native_status(status: i32) -> io::Result<()> {
    if status == STATUS_SUCCESS {
        return Ok(());
    }
    // SAFETY: pure conversion of the returned native status.
    #[allow(unsafe_code)]
    let error = unsafe { RtlNtStatusToDosError(status) };
    Err(io::Error::from_raw_os_error(error as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_uses_caller_quota_with_checked_geometry() {
        let mut info = FullSizeInformation {
            total: 1000,
            actual_available: 500,
            caller_available: 3,
            sectors_per_unit: 8,
            bytes_per_sector: 512,
        };
        assert_eq!(info.available_bytes().unwrap(), 12288);
        info.caller_available = 1;
        assert_eq!(info.available_bytes().unwrap(), 4096);
        info.caller_available = 0;
        assert_eq!(info.available_bytes().unwrap(), 0);
        info.caller_available = -1;
        assert!(info.available_bytes().is_err());
        info.caller_available = i64::MAX;
        assert!(info.available_bytes().is_err());
        info.caller_available = 1;
        info.sectors_per_unit = 0;
        assert!(info.available_bytes().is_err());
        info.sectors_per_unit = 1;
        info.bytes_per_sector = 0;
        assert!(info.available_bytes().is_err());
    }
}
