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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ObjectIdentity(Repr);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
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
    /// Canonical lossless physical-source observation. This is provenance, not
    /// a logical journal ID, destination constraint, or reconstructible handle.
    pub fn observation_id(&self) -> String {
        match self.0 {
            #[cfg(unix)]
            Repr::Unix { dev, ino } => format!("native-root-v1:unix:{dev:016x}:{ino:016x}"),
            #[cfg(windows)]
            Repr::Windows {
                volume_serial,
                file_id,
            } => {
                use std::fmt::Write;
                let mut encoded = format!("native-root-v1:windows:{volume_serial:016x}:");
                for byte in file_id {
                    write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
                }
                encoded
            }
        }
    }

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

/// Validate either platform's observation syntax on every OS. Parsing this
/// string never reconstructs an ObjectIdentity or grants mutation authority.
pub fn is_canonical_identity_observation(value: &str) -> bool {
    fn hex(value: &str, width: usize) -> bool {
        value.len() == width
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
    let mut parts = value.split(':');
    if parts.next() != Some("native-root-v1") {
        return false;
    }
    let width = match parts.next() {
        Some("unix") => 16,
        Some("windows") => 32,
        _ => return false,
    };
    let Some(volume) = parts.next() else {
        return false;
    };
    let Some(object) = parts.next() else {
        return false;
    };
    parts.next().is_none() && hex(volume, 16) && hex(object, width)
}

#[cfg(test)]
mod identity_observation_tests {
    use super::*;

    #[test]
    fn both_platforms_parse_canonically_without_authority() {
        let unix = "native-root-v1:unix:0000000000000000:ffffffffffffffff";
        let windows = "native-root-v1:windows:ffffffffffffffff:01000000000000000100000000000000";
        for value in [
            unix,
            windows,
            "native-root-v1:unix:0000000000000000:0000000000000000",
            "native-root-v1:windows:0000000000000000:00000000000000000000000000000000",
        ] {
            assert!(is_canonical_identity_observation(value));
        }
        for value in [
            "native-root-v2:unix:0000000000000000:ffffffffffffffff",
            "native-root-v1:unix:0:ffffffffffffffff",
            "native-root-v1:unix:0000000000000000:FFFFFFFFFFFFFFFF",
            "native-root-v1:unix:0000000000000000:ffffffffffffffff:extra",
            "native-root-v1:windows:0000000000000000:0000000000000000",
            "native-root-v1:other:0000000000000000:0000000000000000",
            "native-root-v1:unix:0000000000000000:",
        ] {
            assert!(!is_canonical_identity_observation(value), "{value}");
        }
        #[cfg(unix)]
        assert_eq!(
            ObjectIdentity::from_device_inode(0, u64::MAX).observation_id(),
            unix
        );
        #[cfg(windows)]
        {
            let mut bytes = [0; 16];
            bytes[0] = 1;
            bytes[8] = 1;
            let observed = ObjectIdentity::from_volume_and_file_id(u64::MAX, bytes);
            assert_eq!(observed.observation_id(), windows);
            assert_ne!(
                observed.observation_id(),
                ObjectIdentity::from_volume_and_file_id(u64::MAX, [0; 16]).observation_id()
            );
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

/// A retained directory identity and canonical binding, without publication authority.
/// Mobile sandbox ancestors can be searchable without being listable. Unlike a
/// `JournalRoot`, this witness exposes no descriptor for reads, writes or sync.
pub struct DirectoryIdentityWitness {
    root: JournalRoot,
}

impl DirectoryIdentityWitness {
    /// Retain an identity using search-only authority where the platform supports it.
    pub fn open(path: &Path) -> Result<Self, JournalRootError> {
        Ok(Self {
            root: JournalRoot {
                #[cfg(unix)]
                inner: unix::acquire_identity(path)?,
                #[cfg(windows)]
                inner: windows::acquire(path)?,
            },
        })
    }

    /// Refuse canonical replacement, including a different directory with the same name.
    pub fn revalidate_canonical_binding(&self) -> Result<(), JournalRootError> {
        self.root.revalidate_canonical_binding()
    }
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

    /// Acquire a requested Unix directory without following its final component.
    #[cfg(unix)]
    pub fn open_no_follow(root: &Path) -> Result<Self, JournalRootError> {
        Ok(Self {
            inner: unix::acquire_no_follow(root)?,
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
