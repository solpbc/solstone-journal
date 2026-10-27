// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Transient Unix socket names retained by the journal's serving owner.

#[cfg(any(target_os = "linux", target_os = "android"))]
use std::os::fd::AsRawFd;
use std::{
    ffi::OsStr,
    io,
    os::fd::{AsFd, BorrowedFd},
    path::{Path, PathBuf},
    sync::Arc,
};

use nix::{
    errno::Errno,
    fcntl::{AtFlags, readlinkat},
    sys::stat::{FileStat, SFlag, fstatat},
    unistd::{UnlinkatFlags, symlinkat, unlinkat},
};

use crate::{JournalRoot, journal_directory::JournalNamespace};

const NAME: &str = "callosum.sock";

enum Parent {
    Health(Arc<crate::journal_directory::JournalDirectory>),
    Private(Arc<JournalRoot>),
}

impl AsFd for Parent {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Self::Health(parent) => parent.as_fd(),
            Self::Private(parent) => parent.as_fd(),
        }
    }
}

pub struct JournalSocketEndpoint {
    namespace: JournalNamespace,
    parent: Parent,
    target: PathBuf,
    created: Option<(u64, u64)>,
}

impl JournalSocketEndpoint {
    /// Capture both parents before installing the app-owned alias. A missing
    /// health directory is created through the original root descriptor.
    pub fn prepare(root: Arc<JournalRoot>, private_target: Option<&Path>) -> io::Result<Self> {
        root.revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let private = private_target
            .map(|target| {
                if target.file_name() != Some(OsStr::new(NAME))
                    || !target.is_absolute()
                    || target.as_os_str().len() >= 104
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid private socket path",
                    ));
                }
                JournalRoot::open(target.parent().expect("absolute socket has parent"))
                    .map(Arc::new)
                    .map_err(io::Error::other)
            })
            .transpose()?;
        let namespace = JournalNamespace::new(root);
        let health = namespace
            .directory(Path::new("health"), true)?
            .expect("requested health creation");
        if let (Some(target), Some(private)) = (private_target, &private) {
            private
                .revalidate_canonical_binding()
                .map_err(io::Error::other)?;
            namespace.revalidate()?;
            match observe(health.as_ref())? {
                Some(status) if kind(&status) == SFlag::S_IFLNK => {
                    let previous = readlinkat(health.as_ref(), NAME).map_err(io::Error::from)?;
                    if Path::new(&previous) != target {
                        return Err(io::Error::other("socket alias has another binding"));
                    }
                }
                Some(_) => return Err(io::Error::other("socket endpoint has another binding")),
                None => symlinkat(target, health.as_ref(), NAME).map_err(io::Error::from)?,
            }
            namespace.revalidate()?;
        }
        let target = private_target
            .map(Path::to_path_buf)
            .unwrap_or_else(|| namespace.root().canonical_path().join("health").join(NAME));
        let parent = match private {
            Some(parent) => Parent::Private(parent),
            None => Parent::Health(health),
        };
        let endpoint = Self {
            namespace,
            parent,
            target,
            created: None,
        };
        endpoint.revalidate()?;
        Ok(endpoint)
    }

    pub fn revalidate(&self) -> io::Result<()> {
        self.namespace.revalidate()?;
        if let Parent::Private(parent) = &self.parent {
            parent
                .revalidate_canonical_binding()
                .map_err(io::Error::other)?;
        }
        Ok(())
    }

    /// Remove a stale socket only beneath the captured parent. Other entry
    /// kinds are preserved. Linux can also bind through this directory fd.
    pub fn prepare_bind(&self) -> io::Result<PathBuf> {
        self.revalidate()?;
        match observe(&self.parent)? {
            Some(status) if kind(&status) == SFlag::S_IFSOCK => {
                unlinkat(&self.parent, NAME, UnlinkatFlags::NoRemoveDir)
                    .map_err(io::Error::from)?;
            }
            Some(_) => return Err(io::Error::other("socket target has another binding")),
            None => {}
        }
        self.revalidate()?;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        return Ok(PathBuf::from(format!(
            "/proc/self/fd/{}/{NAME}",
            self.parent.as_fd().as_raw_fd()
        )));
    }

    /// Capture the actual created socket before the server starts any tasks.
    pub fn bound(&mut self) -> io::Result<()> {
        let status =
            observe(&self.parent)?.ok_or_else(|| io::Error::other("socket disappeared"))?;
        if kind(&status) != SFlag::S_IFSOCK {
            return Err(io::Error::other("socket target changed"));
        }
        self.created = Some(identity(&status));
        self.revalidate()
    }

    pub fn target(&self) -> &Path {
        &self.target
    }

    /// An old serving owner cannot unlink a successor's socket or a socket in
    /// a substituted directory. Root loss does not redirect this cleanup.
    pub fn cleanup(&mut self) {
        let Some(created) = self.created.take() else {
            return;
        };
        if observe(&self.parent)
            .ok()
            .flatten()
            .is_some_and(|status| kind(&status) == SFlag::S_IFSOCK && identity(&status) == created)
        {
            let _ = unlinkat(&self.parent, NAME, UnlinkatFlags::NoRemoveDir);
        }
    }
}

impl Drop for JournalSocketEndpoint {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn observe(parent: &impl AsFd) -> io::Result<Option<FileStat>> {
    match fstatat(parent, NAME, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(status) => Ok(Some(status)),
        Err(Errno::ENOENT) => Ok(None),
        Err(error) => Err(io::Error::from(error)),
    }
}

fn kind(status: &FileStat) -> SFlag {
    SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT
}

#[allow(clippy::unnecessary_cast)] // Darwin and Linux use different native widths.
fn identity(status: &FileStat) -> (u64, u64) {
    (status.st_dev as u64, status.st_ino as u64)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::{fs::MetadataExt, net::UnixListener},
    };

    #[test]
    fn refused_original_root_creates_no_successor_ipc() {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("journal");
        fs::create_dir(&path).unwrap();
        let root = Arc::new(JournalRoot::open(&path).unwrap());
        fs::rename(&path, parent.path().join("original")).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(JournalSocketEndpoint::prepare(root, None).is_err());
        assert!(fs::read_dir(&path).unwrap().next().is_none());
    }

    #[test]
    fn parent_replacement_before_bind_is_refused() {
        let journal = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let private = parent.path().join("private");
        fs::create_dir(&private).unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let endpoint = JournalSocketEndpoint::prepare(root, Some(&private.join(NAME))).unwrap();
        fs::rename(&private, parent.path().join("original")).unwrap();
        fs::create_dir(&private).unwrap();
        assert!(endpoint.prepare_bind().is_err());
        assert!(fs::read_dir(&private).unwrap().next().is_none());
    }

    #[test]
    fn cleanup_uses_the_original_parent_and_preserves_a_new_binding() {
        let journal = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let mut endpoint = JournalSocketEndpoint::prepare(root, None).unwrap();
        let _listener = UnixListener::bind(endpoint.prepare_bind().unwrap()).unwrap();
        endpoint.bound().unwrap();
        let health = journal.path().join("health");
        fs::rename(&health, journal.path().join("old-health")).unwrap();
        fs::create_dir(&health).unwrap();
        fs::write(health.join(NAME), b"successor").unwrap();
        endpoint.cleanup();
        assert!(!journal.path().join("old-health").join(NAME).exists());
        assert_eq!(fs::read(health.join(NAME)).unwrap(), b"successor");
    }

    #[test]
    fn an_old_endpoint_owner_does_not_unlink_the_next_socket() {
        let journal = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let mut old = JournalSocketEndpoint::prepare(Arc::clone(&root), None).unwrap();
        let _old_listener = UnixListener::bind(old.prepare_bind().unwrap()).unwrap();
        old.bound().unwrap();
        let old_inode = fs::symlink_metadata(old.target()).unwrap().ino();
        let mut next = JournalSocketEndpoint::prepare(root, None).unwrap();
        let _next_listener = UnixListener::bind(next.prepare_bind().unwrap()).unwrap();
        next.bound().unwrap();
        let next_inode = fs::symlink_metadata(next.target()).unwrap().ino();
        assert_ne!(old_inode, next_inode);
        old.cleanup();
        assert_eq!(
            fs::symlink_metadata(next.target()).unwrap().ino(),
            next_inode
        );
        next.cleanup();
        assert!(!next.target().exists());
    }

    #[test]
    fn private_alias_refuses_another_binding_without_touching_either_target() {
        let journal = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        let old_target = first.path().join(NAME);
        fs::write(&old_target, b"old target").unwrap();
        drop(JournalSocketEndpoint::prepare(Arc::clone(&root), Some(&old_target)).unwrap());
        let target = second.path().join(NAME);
        assert!(JournalSocketEndpoint::prepare(Arc::clone(&root), Some(&target)).is_err());
        assert_eq!(
            fs::read_link(journal.path().join("health").join(NAME)).unwrap(),
            old_target
        );
        assert_eq!(fs::read(&old_target).unwrap(), b"old target");
        assert!(!target.exists());
    }
}
