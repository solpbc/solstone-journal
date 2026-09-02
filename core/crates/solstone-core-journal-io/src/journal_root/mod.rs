// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unix and Windows journal-root acquisition capability.
//!
//! [`JournalRoot`] retains one admitted journal directory. The stored canonical
//! path is diagnostic metadata, not source authority.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::fd::{AsFd, BorrowedFd};
#[cfg(windows)]
use std::os::windows::io::{AsHandle, BorrowedHandle};

#[cfg(unix)]
use nix::sys::stat::SFlag;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use backend::Backend;

#[cfg(all(unix, feature = "test-hooks"))]
pub use unix::{AcquisitionPrimitive, run_with_acquisition_fault};
#[cfg(all(windows, feature = "test-hooks"))]
pub use windows::{
    WindowsAcquisitionPrimitive, WindowsAcquisitionTrace, run_with_windows_acquisition_fault,
    run_with_windows_acquisition_trace,
};

/// The no-follow file kind observed for a journal entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalEntryKind {
    RegularFile,
    Directory,
    Symlink,
    Fifo,
    Socket,
    CharacterDevice,
    BlockDevice,
    Other,
}

impl JournalEntryKind {
    /// Classify a Unix `st_mode` file-type mask without following a symlink.
    #[cfg(unix)]
    pub fn from_mode(mode: SFlag) -> Self {
        match mode & SFlag::S_IFMT {
            SFlag::S_IFREG => Self::RegularFile,
            SFlag::S_IFDIR => Self::Directory,
            SFlag::S_IFLNK => Self::Symlink,
            SFlag::S_IFIFO => Self::Fifo,
            SFlag::S_IFSOCK => Self::Socket,
            SFlag::S_IFCHR => Self::CharacterDevice,
            SFlag::S_IFBLK => Self::BlockDevice,
            _ => Self::Other,
        }
    }
}

/// Failure while acquiring or revalidating a journal root.
#[derive(Debug)]
pub enum JournalRootError {
    /// The requested journal root is not a usable absolute directory.
    Invalid {
        root: PathBuf,
        reason: &'static str,
        category: Option<WindowsRefusalCategory>,
    },
    /// The current backend cannot retain a handle for this root.
    Unsupported {
        root: PathBuf,
        reason: &'static str,
        category: Option<WindowsRefusalCategory>,
    },
    /// A source operation failed without evidence of a replacement race.
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    /// The acquired root changed after observation.
    Changed,
}

impl fmt::Display for JournalRootError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { root, reason, .. } => {
                write!(
                    formatter,
                    "invalid journal root {}: {reason}",
                    root.display()
                )
            }
            Self::Unsupported { root, reason, .. } => {
                write!(
                    formatter,
                    "unsupported journal root {}: {reason}",
                    root.display()
                )
            }
            Self::Io {
                operation, source, ..
            } => write!(formatter, "{operation}: {source}"),
            Self::Changed => formatter.write_str("journal source changed during acquisition"),
        }
    }
}

impl Error for JournalRootError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Invalid { .. } | Self::Unsupported { .. } | Self::Changed => None,
        }
    }
}

/// Windows policy refusal classes for journal-root admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsRefusalCategory {
    NonAbsolutePath,
    NonCanonicalPath,
    NonFullyQualifiedNamespace,
    NonLocalVolume,
    RemovableOrOpticalVolume,
    RamDiskVolume,
    UnknownVolumeType,
    UnsupportedFilesystem,
    ReparsePoint,
    CloudSyncRootRegistered,
    CloudSyncRootStatusUnverifiable(Option<i32>),
    InvalidJournalName,
}

#[cfg(all(test, windows))]
impl WindowsRefusalCategory {
    pub(crate) fn raw_hresult(&self) -> Option<i32> {
        match self {
            Self::CloudSyncRootRegistered => Some(0),
            Self::CloudSyncRootStatusUnverifiable(code) => *code,
            _ => None,
        }
    }
}

/// Opaque platform-specific identity of an admitted journal root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectIdentity(Repr);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Repr {
    #[cfg(unix)]
    Unix { dev: u64, ino: u64 },
    #[cfg(windows)]
    Windows {
        volume_serial: u64,
        file_id: [u8; 16],
    },
}

impl ObjectIdentity {
    /// Construct an identity from a no-follow Unix stat result.
    #[cfg(unix)]
    pub(crate) const fn from_device_inode(dev: u64, ino: u64) -> Self {
        Self(Repr::Unix { dev, ino })
    }

    /// Construct an identity from a retained Windows file handle query.
    #[cfg(windows)]
    pub(crate) const fn from_volume_and_file_id(volume_serial: u64, file_id: [u8; 16]) -> Self {
        Self(Repr::Windows {
            volume_serial,
            file_id,
        })
    }

    /// Compare a Windows directory-listing file identifier with this retained identity.
    #[cfg(windows)]
    pub(crate) fn matches_windows_file_id(self, file_id: [u8; 16]) -> bool {
        match self.0 {
            Repr::Windows {
                file_id: observed, ..
            } => observed == file_id,
        }
    }
}

mod backend {
    use std::path::Path;

    use super::{JournalRootError, ObjectIdentity};

    pub(crate) trait Backend {
        fn identity(&self) -> ObjectIdentity;
        fn diagnostic_path(&self) -> &Path;
        fn revalidate(&self) -> Result<(), JournalRootError>;
        fn revalidate_canonical_binding(&self) -> Result<(), JournalRootError>;
    }
}

/// One admitted journal directory, retained by descriptor.
///
/// Not [`Clone`]: a cloned descriptor would be a second capability, and a cloned
/// path would be reacquisition. The canonical path is non-authoritative
/// metadata recorded at admit time.
pub struct JournalRoot {
    #[cfg(unix)]
    inner: unix::UnixRoot,
    #[cfg(windows)]
    inner: windows::WindowsRoot,
}

impl JournalRoot {
    /// Acquire `root` once and retain its directory descriptor.
    pub fn open(root: &Path) -> Result<Self, JournalRootError> {
        Ok(Self {
            #[cfg(unix)]
            inner: unix::acquire(root)?,
            #[cfg(windows)]
            inner: windows::acquire(root)?,
        })
    }

    /// Return the opaque identity frozen when this root was admitted.
    pub fn identity(&self) -> ObjectIdentity {
        self.inner.identity()
    }

    /// Return the verified canonical path recorded at admit time.
    ///
    /// This spelling is diagnostic metadata, not source authority.
    pub fn canonical_path(&self) -> &Path {
        self.inner.diagnostic_path()
    }

    /// Confirm the retained descriptor still names the admitted directory identity.
    pub fn revalidate(&self) -> Result<(), JournalRootError> {
        self.inner.revalidate()
    }

    /// Prove the recorded canonical pathname still resolves, no-follow,
    /// component-by-component, to the retained root identity.
    pub fn revalidate_canonical_binding(&self) -> Result<(), JournalRootError> {
        self.inner.revalidate_canonical_binding()
    }
}

#[cfg(unix)]
impl AsFd for JournalRoot {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(windows)]
impl AsHandle for JournalRoot {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.inner.as_handle()
    }
}

#[cfg(windows)]
impl JournalRoot {
    /// Borrow the separately admitted overlapped handle reserved for namespace witnessing.
    pub(crate) fn as_namespace_watch_handle(&self) -> BorrowedHandle<'_> {
        self.inner.as_watch_handle()
    }
}

#[cfg(test)]
mod architecture {
    const UNIX: &str = include_str!("unix.rs");
    const WINDOWS: &str = include_str!("windows.rs");
    const MOD: &str = include_str!("mod.rs");

    fn production_source(source: &str) -> &str {
        [
            "\n#[cfg(test)]\nmod tests",
            "\n#[cfg(test)]\nmod architecture",
        ]
        .into_iter()
        .filter_map(|boundary| source.find(boundary))
        .min()
        .map_or(source, |boundary| &source[..boundary])
    }

    #[test]
    fn filesystem_root_reopens_are_literal_and_argument_free() {
        assert!(
            UNIX.contains(
                "fn open_absolute_filesystem_root() -> Result<OwnedFd, JournalRootError>"
            )
        );
        assert!(UNIX.contains("open(\"/\", DIRECTORY_FLAGS, Mode::empty())"));
        assert_eq!(
            UNIX.matches("open_absolute_filesystem_root()?").count(),
            3,
            "two root-self reopens plus the non-root traversal open"
        );
        assert!(!UNIX.contains("OsString::from(\".\")"));
        assert!(!UNIX.contains("openat(&authoritative"));
        assert!(!UNIX.contains("openat(&first"));
    }

    #[test]
    fn journal_root_sources_are_read_only() {
        for (name, source) in [("mod", MOD), ("unix", UNIX), ("windows", WINDOWS)] {
            let production = production_source(source);
            for forbidden in [
                "fs::write",
                "create_dir(",
                "create_dir_all(",
                "remove_file(",
                "remove_dir(",
                "remove_dir_all(",
                "fs::rename",
                "fs::copy",
                "hard_link",
                "set_permissions",
                "File::create",
                "File::create_new",
                "OpenOptions",
                "DirBuilder",
                "unix::fs::symlink",
                "unix::fs::chown",
                "lchown",
                "chroot",
                "UnixListener",
                "mkfifo",
                "mkdirat",
                "unlinkat",
                "rmdir",
                "linkat",
                "symlinkat",
                "renameat",
                "mknod",
                "tokio::fs",
                "libc::rename",
                "libc::unlink",
                "libc::symlink",
                "libc::link",
                "libc::mkdir",
                "libc::mkfifo",
                "libc::mknod",
                "libc::FILE",
                "unistd::mkdir",
                "unistd::unlink",
                "unistd::link",
            ] {
                assert!(
                    !production.contains(forbidden),
                    "journal_root {name} reaches write primitive {forbidden}"
                );
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use nix::sys::stat::SFlag;

    use super::JournalEntryKind;

    #[test]
    fn from_mode_maps_each_kind_once() {
        let cases = [
            (
                SFlag::S_IFREG | SFlag::from_bits_truncate(0o644),
                JournalEntryKind::RegularFile,
            ),
            (SFlag::S_IFDIR, JournalEntryKind::Directory),
            (SFlag::S_IFLNK, JournalEntryKind::Symlink),
            (SFlag::S_IFIFO, JournalEntryKind::Fifo),
            (SFlag::S_IFSOCK, JournalEntryKind::Socket),
            (SFlag::S_IFCHR, JournalEntryKind::CharacterDevice),
            (SFlag::S_IFBLK, JournalEntryKind::BlockDevice),
            (SFlag::from_bits_truncate(0), JournalEntryKind::Other),
        ];
        let mut seen = Vec::new();
        for (mode, expected) in cases {
            let kind = JournalEntryKind::from_mode(mode);
            assert_eq!(kind, expected);
            assert!(
                !seen.contains(&kind),
                "kind {kind:?} mapped from more than one mask"
            );
            seen.push(kind);
        }
        assert_eq!(seen.len(), 8);
    }
}
