// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A read-only original descriptor, retained across a domain's proof boundary.

use super::*;
use std::sync::Mutex;

/// Read evidence only. This owner neither freezes other writers nor grants
/// retirement, publication, archive admission or helper authority.
pub struct RetainedMediaFile {
    directory: Arc<MediaDirectory>,
    file: Mutex<File>,
    entry: MediaEntry,
    digest: MediaDigest,
}

impl MediaDirectory {
    pub fn retain_file(
        self: &Arc<Self>,
        name: &OsStr,
        checkpoint: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<Option<RetainedMediaFile>> {
        checkpoint()?;
        plain_name(name)?;
        self.revalidate()?;
        let Some(file) = platform::open_source(&self.inner, name)? else {
            self.revalidate()?;
            return Ok(None);
        };
        let entry = platform::entry_from_file(name, &file)?;
        let mut owned = RetainedMediaFile {
            directory: Arc::clone(self),
            file: Mutex::new(file),
            entry,
            digest: MediaDigest {
                size: 0,
                sha256: String::new(),
            },
        };
        owned.digest = owned.read_current(checkpoint)?;
        Ok(Some(owned))
    }
}

impl RetainedMediaFile {
    pub fn entry(&self) -> &MediaEntry {
        &self.entry
    }

    pub fn digest(&self) -> &MediaDigest {
        &self.digest
    }

    /// Rehash the actual retained descriptor and verify its original name,
    /// identity and native stamp around the complete bounded streaming read.
    pub fn revalidate_contents(&self, checkpoint: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
        if self.read_current(checkpoint)? != self.digest {
            return Err(changed());
        }
        Ok(())
    }

    fn check(&self, file: &File, checkpoint: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
        checkpoint()?;
        self.directory.revalidate()?;
        if platform::entry_from_file(&self.entry.name, file)? != self.entry
            || platform::entry(&self.directory.inner, &self.entry.name)? != Some(self.entry.clone())
            || !single_link(file)?
        {
            return Err(changed());
        }
        Ok(())
    }

    fn read_current(&self, checkpoint: &dyn Fn() -> io::Result<()>) -> io::Result<MediaDigest> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| invalid("retained media read poisoned"))?;
        self.check(&file, checkpoint)?;
        file.seek(SeekFrom::Start(0))?;
        let mut buffer = [0u8; BUFFER_BYTES];
        let mut hash = Sha256::new();
        let mut size = 0u64;
        loop {
            checkpoint()?;
            let count = match file.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                break;
            }
            size = checked_length(size, count as u64)?;
            if size > self.entry.size {
                return Err(changed());
            }
            hash.update(&buffer[..count]);
        }
        self.check(&file, checkpoint)?;
        if size != self.entry.size {
            return Err(changed());
        }
        Ok(MediaDigest {
            size,
            sha256: format!("{:x}", hash.finalize()),
        })
    }
}

fn single_link(file: &File) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink() == 1)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        crate::windows_identity::file_link_count(file.as_raw_handle()).map(|links| links == 1)
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn actual_read_stays_bound_and_detects_same_inode_changes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("raw.wav");
        std::fs::write(&path, b"original bytes").unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let parent = MediaDirectory::open(root, Path::new("")).unwrap();
        let read = parent
            .retain_file(OsStr::new("raw.wav"), &|| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(read.digest().size, 14);
        read.revalidate_contents(&|| Ok(())).unwrap();
        std::fs::write(path, b"replaced bytes").unwrap();
        assert!(read.revalidate_contents(&|| Ok(())).is_err());
    }

    #[test]
    fn linked_copy_is_not_an_independent_original() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("raw.wav"), b"bytes").unwrap();
        std::fs::hard_link(temp.path().join("raw.wav"), temp.path().join("alias.wav")).unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let parent = MediaDirectory::open(root, Path::new("")).unwrap();
        assert!(
            parent
                .retain_file(OsStr::new("raw.wav"), &|| Ok(()))
                .is_err()
        );
    }

    #[test]
    fn cancellation_during_streaming_is_a_refusal() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("raw.wav"), vec![7; BUFFER_BYTES * 3]).unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let parent = MediaDirectory::open(root, Path::new("")).unwrap();
        let checks = std::cell::Cell::new(0);
        let result = parent.retain_file(OsStr::new("raw.wav"), &|| {
            checks.set(checks.get() + 1);
            if checks.get() == 4 {
                Err(io::Error::from(io::ErrorKind::Interrupted))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(temp.path().join("raw.wav")).unwrap().len(),
            BUFFER_BYTES * 3
        );
    }
}
