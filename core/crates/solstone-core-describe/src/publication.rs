// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable private files for the short Describe publication boundary.

use sha2::{Digest, Sha256};
use solstone_core_journal_io::JournalRoot;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct StagedFile {
    parent: Arc<JournalRoot>,
    stage: OsString,
    destination: OsString,
    file: File,
    facts: Metadata,
    installed: std::cell::Cell<bool>,
}

struct CheckedWriter<'a> {
    file: &'a mut File,
    stop: &'a dyn Fn() -> bool,
}
impl Write for CheckedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        check(self.stop)?;
        self.file.write(&bytes[..bytes.len().min(64 * 1024)])
    }
    fn flush(&mut self) -> io::Result<()> {
        check(self.stop)?;
        self.file.flush()
    }
}
fn check(stop: &dyn Fn() -> bool) -> io::Result<()> {
    if stop() {
        Err(io::Error::other("describe cancelled"))
    } else {
        Ok(())
    }
}

impl StagedFile {
    pub(crate) fn stage_prefix(destination: &Path) -> String {
        let digest = format!(
            "{:x}",
            Sha256::digest(
                destination
                    .file_name()
                    .unwrap_or_default()
                    .as_encoded_bytes()
            )
        );
        format!(".describe-stage-{}-", &digest[..16])
    }

    /// Populate and flush before the caller acquires final publication authority.
    /// No destination is created or replaced until `install`.
    pub(crate) fn prepare(
        destination: &Path,
        stop: &dyn Fn() -> bool,
        write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<Self> {
        check(stop)?;
        let parent = JournalRoot::open(
            destination
                .parent()
                .ok_or_else(|| io::Error::other("publication parent missing"))?,
        )
        .map_err(io::Error::other)?;
        let name = destination
            .file_name()
            .ok_or_else(|| io::Error::other("publication leaf missing"))?;
        Self::prepare_in(Arc::new(parent), name, stop, write)
    }

    pub(crate) fn prepare_in(
        parent: Arc<JournalRoot>,
        destination: &OsStr,
        stop: &dyn Fn() -> bool,
        write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<Self> {
        let observed_cancel = std::cell::Cell::new(false);
        let owner_stop = stop;
        let stop = &|| {
            if owner_stop() {
                observed_cancel.set(true);
            }
            observed_cancel.get()
        };
        if Path::new(destination).components().count() != 1
            || !matches!(
                Path::new(destination).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(io::Error::other(
                "publication leaf is not a plain component",
            ));
        }
        check(stop)?;
        let destination = destination.to_owned();
        let (stage, mut file) = loop {
            let stage = OsString::from(format!(
                "{}{}-{}.tmp",
                Self::stage_prefix(Path::new(&destination)),
                std::process::id(),
                NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
            ));
            #[cfg(unix)]
            let opened = {
                use nix::fcntl::{OFlag, openat};
                use nix::sys::stat::Mode;
                openat(
                    parent.as_ref(),
                    stage.as_os_str(),
                    OFlag::O_WRONLY
                        | OFlag::O_CREAT
                        | OFlag::O_EXCL
                        | OFlag::O_NOFOLLOW
                        | OFlag::O_CLOEXEC,
                    Mode::from_bits_truncate(0o600),
                )
                .map(File::from)
                .map_err(io::Error::from)
            };
            #[cfg(windows)]
            let opened = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(parent.canonical_path().join(&stage));
            match opened {
                Ok(file) => break (stage, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let result = (|| {
            write(&mut CheckedWriter {
                file: &mut file,
                stop,
            })?;
            check(stop)?;
            #[cfg(unix)]
            solstone_core_journal_io::atomic::sync_file(&file)?;
            #[cfg(windows)]
            file.sync_all()?;
            check(stop)?;
            parent
                .revalidate_canonical_binding()
                .map_err(io::Error::other)?;
            let facts = file.metadata()?;
            #[cfg(windows)]
            let retained = {
                let reader = File::open(parent.canonical_path().join(&stage))?;
                if solstone_core_journal_io::windows_file_identity(&reader)?
                    != solstone_core_journal_io::windows_file_identity(&file)?
                {
                    return Err(io::Error::other("prepared publication identity changed"));
                }
                Some(reader)
            };
            #[cfg(unix)]
            let retained = None::<File>;
            Ok((facts, retained))
        })();
        match result {
            Ok((facts, retained)) => {
                let (file, facts) = if let Some(reader) = retained {
                    // Windows finalizes last-write metadata when writers close.
                    drop(file);
                    let facts = reader.metadata().inspect_err(|_| {
                        remove_owned_stage(parent.as_ref(), &stage, &reader);
                    })?;
                    (reader, facts)
                } else {
                    (file, facts)
                };
                Ok(Self {
                    parent,
                    stage,
                    destination,
                    file,
                    facts,
                    installed: std::cell::Cell::new(false),
                })
            }
            Err(error) => {
                remove_owned_stage(parent.as_ref(), &stage, &file);
                let error = if observed_cancel.get() {
                    io::Error::other("describe cancelled")
                } else {
                    error
                };

                Err(error)
            }
        }
    }

    pub(crate) fn validate_stage(&self) -> io::Result<()> {
        self.parent
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let named = fs::symlink_metadata(self.parent.canonical_path().join(&self.stage))?;
        let opened = self.file.metadata()?;
        if !named.file_type().is_file() || !same(&named, &self.facts) || !same(&opened, &self.facts)
        {
            return Err(io::Error::other("prepared publication changed"));
        }
        #[cfg(windows)]
        {
            let named = File::open(self.parent.canonical_path().join(&self.stage))?;
            if solstone_core_journal_io::windows_file_identity(&named)?
                != solstone_core_journal_io::windows_file_identity(&self.file)?
            {
                return Err(io::Error::other("prepared publication identity changed"));
            }
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        self.validate_stage()?;
        let destination = self.parent.canonical_path().join(&self.destination);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(io::Error::other(
                    "publication destination is not a plain file",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    /// The caller has completed all long work and accepted the final commit.
    pub(crate) fn install(&self) -> io::Result<()> {
        self.validate()?;
        #[cfg(unix)]
        {
            nix::fcntl::renameat(
                self.parent.as_ref(),
                self.stage.as_os_str(),
                self.parent.as_ref(),
                self.destination.as_os_str(),
            )
            .map_err(io::Error::from)?;
            self.installed.set(true);
            // Match JournalIO's existing post-publication directory-sync policy:
            // the rename is already visible and must finish as a committed job.
            if let Err(error) = nix::unistd::fsync(self.parent.as_ref()) {
                log::warn!("describe publication parent sync failed: {error}");
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            solstone_core_journal_io::install_file(
                self.parent.canonical_path().join(&self.stage),
                self.parent.canonical_path().join(&self.destination),
                Default::default(),
            )
            .map_err(io::Error::other)
        }
    }
}

fn same(a: &Metadata, b: &Metadata) -> bool {
    let (Ok(a_modified), Ok(b_modified)) = (a.modified(), b.modified()) else {
        return false;
    };
    if a.len() != b.len() || a_modified != b_modified {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (a.dev(), a.ino(), a.ctime(), a.ctime_nsec())
            == (b.dev(), b.ino(), b.ctime(), b.ctime_nsec())
    }
    #[cfg(windows)]
    {
        true
    }
}

fn remove_owned_stage(parent: &JournalRoot, stage: &OsStr, file: &File) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(named), Ok(opened)) = (
            nix::sys::stat::fstatat(parent, stage, nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW),
            file.metadata(),
        ) && (named.st_dev, named.st_ino) == (opened.dev(), opened.ino())
        {
            let _ = nix::unistd::unlinkat(parent, stage, nix::unistd::UnlinkatFlags::NoRemoveDir);
        }
    }
    #[cfg(windows)]
    {
        let path = parent.canonical_path().join(stage);
        if let Ok(named) = File::open(&path)
            && let (Ok(named_id), Ok(opened_id)) = (
                solstone_core_journal_io::windows_file_identity(&named),
                solstone_core_journal_io::windows_file_identity(file),
            )
            && named_id == opened_id
        {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if !self.installed.get() {
            remove_owned_stage(self.parent.as_ref(), &self.stage, &self.file);
        }
    }
}

/// A cached sibling may contribute only while its admitted bytes stay bound.
pub(crate) struct SourceWitness {
    path: PathBuf,
    file: File,
    facts: Metadata,
}
impl SourceWitness {
    pub(crate) fn capture(
        path: &Path,
        expected_sha256: &str,
        stop: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        let named = fs::symlink_metadata(path)?;
        if !named.file_type().is_file() {
            return Err(io::Error::other("OCR source is not a plain file"));
        }
        #[cfg(unix)]
        let mut file = File::open(path)?;
        #[cfg(windows)]
        let mut file = {
            use std::os::windows::fs::OpenOptionsExt;
            // Keep deletion possible while excluding same-object writes whose
            // last-write timestamp may not be visible until their handle closes.
            fs::OpenOptions::new()
                .read(true)
                .share_mode(0x00000001 | 0x00000004)
                .open(path)?
        };
        let facts = file.metadata()?;
        if !same(&named, &facts) {
            return Err(io::Error::other("OCR source changed while opening"));
        }
        let mut hash = Sha256::new();
        let mut bytes = [0u8; 64 * 1024];
        loop {
            check(stop)?;
            let count = file.read(&mut bytes)?;
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
        }
        if format!("{:x}", hash.finalize()) != expected_sha256 {
            return Err(io::Error::other("OCR source bytes changed"));
        }
        let witness = Self {
            path: path.to_owned(),
            file,
            facts,
        };
        witness.validate()?;
        Ok(witness)
    }
    pub(crate) fn validate(&self) -> io::Result<()> {
        let named = fs::symlink_metadata(&self.path)?;
        if !named.file_type().is_file()
            || !same(&named, &self.facts)
            || !same(&self.file.metadata()?, &self.facts)
        {
            return Err(io::Error::other("OCR source changed before publication"));
        }
        #[cfg(windows)]
        {
            let current = File::open(&self.path)?;
            if solstone_core_journal_io::windows_file_identity(&current)?
                != solstone_core_journal_io::windows_file_identity(&self.file)?
            {
                return Err(io::Error::other("OCR source identity changed"));
            }
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    #[test]
    fn cancelled_staging_preserves_the_destination_and_removes_private_bytes() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("screen.jsonl");
        fs::write(&output, b"prior reading").unwrap();
        let stop = std::cell::Cell::new(false);
        let result = StagedFile::prepare(&output, &|| stop.get(), |writer| {
            writer.write_all(&vec![7u8; 256 * 1024])?;
            stop.set(true);
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&output).unwrap(), b"prior reading");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn different_outputs_in_one_process_have_independent_stages() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.jsonl");
        let b = root.path().join("b.jsonl");
        let first =
            StagedFile::prepare(&a, &|| false, |writer| writer.write_all(b"first")).unwrap();
        let second =
            StagedFile::prepare(&b, &|| false, |writer| writer.write_all(b"second")).unwrap();
        assert!(!a.exists() && !b.exists());
        first.install().unwrap();
        second.install().unwrap();
        assert_eq!(fs::read(a).unwrap(), b"first");
        assert_eq!(fs::read(b).unwrap(), b"second");
    }

    #[cfg(unix)]
    #[test]
    fn replaced_parent_or_stage_cannot_receive_publication() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("segment");
        fs::create_dir(&parent).unwrap();
        let output = parent.join("screen.jsonl");
        let staged =
            StagedFile::prepare(&output, &|| false, |writer| writer.write_all(b"reading")).unwrap();
        fs::rename(&parent, root.path().join("moved")).unwrap();
        fs::create_dir(&parent).unwrap();
        assert!(staged.install().is_err());
        assert!(!output.exists());
        drop(staged);
        assert_eq!(fs::read_dir(root.path().join("moved")).unwrap().count(), 0);

        let staged =
            StagedFile::prepare(&output, &|| false, |writer| writer.write_all(b"reading")).unwrap();
        let name = parent.join(&staged.stage);
        fs::remove_file(&name).unwrap();
        fs::write(&name, b"replacement").unwrap();
        assert!(staged.install().is_err());
        assert!(!output.exists());
        drop(staged);
        assert_eq!(fs::read(name).unwrap(), b"replacement");
    }
}
