// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;

use sha2::{Digest, Sha256};
use solstone_core_journal_io::{AtomicWriteError, AtomicWriteOptions, write_bytes_exclusive};

use crate::{ContentName, SegmentDir, SegmentError};

/// Immutable content facts used for collision outcomes and identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentDescriptor {
    pub name: ContentName,
    pub sha256: String,
    pub size: u64,
}

/// Result of a single create-exclusive content write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContentWriteOutcome {
    Written(ContentDescriptor),
    AlreadyHeld(ContentDescriptor),
    Conflict {
        incoming: ContentDescriptor,
        existing: ContentDescriptor,
    },
}

/// Write one content file without overwriting a held filename.
pub fn write_content(
    segment: &SegmentDir,
    name: ContentName,
    bytes: &[u8],
) -> Result<ContentWriteOutcome, SegmentError> {
    let path = segment.path.join(name.as_str());
    let incoming = descriptor(name.clone(), bytes);
    match write_bytes_exclusive(&path, bytes, AtomicWriteOptions::default()) {
        Ok(()) => Ok(ContentWriteOutcome::Written(incoming)),
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists =>
        {
            let existing_bytes = fs::read(&path).map_err(|source| SegmentError::Io {
                path: path.clone(),
                source,
            })?;
            let existing = descriptor(name, &existing_bytes);
            if existing.sha256 == incoming.sha256 && existing.size == incoming.size {
                Ok(ContentWriteOutcome::AlreadyHeld(incoming))
            } else {
                Ok(ContentWriteOutcome::Conflict { incoming, existing })
            }
        }
        Err(error) => Err(SegmentError::Atomic(error)),
    }
}

pub(crate) fn descriptor(name: ContentName, bytes: &[u8]) -> ContentDescriptor {
    ContentDescriptor {
        name,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use crate::test_support::TempDir;

    use super::*;

    fn segment(root: &Path) -> SegmentDir {
        SegmentDir::resolve(root, "20260804", "120000_60", "workstation").unwrap()
    }

    #[test]
    fn classifies_same_length_conflict_without_overwriting_bytes() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        let name = ContentName::new("audio.flac").unwrap();
        assert!(matches!(
            write_content(&segment, name.clone(), b"abcd").unwrap(),
            ContentWriteOutcome::Written(_)
        ));

        let outcome = write_content(&segment, name.clone(), b"abce").unwrap();
        assert!(matches!(outcome, ContentWriteOutcome::Conflict { .. }));
        assert_eq!(fs::read(segment.path.join(name.as_str())).unwrap(), b"abcd");
    }

    #[test]
    fn classifies_identical_bytes_as_already_held_without_overwriting() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        let name = ContentName::new("audio.flac").unwrap();
        write_content(&segment, name.clone(), b"same").unwrap();

        let outcome = write_content(&segment, name.clone(), b"same").unwrap();
        assert!(matches!(outcome, ContentWriteOutcome::AlreadyHeld(_)));
        assert_eq!(fs::read(segment.path.join(name.as_str())).unwrap(), b"same");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_collision_is_an_io_error() {
        let temporary = TempDir::new();
        let segment = segment(temporary.path());
        let name = ContentName::new("audio.flac").unwrap();
        write_content(&segment, name.clone(), b"held").unwrap();
        let path = segment.path.join(name.as_str());
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(&path, permissions).unwrap();

        let result = write_content(&segment, name, b"held");
        let mut restore = fs::metadata(&path).unwrap().permissions();
        restore.set_mode(0o600);
        fs::set_permissions(&path, restore).unwrap();
        assert!(matches!(result, Err(SegmentError::Io { .. })));
    }
}
