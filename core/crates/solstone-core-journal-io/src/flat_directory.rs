// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Descriptor-bound, non-mutating access to one flat journal directory.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};

use crate::errors::{FlatDirectoryError, PathError};
use crate::journal_root::{JournalEntryKind, JournalRoot, JournalRootError, ObjectIdentity};
use crate::name_admission::{NameAdmissionReason, check_portable_component};
use crate::observation::{FileObservation, FlatDirectoryEntry, NativeMtime, same_entry_metadata};
use crate::paths::create_directory_bound;

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlatDirectoryTestPrimitive {
    BeforeDescendantOpen,
    BeforeEntryStat,
    BeforeObservedFileOpen,
    AfterObservedFileStat,
}

#[cfg(test)]
struct FlatDirectoryTestHook {
    primitive: FlatDirectoryTestPrimitive,
    callback: Box<dyn FnOnce()>,
}

#[cfg(test)]
thread_local! {
    static FLAT_DIRECTORY_TEST_HOOK: std::cell::RefCell<Option<FlatDirectoryTestHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
fn run_with_flat_directory_hook<T>(
    primitive: FlatDirectoryTestPrimitive,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    FLAT_DIRECTORY_TEST_HOOK.with(|hook| {
        assert!(
            hook.borrow().is_none(),
            "flat-directory test hook is already active"
        );
        *hook.borrow_mut() = Some(FlatDirectoryTestHook {
            primitive,
            callback: Box::new(callback),
        });
    });
    let result = op();
    let callback = FLAT_DIRECTORY_TEST_HOOK.with(|hook| hook.borrow_mut().take());
    (result, callback.is_none())
}

#[cfg(test)]
fn flat_directory_test_hook(primitive: FlatDirectoryTestPrimitive) {
    let callback = FLAT_DIRECTORY_TEST_HOOK.with(|hook| {
        let mut hook = hook.borrow_mut();
        if hook
            .as_ref()
            .is_some_and(|candidate| candidate.primitive == primitive)
        {
            hook.take().map(|hook| hook.callback)
        } else {
            None
        }
    });
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(not(test))]
fn flat_directory_test_hook(_primitive: FlatDirectoryTestPrimitive) {}

/// An acquired directory descriptor for direct-entry operations only.
pub struct FlatDirectory {
    directory: OwnedFd,
    identity: ObjectIdentity,
    diagnostic_path: PathBuf,
}

impl FlatDirectory {
    /// Descend from `root` through a nonempty portable relative directory path.
    ///
    /// Every component is checked and opened relative to an already verified
    /// descriptor. [`JournalRoot::canonical_path`] contributes only diagnostic
    /// metadata to errors; it is never reopened or used as authority.
    pub fn open(root: &JournalRoot, relative: &Path) -> Result<Self, FlatDirectoryError> {
        let components = portable_relative_components(relative)?;
        root.revalidate().map_err(|error| {
            map_root_revalidation_error(error, root.canonical_path().to_path_buf())
        })?;

        let mut diagnostic_path = root.canonical_path().to_path_buf();
        let mut opened: Option<(OwnedFd, ObjectIdentity)> = None;

        for component in components {
            diagnostic_path.push(component);
            let parent: &dyn AsFd = match &opened {
                Some((directory, _)) => directory,
                None => root,
            };
            let next = open_verified_child_directory(parent, component, &diagnostic_path)?
                .ok_or_else(|| {
                    errno_error(
                        "stat flat-directory descendant",
                        diagnostic_path.clone(),
                        Errno::ENOENT,
                    )
                })?;
            opened = Some(next);
        }

        let (directory, identity) = opened.expect("nonempty components open one descriptor");
        Ok(Self {
            identity,
            directory,
            diagnostic_path,
        })
    }

    /// Return the identity frozen when this flat directory was acquired.
    #[must_use]
    pub fn identity(&self) -> ObjectIdentity {
        self.identity
    }

    /// Confirm the retained descriptor still names the acquired directory identity.
    pub fn revalidate(&self) -> Result<(), FlatDirectoryError> {
        let stat = fstat(self).map_err(|error| {
            errno_error(
                "revalidate flat directory",
                self.diagnostic_path.clone(),
                error,
            )
        })?;
        if kind_from_stat(&stat) != JournalEntryKind::Directory
            || identity_from_stat(&stat, &self.diagnostic_path)? != self.identity
        {
            return Err(FlatDirectoryError::IdentityChanged {
                path: self.diagnostic_path.clone(),
            });
        }
        Ok(())
    }

    /// Return the recorded diagnostic spelling, which is not source authority.
    #[must_use]
    pub(crate) fn diagnostic_path(&self) -> &Path {
        &self.diagnostic_path
    }

    pub(crate) fn diagnostic_entry(&self, name: &OsStr) -> PathBuf {
        self.diagnostic_path.join(name)
    }
}

/// Create or accept one direct portable child directory beneath a bound parent.
pub fn create_or_open_flat_directory_bound(
    parent: &impl AsFd,
    name: &OsStr,
    mode: u32,
    diagnostic_parent: &Path,
) -> Result<FlatDirectory, FlatDirectoryError> {
    validate_portable_name(name)?;
    let diagnostic_path = diagnostic_parent.join(name);
    create_directory_bound(parent, name, mode)
        .map_err(|error| map_create_directory_error(error, diagnostic_path.clone()))?;
    let Some((directory, identity)) =
        open_verified_child_directory(parent, name, &diagnostic_path)?
    else {
        return Err(FlatDirectoryError::EnumerationChanged {
            path: diagnostic_path,
        });
    };
    Ok(FlatDirectory {
        directory,
        identity,
        diagnostic_path,
    })
}

/// Open one direct portable child directory beneath a bound parent without creating it.
pub fn open_flat_directory_bound(
    parent: &impl AsFd,
    name: &OsStr,
    diagnostic_parent: &Path,
) -> Result<Option<FlatDirectory>, FlatDirectoryError> {
    validate_portable_name(name)?;
    let diagnostic_path = diagnostic_parent.join(name);
    let Some((directory, identity)) =
        open_verified_child_directory(parent, name, &diagnostic_path)?
    else {
        return Ok(None);
    };
    Ok(Some(FlatDirectory {
        directory,
        identity,
        diagnostic_path,
    }))
}

impl AsFd for FlatDirectory {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.directory.as_fd()
    }
}

/// List direct entries, returning `None` instead of a partial list above `maximum`.
pub fn list_flat_directory(
    directory: &FlatDirectory,
    maximum: usize,
) -> Result<Option<Vec<FlatDirectoryEntry>>, FlatDirectoryError> {
    directory.revalidate()?;
    let mut opened = nix::dir::Dir::openat(directory, ".", DIRECTORY_FLAGS, Mode::empty())
        .map_err(|error| {
            errno_error(
                "open flat directory for listing",
                directory.diagnostic_path().to_path_buf(),
                error,
            )
        })?;
    let mut entries = Vec::new();
    for listed in opened.iter() {
        let listed = listed.map_err(|error| {
            errno_error(
                "iterate flat directory",
                directory.diagnostic_path().to_path_buf(),
                error,
            )
        })?;
        let bytes = listed.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        if entries.len() == maximum {
            return Ok(None);
        }
        let name = OsString::from_vec(bytes.to_vec());
        flat_directory_test_hook(FlatDirectoryTestPrimitive::BeforeEntryStat);
        let entry = stat_entry(directory, &name)?.ok_or_else(|| {
            FlatDirectoryError::EnumerationChanged {
                path: directory.diagnostic_entry(&name),
            }
        })?;
        entries.push(entry);
    }
    entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    Ok(Some(entries))
}

/// Read one direct regular file while proving stable metadata and exact bytes.
pub fn read_observed_file(
    directory: &FlatDirectory,
    name: &OsStr,
) -> Result<Option<FileObservation>, FlatDirectoryError> {
    read_observed_file_bounded(directory, name, usize::MAX)
}

/// Read one direct regular file while proving stable metadata and enforcing a byte limit.
pub fn read_observed_file_bounded(
    directory: &FlatDirectory,
    name: &OsStr,
    maximum: usize,
) -> Result<Option<FileObservation>, FlatDirectoryError> {
    validate_portable_name(name)?;
    read_observed_file_unchecked_bounded(directory, name, maximum)
}

pub(crate) fn read_observed_file_unchecked(
    directory: &FlatDirectory,
    name: &OsStr,
) -> Result<Option<FileObservation>, FlatDirectoryError> {
    read_observed_file_unchecked_bounded(directory, name, usize::MAX)
}

fn read_observed_file_unchecked_bounded(
    directory: &FlatDirectory,
    name: &OsStr,
    maximum: usize,
) -> Result<Option<FileObservation>, FlatDirectoryError> {
    directory.revalidate()?;
    let path = directory.diagnostic_entry(name);
    flat_directory_test_hook(FlatDirectoryTestPrimitive::BeforeObservedFileOpen);
    let fd = match openat(directory, name, FILE_FLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ENOENT) => return Ok(None),
        Err(Errno::ELOOP) => return Err(FlatDirectoryError::NotRegular { path }),
        Err(error) => return Err(errno_error("open observed file", path, error)),
    };
    let before = fstat(&fd)
        .map_err(|error| errno_error("stat opened observed file", path.clone(), error))?;
    let entry = entry_from_stat(name.to_os_string(), &before, &path)?;
    if entry.kind != JournalEntryKind::RegularFile {
        return Err(FlatDirectoryError::NotRegular { path });
    }
    flat_directory_test_hook(FlatDirectoryTestPrimitive::AfterObservedFileStat);
    if (entry.size as u128) > (maximum as u128) {
        return Err(FlatDirectoryError::SizeLimitExceeded {
            path,
            kind: entry.kind,
            size: entry.size,
            limit: maximum,
        });
    }
    let size = usize::try_from(entry.size).map_err(|_| FlatDirectoryError::Io {
        operation: "size observed file buffer",
        path: path.clone(),
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            "observed file exceeds address space",
        ),
    })?;
    let mut file = File::from(fd);
    let mut bytes = vec![0; size];
    if let Err(source) = read_exact(&mut file, &mut bytes) {
        if source.kind() == io::ErrorKind::UnexpectedEof {
            return Err(FlatDirectoryError::IdentityChanged { path });
        }
        return Err(FlatDirectoryError::Io {
            operation: "read observed file",
            path,
            source,
        });
    }
    let after = fstat(&file).map_err(|error| {
        errno_error(
            "restat observed file",
            directory.diagnostic_entry(name),
            error,
        )
    })?;
    let after_entry = entry_from_stat(
        name.to_os_string(),
        &after,
        &directory.diagnostic_entry(name),
    )?;
    if !same_entry_metadata(&entry, &after_entry) {
        return Err(FlatDirectoryError::IdentityChanged {
            path: directory.diagnostic_entry(name),
        });
    }
    Ok(Some(FileObservation { entry, bytes }))
}

fn read_exact(reader: &mut impl Read, bytes: &mut [u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        match reader.read(&mut bytes[offset..]) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(read) => offset += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn stat_entry(
    directory: &FlatDirectory,
    name: &OsStr,
) -> Result<Option<FlatDirectoryEntry>, FlatDirectoryError> {
    match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(entry_from_stat(
            name.to_os_string(),
            &stat,
            &directory.diagnostic_entry(name),
        )?)),
        Err(Errno::ENOENT) => Ok(None),
        Err(error) => Err(errno_error(
            "stat flat-directory entry",
            directory.diagnostic_entry(name),
            error,
        )),
    }
}

fn portable_relative_components(relative: &Path) -> Result<Vec<&OsStr>, FlatDirectoryError> {
    let bytes = relative.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.starts_with(b"/") {
        return Err(FlatDirectoryError::InvalidRelativePath {
            path: relative.to_path_buf(),
            reason: "path must be a nonempty sequence of normal portable components",
        });
    }

    let mut components = Vec::new();
    for raw_component in bytes.split(|byte| *byte == b'/') {
        if raw_component.is_empty() || matches!(raw_component, b"." | b"..") {
            return Err(FlatDirectoryError::InvalidRelativePath {
                path: relative.to_path_buf(),
                reason: "path must be a nonempty sequence of normal portable components",
            });
        }
        let component = OsStr::from_bytes(raw_component);
        validate_portable_name(component)?;
        components.push(component);
    }
    Ok(components)
}

fn map_root_revalidation_error(
    error: JournalRootError,
    diagnostic_path: PathBuf,
) -> FlatDirectoryError {
    match error {
        JournalRootError::Changed => FlatDirectoryError::IdentityChanged {
            path: diagnostic_path,
        },
        JournalRootError::Io {
            operation,
            path,
            source,
        } => FlatDirectoryError::Io {
            operation,
            path,
            source,
        },
        JournalRootError::Invalid { root, reason, .. }
        | JournalRootError::Unsupported { root, reason, .. } => FlatDirectoryError::Io {
            operation: "revalidate journal root",
            path: root,
            source: io::Error::other(reason),
        },
    }
}

fn open_verified_child_directory(
    parent: &(impl AsFd + ?Sized),
    name: &OsStr,
    diagnostic_path: &Path,
) -> Result<Option<(OwnedFd, ObjectIdentity)>, FlatDirectoryError> {
    let before = match fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::ENOENT) => return Ok(None),
        Err(Errno::ELOOP) => {
            return Err(FlatDirectoryError::SymlinkRefused {
                path: diagnostic_path.to_path_buf(),
            });
        }
        Err(error) => {
            return Err(errno_error(
                "stat flat-directory descendant",
                diagnostic_path.to_path_buf(),
                error,
            ));
        }
    };
    match kind_from_stat(&before) {
        JournalEntryKind::Symlink => {
            return Err(FlatDirectoryError::SymlinkRefused {
                path: diagnostic_path.to_path_buf(),
            });
        }
        JournalEntryKind::Directory => {}
        _ => {
            return Err(FlatDirectoryError::NotDirectory {
                path: diagnostic_path.to_path_buf(),
            });
        }
    }
    flat_directory_test_hook(FlatDirectoryTestPrimitive::BeforeDescendantOpen);
    let directory = match openat(parent, name, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ELOOP) => {
            return Err(FlatDirectoryError::SymlinkRefused {
                path: diagnostic_path.to_path_buf(),
            });
        }
        Err(error) => {
            return Err(errno_error(
                "open flat-directory descendant",
                diagnostic_path.to_path_buf(),
                error,
            ));
        }
    };
    let after = fstat(&directory).map_err(|error| {
        errno_error(
            "stat opened flat-directory descendant",
            diagnostic_path.to_path_buf(),
            error,
        )
    })?;
    if kind_from_stat(&after) != JournalEntryKind::Directory {
        return Err(FlatDirectoryError::NotDirectory {
            path: diagnostic_path.to_path_buf(),
        });
    }
    let identity = identity_from_stat(&before, diagnostic_path)?;
    if identity != identity_from_stat(&after, diagnostic_path)? {
        return Err(FlatDirectoryError::IdentityChanged {
            path: diagnostic_path.to_path_buf(),
        });
    }
    Ok(Some((directory, identity)))
}

fn map_create_directory_error(error: PathError, path: PathBuf) -> FlatDirectoryError {
    match error {
        PathError::Io { source, .. } => FlatDirectoryError::Io {
            operation: "create flat-directory child",
            path,
            source,
        },
        error => FlatDirectoryError::Io {
            operation: "create flat-directory child",
            path,
            source: io::Error::new(io::ErrorKind::InvalidInput, error.to_string()),
        },
    }
}

fn validate_portable_name(name: &OsStr) -> Result<(), FlatDirectoryError> {
    let text = name
        .to_str()
        .ok_or_else(|| FlatDirectoryError::InvalidName {
            name: name.to_os_string(),
            reason: NameAdmissionReason::NotUtf8,
        })?;
    check_portable_component(text).map_err(|reason| FlatDirectoryError::InvalidName {
        name: name.to_os_string(),
        reason,
    })
}

fn checked_identifier(
    value: impl TryInto<u64>,
    operation: &'static str,
    path: &Path,
) -> Result<u64, FlatDirectoryError> {
    value.try_into().map_err(|_| FlatDirectoryError::Io {
        operation,
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidData, "identity value out of range"),
    })
}

pub(crate) fn entry_from_stat(
    name: OsString,
    stat: &FileStat,
    path: &Path,
) -> Result<FlatDirectoryEntry, FlatDirectoryError> {
    Ok(FlatDirectoryEntry {
        name,
        kind: kind_from_stat(stat),
        device: checked_identifier(stat.st_dev, "read flat-directory entry identity", path)?,
        inode: checked_identifier(stat.st_ino, "read flat-directory entry identity", path)?,
        size: stat.st_size as u64,
        mtime: native_mtime(stat),
    })
}

fn identity_from_stat(stat: &FileStat, path: &Path) -> Result<ObjectIdentity, FlatDirectoryError> {
    Ok(ObjectIdentity::from_device_inode(
        checked_identifier(stat.st_dev, "read flat-directory identity", path)?,
        checked_identifier(stat.st_ino, "read flat-directory identity", path)?,
    ))
}

fn kind_from_stat(stat: &FileStat) -> JournalEntryKind {
    JournalEntryKind::from_mode(SFlag::from_bits_truncate(stat.st_mode))
}

fn native_mtime(stat: &FileStat) -> NativeMtime {
    NativeMtime {
        seconds: stat.st_mtime,
        nanoseconds: stat.st_mtime_nsec,
    }
}

fn errno_error(operation: &'static str, path: PathBuf, error: Errno) -> FlatDirectoryError {
    FlatDirectoryError::Io {
        operation,
        path,
        source: io::Error::from_raw_os_error(error as i32),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::fs::{self, File, FileTimes};
    use std::io::{self, Read};
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::time::{Duration, UNIX_EPOCH};

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    use super::*;
    use crate::test_support::TempDir;

    fn root_and_flat() -> (TempDir, JournalRoot, FlatDirectory) {
        let temporary = TempDir::new();
        fs::create_dir(temporary.path().join("flat")).unwrap();
        let root = JournalRoot::open(temporary.path()).unwrap();
        let flat = FlatDirectory::open(&root, Path::new("flat")).unwrap();
        (temporary, root, flat)
    }

    #[test]
    fn acquisition_refuses_non_normal_paths_non_directories_and_symlinks() {
        let temporary = TempDir::new();
        fs::create_dir(temporary.path().join("directory")).unwrap();
        fs::write(temporary.path().join("file"), b"not a directory").unwrap();
        symlink("directory", temporary.path().join("link")).unwrap();
        let root = JournalRoot::open(temporary.path()).unwrap();
        for relative in [
            Path::new(""),
            Path::new("/absolute"),
            Path::new("."),
            Path::new("./directory"),
            Path::new("directory/."),
            Path::new("directory/.."),
            Path::new("flat/./file"),
        ] {
            assert!(matches!(
                FlatDirectory::open(&root, relative),
                Err(FlatDirectoryError::InvalidRelativePath { .. })
            ));
        }
        assert!(matches!(
            FlatDirectory::open(&root, Path::new("link")),
            Err(FlatDirectoryError::SymlinkRefused { .. })
        ));
        assert!(matches!(
            FlatDirectory::open(&root, Path::new("file")),
            Err(FlatDirectoryError::NotDirectory { .. })
        ));
    }

    #[test]
    fn acquisition_rejects_a_descendant_replaced_between_stat_and_open() {
        let temporary = TempDir::new();
        let descendant = temporary.path().join("flat");
        fs::create_dir(&descendant).unwrap();
        let original_metadata = fs::metadata(&descendant).unwrap();
        let original_identity = (original_metadata.dev(), original_metadata.ino());
        let root = JournalRoot::open(temporary.path()).unwrap();
        let moved_descendant = temporary.path().join("flat-original");
        let replacement = descendant.clone();
        let moved_for_hook = moved_descendant.clone();
        let (result, fired) = run_with_flat_directory_hook(
            FlatDirectoryTestPrimitive::BeforeDescendantOpen,
            move || {
                fs::rename(&replacement, moved_for_hook).unwrap();
                fs::create_dir(&replacement).unwrap();
            },
            || FlatDirectory::open(&root, Path::new("flat")),
        );
        assert!(fired);
        let moved_metadata = fs::metadata(&moved_descendant).unwrap();
        assert_eq!(
            (moved_metadata.dev(), moved_metadata.ino()),
            original_identity
        );
        let fresh_metadata = fs::metadata(&descendant).unwrap();
        assert_ne!(
            (fresh_metadata.dev(), fresh_metadata.ino()),
            original_identity
        );
        assert!(matches!(
            result,
            Err(FlatDirectoryError::IdentityChanged { .. })
        ));
    }

    #[test]
    fn bound_child_open_creates_accepts_and_observes_absence_without_creation() {
        let temporary = TempDir::new();
        let root = JournalRoot::open(temporary.path()).unwrap();

        assert!(
            open_flat_directory_bound(&root, OsStr::new("health"), root.canonical_path())
                .unwrap()
                .is_none()
        );
        assert!(!temporary.path().join("health").exists());

        let health = create_or_open_flat_directory_bound(
            &root,
            OsStr::new("health"),
            0o700,
            root.canonical_path(),
        )
        .unwrap();
        let accepted = create_or_open_flat_directory_bound(
            &root,
            OsStr::new("health"),
            0o700,
            root.canonical_path(),
        )
        .unwrap();
        let reopened =
            open_flat_directory_bound(&root, OsStr::new("health"), root.canonical_path())
                .unwrap()
                .expect("created child is openable");

        assert_eq!(health.identity(), accepted.identity());
        assert_eq!(health.identity(), reopened.identity());
        assert!(matches!(
            create_or_open_flat_directory_bound(
                &root,
                OsStr::new("health/sync"),
                0o700,
                root.canonical_path(),
            ),
            Err(FlatDirectoryError::InvalidName { .. })
        ));
    }

    #[test]
    fn listing_uses_an_all_or_nothing_overflow_sentinel_at_both_boundaries() {
        let (temporary, _root, flat) = root_and_flat();
        fs::write(temporary.path().join("flat/a"), b"a").unwrap();
        fs::write(temporary.path().join("flat/b"), b"b").unwrap();
        assert_eq!(list_flat_directory(&flat, 1).unwrap(), None);
        let entries = list_flat_directory(&flat, 2).unwrap().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a");
        assert_eq!(entries[1].name, "b");
    }

    #[test]
    fn listing_entry_disappearance_is_one_error_not_a_partial_listing() {
        let (temporary, _root, flat) = root_and_flat();
        let entry = temporary.path().join("flat/entry");
        fs::write(&entry, b"entry").unwrap();
        let (result, fired) = run_with_flat_directory_hook(
            FlatDirectoryTestPrimitive::BeforeEntryStat,
            move || fs::remove_file(&entry).unwrap(),
            || list_flat_directory(&flat, 1),
        );
        assert!(fired);
        assert!(matches!(
            result,
            Err(FlatDirectoryError::EnumerationChanged { .. })
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn listing_round_trips_non_utf8_names_without_loss() {
        let (temporary, _root, flat) = root_and_flat();
        let name = OsString::from_vec(b"entry-\xff".to_vec());
        fs::write(temporary.path().join("flat").join(&name), b"entry").unwrap();
        let entries = list_flat_directory(&flat, 1).unwrap().unwrap();
        assert_eq!(entries[0].name.as_bytes(), name.as_bytes());
    }

    #[test]
    fn observed_read_returns_exact_bytes_and_loops_for_short_reads() {
        let (temporary, _root, flat) = root_and_flat();
        fs::write(temporary.path().join("flat/record"), b"bytes").unwrap();
        let observed = read_observed_file(&flat, OsStr::new("record"))
            .unwrap()
            .unwrap();
        assert_eq!(observed.bytes, b"bytes");
        assert_eq!(observed.entry.kind, JournalEntryKind::RegularFile);

        let mut reader = ShortReader::new(b"short-read".to_vec(), 2);
        let mut bytes = vec![0; 10];
        read_exact(&mut reader, &mut bytes).unwrap();
        assert_eq!(bytes, b"short-read");
    }

    #[test]
    fn checked_identifier_accepts_a_valid_value() {
        assert_eq!(
            checked_identifier(7_i32, "read test identity", Path::new("entry")).unwrap(),
            7
        );
    }

    #[test]
    fn bounded_observed_read_enforces_the_post_open_size_limit() {
        let (temporary, _root, flat) = root_and_flat();
        let entry = temporary.path().join("flat/record");
        let exact = vec![b'x'; 16 * 1024];
        fs::write(&entry, &exact).unwrap();
        assert_eq!(
            read_observed_file_bounded(&flat, OsStr::new("record"), exact.len())
                .unwrap()
                .expect("exact limit reads")
                .bytes,
            exact
        );

        let over_limit = (16 * 1024) + 1;
        fs::write(&entry, vec![b'y'; over_limit]).unwrap();
        assert!(matches!(
            read_observed_file_bounded(&flat, OsStr::new("record"), 16 * 1024),
            Err(FlatDirectoryError::SizeLimitExceeded {
                kind: JournalEntryKind::RegularFile,
                size,
                limit: 16_384,
                ..
            }) if size == over_limit as u64
        ));

        fs::write(&entry, b"small").unwrap();
        let replacement = entry.clone();
        let (result, fired) = run_with_flat_directory_hook(
            FlatDirectoryTestPrimitive::BeforeObservedFileOpen,
            move || fs::write(replacement, vec![b'z'; over_limit]).unwrap(),
            || read_observed_file_bounded(&flat, OsStr::new("record"), 16 * 1024),
        );
        assert!(fired);
        assert!(matches!(
            result,
            Err(FlatDirectoryError::SizeLimitExceeded {
                kind: JournalEntryKind::RegularFile,
                size,
                limit: 16_384,
                ..
            }) if size == over_limit as u64
        ));
        assert_eq!(
            read_observed_file_bounded(&flat, OsStr::new("missing"), 16 * 1024).unwrap(),
            None
        );
    }

    #[test]
    fn checked_identifier_rejects_an_out_of_range_value() {
        let error =
            checked_identifier(-1_i32, "read test identity", Path::new("entry")).unwrap_err();
        match error {
            FlatDirectoryError::Io {
                operation,
                path,
                source,
            } => {
                assert_eq!(operation, "read test identity");
                assert_eq!(path, Path::new("entry"));
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
                assert_eq!(source.to_string(), "identity value out of range");
            }
            error => panic!("expected identity conversion error, got {error:?}"),
        }
    }

    #[test]
    fn observed_read_round_trips_native_mtime_exactly() {
        let (temporary, _root, flat) = root_and_flat();
        let entry_path = temporary.path().join("flat/record");
        fs::write(&entry_path, b"record").unwrap();
        let seconds = 1_700_000_001;
        let nanoseconds = 123_456_789;
        let modified = UNIX_EPOCH + Duration::new(seconds, nanoseconds);
        File::options()
            .write(true)
            .open(&entry_path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(modified))
            .unwrap();

        let observed = read_observed_file(&flat, OsStr::new("record"))
            .unwrap()
            .unwrap();
        let expected_mtime = NativeMtime {
            seconds: seconds.try_into().unwrap(),
            nanoseconds: nanoseconds.into(),
        };
        assert_eq!(observed.entry.mtime, expected_mtime);
    }

    #[test]
    fn observed_read_rejects_early_eof_and_growth_after_initial_stat() {
        let (temporary, _root, flat) = root_and_flat();
        let entry = temporary.path().join("flat/record");
        fs::write(&entry, b"old").unwrap();
        let truncate = entry.clone();
        let (result, fired) = run_with_flat_directory_hook(
            FlatDirectoryTestPrimitive::AfterObservedFileStat,
            move || fs::write(truncate, b"").unwrap(),
            || read_observed_file(&flat, OsStr::new("record")),
        );
        assert!(fired);
        assert!(matches!(
            result,
            Err(FlatDirectoryError::IdentityChanged { .. })
        ));

        fs::write(&entry, b"old").unwrap();
        let grow = entry.clone();
        let (result, fired) = run_with_flat_directory_hook(
            FlatDirectoryTestPrimitive::AfterObservedFileStat,
            move || fs::write(grow, b"newer").unwrap(),
            || read_observed_file(&flat, OsStr::new("record")),
        );
        assert!(fired);
        assert!(matches!(
            result,
            Err(FlatDirectoryError::IdentityChanged { .. })
        ));
    }

    #[test]
    fn observed_read_refuses_every_supported_non_regular_kind() {
        let (temporary, _root, flat) = root_and_flat();
        let parent = temporary.path().join("flat");
        fs::write(parent.join("target"), b"target").unwrap();
        symlink("target", parent.join("link")).unwrap();
        mkfifo(&parent.join("fifo"), Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
        fs::create_dir(parent.join("directory")).unwrap();
        for name in ["link", "fifo", "directory"] {
            assert!(matches!(
                read_observed_file(&flat, OsStr::new(name)),
                Err(FlatDirectoryError::NotRegular { .. })
            ));
        }
    }

    #[test]
    fn observed_read_refuses_a_regular_leaf_replaced_before_open() {
        let (temporary, _root, flat) = root_and_flat();
        let entry = temporary.path().join("flat/record");
        fs::write(&entry, b"regular").unwrap();
        let replacement = entry.clone();
        let (result, fired) = run_with_flat_directory_hook(
            FlatDirectoryTestPrimitive::BeforeObservedFileOpen,
            move || {
                fs::remove_file(&replacement).unwrap();
                fs::create_dir(&replacement).unwrap();
            },
            || read_observed_file(&flat, OsStr::new("record")),
        );
        assert!(fired);
        assert!(matches!(result, Err(FlatDirectoryError::NotRegular { .. })));
    }

    #[test]
    fn stable_comparison_includes_identity_but_excludes_atime_and_ctime() {
        let entry = FlatDirectoryEntry {
            name: OsString::from("entry"),
            kind: JournalEntryKind::RegularFile,
            device: 1,
            inode: 2,
            size: 3,
            mtime: NativeMtime {
                seconds: 4,
                nanoseconds: 5,
            },
        };
        let same_bytes_new_inode = FlatDirectoryEntry {
            inode: 6,
            ..entry.clone()
        };
        assert!(!same_entry_metadata(&entry, &same_bytes_new_inode));

        // `FlatDirectoryEntry` intentionally has no atime or ctime field, so
        // equal exposed stable fields remain equal when only either changes.
        assert!(same_entry_metadata(&entry, &entry.clone()));
    }

    #[test]
    fn observed_read_refuses_the_full_portable_leaf_name_matrix_before_io() {
        let (_temporary, _root, flat) = root_and_flat();
        let over_limit = "a".repeat(256);
        let invalid = [
            OsString::from(""),
            OsString::from("a/b"),
            OsString::from(r"a\b"),
            OsString::from("."),
            OsString::from(".."),
            OsString::from("trailing."),
            OsString::from("trailing "),
            OsString::from("CON"),
            OsString::from(over_limit),
            OsString::from_vec(b"embedded\0nul".to_vec()),
        ];
        for name in invalid {
            assert!(matches!(
                read_observed_file(&flat, &name),
                Err(FlatDirectoryError::InvalidName { .. })
            ));
        }
    }

    struct ShortReader {
        chunks: VecDeque<u8>,
        maximum: usize,
    }

    impl ShortReader {
        fn new(bytes: Vec<u8>, maximum: usize) -> Self {
            Self {
                chunks: bytes.into(),
                maximum,
            }
        }
    }

    impl Read for ShortReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer.len().min(self.maximum).min(self.chunks.len());
            for destination in &mut buffer[..count] {
                *destination = self.chunks.pop_front().expect("count bounds chunks");
            }
            Ok(count)
        }
    }
}
