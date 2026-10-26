// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{Mode, SFlag, fstatat, mkdirat};
use nix::sys::statvfs::fstatvfs;
use nix::unistd::{UnlinkatFlags, linkat, unlinkat};

use super::removal::ClaimFailure;
use super::{JournalRoot, MediaEntry, MediaIdentity, MediaPrimitive, changed, invalid, step};
use crate::journal_root::JournalEntryKind;

const DIRECTORY: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);
const PLAIN: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC);

struct Node {
    name: OsString,
    file: File,
    identity: MediaIdentity,
}

pub(super) struct Directory {
    root: Arc<JournalRoot>,
    nodes: Vec<Node>,
    root_file: File,
    root_identity: MediaIdentity,
    path: PathBuf,
}

impl AsFd for Directory {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.nodes
            .last()
            .map_or_else(|| self.root_file.as_fd(), |node| node.file.as_fd())
    }
}

impl Directory {
    pub(super) fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }

    pub(super) fn open(root: Arc<JournalRoot>, names: &[OsString]) -> io::Result<Self> {
        root.revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let root_file = File::from(root.as_fd().try_clone_to_owned()?);
        let root_identity = identity(&root_file)?;
        let mut directory = Self {
            path: root.canonical_path().to_path_buf(),
            root,
            root_file,
            root_identity,
            nodes: Vec::new(),
        };
        for name in names {
            let file = File::from(openat(
                &directory,
                name.as_os_str(),
                DIRECTORY,
                Mode::empty(),
            )?);
            let id = identity(&file)?;
            check_leaf(&directory, name, id)?;
            directory.nodes.push(Node {
                name: name.clone(),
                file,
                identity: id,
            });
            directory.path.push(name);
        }
        directory.revalidate()?;
        Ok(directory)
    }

    pub(super) fn open_child(&self, name: &OsStr) -> io::Result<Option<Self>> {
        self.revalidate()?;
        let mut nodes = Vec::with_capacity(self.nodes.len() + 1);
        for node in &self.nodes {
            nodes.push(Node {
                name: node.name.clone(),
                file: node.file.try_clone()?,
                identity: node.identity,
            });
        }
        let file = match openat(self, name, DIRECTORY, Mode::empty()) {
            Ok(file) => File::from(file),
            Err(nix::errno::Errno::ENOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        step(MediaPrimitive::AfterChildOpen)?;
        let id = identity(&file)?;
        check_leaf(self, name, id)?;
        step(MediaPrimitive::AfterChildObservation)?;
        nodes.push(Node {
            name: name.to_os_string(),
            file,
            identity: id,
        });
        let directory = Self {
            root: self.root.clone(),
            root_file: self.root_file.try_clone()?,
            root_identity: self.root_identity,
            nodes,
            path: self.path.join(name),
        };
        directory.revalidate()?;
        Ok(Some(directory))
    }

    pub(super) fn identity(&self) -> MediaIdentity {
        self.nodes
            .last()
            .map_or(self.root_identity, |node| node.identity)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn stamp(&self) -> io::Result<Stamp> {
        stamp(self.nodes.last().map_or(&self.root_file, |node| &node.file))
    }

    pub(super) fn revalidate(&self) -> io::Result<()> {
        self.root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let mut parent = &self.root_file;
        for node in &self.nodes {
            let stat = fstatat(parent, node.name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)?;
            let id = MediaIdentity::Unix {
                device: stat.st_dev as u64,
                inode: stat.st_ino as u64,
            };
            if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR
                || id != node.identity
                || identity(&node.file)? != node.identity
            {
                return Err(changed());
            }
            parent = &node.file;
        }
        Ok(())
    }

    pub(super) fn available_bytes(&self) -> io::Result<u64> {
        let stat = fstatvfs(self)?;
        #[allow(
            clippy::unnecessary_cast,
            reason = "fsblkcnt_t is 32-bit on macOS and 64-bit on Linux"
        )]
        let blocks = stat.blocks_available() as u64;
        blocks
            .checked_mul(stat.fragment_size())
            .ok_or_else(|| invalid("media capacity overflow"))
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        self.nodes
            .last()
            .map_or(&self.root_file, |node| &node.file)
            .sync_all()
    }
}

pub(super) fn supported_mutation() -> io::Result<()> {
    Ok(())
}

pub(super) fn entries(
    directory: &Directory,
    maximum: usize,
) -> io::Result<Option<Vec<MediaEntry>>> {
    list(directory, maximum, true)
}

pub(super) fn entries_keeping_any_name(
    directory: &Directory,
    maximum: usize,
) -> io::Result<Option<Vec<MediaEntry>>> {
    list(directory, maximum, false)
}

fn list(
    directory: &Directory,
    maximum: usize,
    portable_only: bool,
) -> io::Result<Option<Vec<MediaEntry>>> {
    let mut cursor = nix::dir::Dir::openat(directory, ".", DIRECTORY, Mode::empty())?;
    let mut result = Vec::new();
    for record in cursor.iter() {
        let record = record?;
        let bytes = record.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        if result.len() == maximum {
            return Ok(None);
        }
        let name = OsString::from_vec(bytes.to_vec());
        if portable_only {
            super::plain_name(&name)?;
        } else {
            super::utf8_name(&name)?;
        }
        step(MediaPrimitive::BeforeEntryObservation)?;
        let observed = entry(directory, &name)?.ok_or_else(changed)?;
        let MediaIdentity::Unix { inode, device } = observed.identity else {
            return Err(changed());
        };
        let MediaIdentity::Unix {
            device: parent_device,
            ..
        } = directory.identity()
        else {
            return Err(changed());
        };
        verify_enumerated_inode(record.ino(), inode, device, parent_device)?;
        result.push(observed);
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Some(result))
}

fn verify_enumerated_inode(
    enumerated: u64,
    observed: u64,
    device: u64,
    parent_device: u64,
) -> io::Result<()> {
    if enumerated == 0 || enumerated != observed || device != parent_device {
        return Err(changed());
    }
    Ok(())
}

pub(super) fn entry(directory: &Directory, name: &OsStr) -> io::Result<Option<MediaEntry>> {
    let stat = match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(nix::errno::Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let size = u64::try_from(stat.st_size).map_err(|_| invalid("negative entry size"))?;
    Ok(Some(MediaEntry {
        name: name.to_owned(),
        kind: JournalEntryKind::from_mode(SFlag::from_bits_truncate(stat.st_mode)),
        identity: MediaIdentity::Unix {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
        },
        size,
        stamp: (
            size,
            stat.st_mtime,
            stat.st_mtime_nsec,
            stat.st_ctime,
            stat.st_ctime_nsec,
        ),
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

#[cfg(not(target_os = "macos"))]
pub(super) fn archive_path(directory: &Directory, name: &OsStr) -> io::Result<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::fd::AsRawFd;
        Ok(PathBuf::from(format!("/proc/self/fd/{}", directory.as_fd().as_raw_fd())).join(name))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        Ok(directory.path().join(name))
    }
}

pub(super) fn identity(file: &File) -> io::Result<MediaIdentity> {
    let metadata = file.metadata()?;
    Ok(MediaIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

pub(super) type Stamp = (u64, i64, i64, i64, i64);
pub(super) fn stamp(file: &File) -> io::Result<Stamp> {
    let m = file.metadata()?;
    Ok((
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}

pub(super) fn check_leaf(
    directory: &Directory,
    name: &OsStr,
    expected: MediaIdentity,
) -> io::Result<()> {
    let stat = fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    if (MediaIdentity::Unix {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    }) != expected
    {
        return Err(changed());
    }
    Ok(())
}

pub(super) fn open_plain(directory: &Directory, name: &OsStr) -> io::Result<Option<File>> {
    // Classify before open, so a FIFO/device cannot become a media reader.
    let stat = match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(nix::errno::Errno::ENOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
        return Err(invalid("media entry is not a plain file"));
    }
    let file = File::from(openat(directory, name, PLAIN, Mode::empty())?);
    if !file.metadata()?.is_file() {
        return Err(invalid("media entry is not a plain file"));
    }
    let expected = MediaIdentity::Unix {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    };
    if identity(&file)? != expected {
        return Err(changed());
    }
    Ok(Some(file))
}

pub(super) fn open_source(directory: &Directory, name: &OsStr) -> io::Result<Option<File>> {
    open_plain(directory, name)
}

pub(super) fn open_removal(directory: &Directory, name: &OsStr) -> io::Result<Option<File>> {
    open_plain(directory, name)
}

pub(super) fn claim(
    source: &Directory,
    source_name: &OsStr,
    _file: &File,
    destination: &Directory,
    name: &OsStr,
) -> Result<(), ClaimFailure> {
    let from = CString::new(source_name.as_bytes()).map_err(|_| invalid("invalid claim name"))?;
    let to = CString::new(name.as_bytes()).map_err(|_| invalid("invalid claim name"))?;
    step(MediaPrimitive::NativeClaimCut)?;
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    {
        use std::os::fd::AsRawFd;

        // SAFETY: both retained directory descriptors and NUL-terminated names
        // remain live for the synchronous create-only rename. No fallback.
        // Android uses the same renameat2 flag as Linux. The teardown lease's
        // rename stays in claim_remove and is not this claim.
        #[allow(unsafe_code)]
        let result = unsafe {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                libc::syscall(
                    libc::SYS_renameat2,
                    source.as_fd().as_raw_fd(),
                    from.as_ptr(),
                    destination.as_fd().as_raw_fd(),
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                ) as i32
            }
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
                libc::renameatx_np(
                    source.as_fd().as_raw_fd(),
                    from.as_ptr(),
                    destination.as_fd().as_raw_fd(),
                    to.as_ptr(),
                    libc::RENAME_EXCL,
                )
            }
        };
        if result != 0 {
            return Err(ClaimFailure::native(io::Error::last_os_error()));
        }
        step(MediaPrimitive::ClaimAppliedError)?;
        Ok(())
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        let _ = (source, destination, from, to);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native no-replace claim unavailable",
        )
        .into())
    }
}

/// Move `from_name` to `to_name` in `parent` only when `to_name` is absent.
///
/// An existing destination, including an empty directory, is `AlreadyExists`.
/// A target without `RENAME_NOREPLACE` or `RENAME_EXCL` returns `Unsupported`.
pub fn publish_directory_noreplace(
    parent: &Path,
    from_name: &OsStr,
    to_name: &OsStr,
) -> io::Result<()> {
    publish_name(from_name)?;
    publish_name(to_name)?;
    let directory = nix::fcntl::open(parent, DIRECTORY, Mode::empty())?;
    rename_directory_noreplace(&directory, from_name, &directory, to_name)
}

/// Move one direct name between retained parents using native create-only rename.
pub fn publish_directory_noreplace_bound(
    from_parent: &impl AsFd,
    from_name: &OsStr,
    to_parent: &impl AsFd,
    to_name: &OsStr,
) -> io::Result<()> {
    for name in [from_name, to_name] {
        publish_name(name)?;
        if Path::new(name).components().count() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "publish name must be one component",
            ));
        }
    }
    rename_directory_noreplace(from_parent, from_name, to_parent, to_name)
}

fn rename_directory_noreplace(
    from_parent: &impl AsFd,
    from_name: &OsStr,
    to_parent: &impl AsFd,
    to_name: &OsStr,
) -> io::Result<()> {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))]
    {
        use std::os::fd::AsRawFd;
        let from = CString::new(from_name.as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "publish name contains NUL")
        })?;
        let to = CString::new(to_name.as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "publish name contains NUL")
        })?;
        // SAFETY: retained directory descriptors and NUL-terminated names live
        // through this synchronous native create-only rename. No fallback.
        #[allow(unsafe_code)]
        let result = unsafe {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                libc::syscall(
                    libc::SYS_renameat2,
                    from_parent.as_fd().as_raw_fd(),
                    from.as_ptr(),
                    to_parent.as_fd().as_raw_fd(),
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                ) as i32
            }
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
                libc::renameatx_np(
                    from_parent.as_fd().as_raw_fd(),
                    from.as_ptr(),
                    to_parent.as_fd().as_raw_fd(),
                    to.as_ptr(),
                    libc::RENAME_EXCL,
                )
            }
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    )))]
    {
        let _ = (from_parent, from_name, to_parent, to_name);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "native no-replace publication unavailable",
        ))
    }
}

fn publish_name(name: &OsStr) -> io::Result<()> {
    let path = Path::new(name);
    if name.is_empty() || path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "publish name must be a relative path",
        ));
    }
    let mut saw_normal = false;
    for component in path.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "publish name must contain only normal components",
            ));
        }
        saw_normal = true;
    }
    if !saw_normal {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "publish name must be a relative path",
        ));
    }
    Ok(())
}

pub(super) fn delete_claim(directory: &Directory, name: &OsStr, file: &File) -> io::Result<()> {
    remove_leaf(directory, name, file)?;
    step(MediaPrimitive::DeleteAppliedError)
}

pub(super) fn ensure_absent(directory: &Directory, name: &OsStr) -> io::Result<()> {
    if entry(directory, name)?.is_some() {
        return Err(changed());
    }
    Ok(())
}

pub(super) fn create_directory(parent: &Directory, name: &OsStr) -> io::Result<Directory> {
    mkdirat(parent, name, Mode::S_IRWXU)?;
    parent.open_child(name)?.ok_or_else(changed)
}

pub(super) fn create_file(parent: &Directory, name: &OsStr) -> io::Result<File> {
    Ok(File::from(openat(
        parent,
        name,
        OFlag::O_RDWR
            .union(OFlag::O_CREAT)
            .union(OFlag::O_EXCL)
            .union(OFlag::O_NOFOLLOW)
            .union(OFlag::O_CLOEXEC),
        Mode::S_IRUSR.union(Mode::S_IWUSR),
    )?))
}

pub(super) fn publish(
    source: &Directory,
    source_name: &OsStr,
    _file: &File,
    destination: &Directory,
    name: &OsStr,
) -> io::Result<()> {
    linkat(source, source_name, destination, name, AtFlags::empty()).map_err(Into::into)
}

pub(super) fn remove_leaf(parent: &Directory, name: &OsStr, _file: &File) -> io::Result<()> {
    unlinkat(parent, name, UnlinkatFlags::NoRemoveDir).map_err(Into::into)
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
))]
pub(super) fn open_removal_directory(
    parent: &Directory,
    name: &OsStr,
) -> io::Result<Option<Directory>> {
    parent.open_child(name)
}

/// Remove one empty child directory that still has `identity`. False when it
/// is absent or another object; a directory holding anything refuses.
pub(super) fn remove_empty_child(
    parent: &Directory,
    name: &OsStr,
    identity: MediaIdentity,
) -> io::Result<bool> {
    match parent.open_child(name)? {
        Some(child) if child.identity() == identity => {
            remove_directory(parent, name, &child).map(|()| true)
        }
        _ => Ok(false),
    }
}

pub(super) fn remove_directory(
    parent: &Directory,
    name: &OsStr,
    directory: &Directory,
) -> io::Result<()> {
    check_leaf(parent, name, directory.identity())?;
    step(MediaPrimitive::NativeDirectoryRemoveCut)?;
    unlinkat(parent, name, UnlinkatFlags::RemoveDir).map_err(Into::into)
}

#[cfg(test)]
mod enumeration_tests {
    #[test]
    fn unavailable_or_truncated_enumeration_identity_never_matches() {
        assert!(super::verify_enumerated_inode(0, 0, 1, 1).is_err());
        assert!(super::verify_enumerated_inode(7, (1u64 << 32) | 7, 1, 1).is_err());
        assert!(super::verify_enumerated_inode(7, 8, 1, 1).is_err());
        assert!(super::verify_enumerated_inode(7, 7, 2, 1).is_err());
        assert!(super::verify_enumerated_inode((1u64 << 32) | 7, (1u64 << 32) | 7, 1, 1).is_ok());
    }
}
