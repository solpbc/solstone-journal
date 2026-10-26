// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Mutation authority for one owned reconciliation frame. The scope is thread
//! affine, like the native frame; ordinary desktop entry points retain their
//! existing pathname contract when no scope is installed.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Read};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use solstone_core_journal_io::journal_directory::{JournalDirectory, JournalNamespace};
use solstone_core_journal_io::{FileLock, JournalRoot, LockOptions};

struct Owner {
    root: Arc<JournalRoot>,
    namespace: Option<JournalNamespace>,
    refused: Arc<AtomicBool>,
}

thread_local! {
    static OWNER: RefCell<Option<Arc<Owner>>> = const { RefCell::new(None) };
}

pub(crate) struct Scope {
    previous: Option<Arc<Owner>>,
    _thread: PhantomData<*const ()>,
}
impl Scope {
    pub(crate) fn install(root: Arc<JournalRoot>) -> io::Result<Self> {
        let owner = Arc::new(Owner {
            root,
            namespace: None,
            refused: Arc::new(AtomicBool::new(false)),
        });
        owner.check()?;
        Ok(Self {
            previous: OWNER.with(|slot| slot.replace(Some(owner))),
            _thread: PhantomData,
        })
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        OWNER.with(|slot| {
            let outgoing = slot.replace(previous);
            if let Some(owner) = outgoing {
                // Namespace calls can discover substitution after the owner's
                // preflight. Carry that refusal into the enclosing frame even
                // when the operation returned early with its own error.
                let _ = owner.check();
            }
        });
    }
}

impl Owner {
    fn check(&self) -> io::Result<()> {
        if self.refused.load(Ordering::Acquire) {
            return Err(io::Error::other("location mutation authority was lost"));
        }
        let result = self
            .root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)
            .and_then(|_| {
                self.namespace
                    .as_ref()
                    .map_or(Ok(()), JournalNamespace::revalidate)
            });
        if result.is_err() {
            self.refused.store(true, Ordering::Release);
        }
        result
    }

    fn parent(self: &Arc<Self>, path: &Path, create: bool) -> io::Result<Option<Parent>> {
        if self.namespace.is_none() {
            let nested = Arc::new(Owner {
                root: Arc::clone(&self.root),
                namespace: Some(JournalNamespace::new(Arc::clone(&self.root))),
                refused: Arc::clone(&self.refused),
            });
            return nested.parent(path, create);
        }
        self.check()?;
        let relative = path
            .parent()
            .and_then(|parent| parent.strip_prefix(self.root.canonical_path()).ok())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "location path is outside its journal",
                )
            })?
            .to_path_buf();
        let directory = self
            .namespace
            .as_ref()
            .expect("compound namespace installed")
            .directory(&relative, create)
            .inspect_err(|_| self.refused.store(true, Ordering::Release))?;
        Ok(directory.map(|directory| Parent {
            owner: Arc::clone(self),
            directory,
        }))
    }
}

struct Parent {
    owner: Arc<Owner>,
    directory: Arc<JournalDirectory>,
}
impl Parent {
    fn check(&self) -> io::Result<()> {
        self.owner.check()
    }

    fn open(&self, path: &Path) -> io::Result<Option<File>> {
        self.check()?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("location file has no name"))?;
        #[cfg(unix)]
        let opened = solstone_core_journal_io::flat_directory::open_regular_file_from_bound_parent(
            &self.directory,
            name,
        );
        #[cfg(windows)]
        let opened =
            solstone_core_journal_io::windows_sync_dir::open_windows_regular_file_from_bound_parent(
                &self.directory,
                name,
                path.parent().expect("bound parent was admitted"),
            );
        let file = opened.map_err(io::Error::other)?;
        self.check()?;
        Ok(file)
    }
}

fn owner() -> Option<Arc<Owner>> {
    OWNER.with(|slot| slot.borrow().clone())
}

/// Correlate the lock, reads and mutations of one native update. Keeping this
/// boundary per snapshot/day/store update bounds retained descriptors on a
/// journal with many days, while no operation switches parents under its lock.
pub(crate) fn transaction<T>(work: impl FnOnce() -> T) -> T {
    let Some(current) = owner() else {
        return work();
    };
    if current.namespace.is_some() {
        return work();
    }
    let nested = Arc::new(Owner {
        root: Arc::clone(&current.root),
        namespace: Some(JournalNamespace::new(Arc::clone(&current.root))),
        refused: Arc::clone(&current.refused),
    });
    let _scope = Scope {
        previous: OWNER.with(|slot| slot.replace(Some(nested))),
        _thread: PhantomData,
    };
    work()
}

pub(crate) fn is_owned() -> bool {
    owner().is_some()
}

pub(crate) fn checkpoint() -> io::Result<()> {
    owner().map_or(Ok(()), |owner| owner.check())
}

pub(crate) fn hold_lock(path: &Path, options: LockOptions, shared: bool) -> io::Result<FileLock> {
    let Some(owner) = owner() else {
        return if shared {
            solstone_core_journal_io::locking::hold_shared_lock(path, options)
        } else {
            solstone_core_journal_io::hold_lock(path, options)
        }
        .map_err(io::Error::other);
    };
    let parent = owner.parent(path, true)?.expect("created bound parent");
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("location lock has no name"))?;
    solstone_core_journal_io::hold_lock_bound_checked(
        &parent.directory,
        name,
        path.parent().expect("bound parent was admitted"),
        options,
        shared,
        &|| parent.check().map_err(|error| error.to_string()),
    )
    .map_err(io::Error::other)
}

pub(crate) fn open_file(path: &Path) -> io::Result<File> {
    let Some(owner) = owner() else {
        return File::open(path);
    };
    let parent = owner
        .parent(path, false)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "location source is absent"))?;
    parent
        .open(path)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "location source is absent"))
}

pub(crate) fn read(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_file(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    checkpoint()?;
    Ok(bytes)
}

pub(crate) fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let Some(owner) = owner() else {
        return solstone_core_journal_io::atomic_replace(
            path,
            bytes,
            solstone_core_journal_io::AtomicWriteOptions::default(),
        )
        .map_err(io::Error::other);
    };
    let parent = owner.parent(path, true)?.expect("created bound parent");
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("location file has no name"))?;
    parent.check()?;
    // Existing file modes remain intact. New owned derived files are private.
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        parent
            .open(path)?
            .map(|file| file.metadata().map(|m| m.permissions().mode() & 0o777))
            .transpose()?
            .unwrap_or(0o600)
    };
    #[cfg(windows)]
    let mode = 0o600;
    #[cfg(unix)]
    match solstone_core_journal_io::atomic_replace_bound_checked(
        &parent.directory,
        name,
        bytes,
        mode,
        &|| parent.check().map_err(|error| error.to_string()),
    )
    .map_err(io::Error::other)?
    {
        solstone_core_journal_io::BoundAtomicOutcome::Published { .. } => {}
        solstone_core_journal_io::BoundAtomicOutcome::PublishedDurabilityUncertain {
            source,
            ..
        } => {
            return Err(io::Error::other(format!(
                "location bytes published; durability uncertain: {source}"
            )));
        }
        solstone_core_journal_io::BoundAtomicOutcome::PublishedObservationUncertain {
            source,
            ..
        } => {
            return Err(io::Error::other(format!(
                "location bytes published; observation uncertain: {source}"
            )));
        }
    }
    #[cfg(windows)]
    {
        let lock = solstone_core_journal_io::acquire_existing_parent_lock_bound_checked(
            &parent.directory,
            std::ffi::OsStr::new(".location-publication.lock"),
            solstone_core_journal_io::DEFAULT_LOCK_TIMEOUT,
            solstone_core_journal_io::DEFAULT_LOCK_POLL_INTERVAL,
            &|| parent.check().map_err(|error| error.to_string()),
        )
        .map_err(io::Error::other)?;
        match solstone_core_journal_io::atomic_replace_detailed_bound_checked(
            &parent.directory,
            &lock,
            name,
            bytes,
            mode,
            &|| parent.check().map_err(|error| error.to_string()),
        )
        .map_err(io::Error::other)?
        {
            solstone_core_journal_io::DetailedAtomicOutcome::Published => {}
            solstone_core_journal_io::DetailedAtomicOutcome::PublishedDurabilityUncertain {
                source,
            } => {
                return Err(io::Error::other(format!(
                    "location bytes published; durability uncertain: {source}"
                )));
            }
            _ => {
                return Err(io::Error::other(
                    "location bytes published; namespace observation uncertain",
                ));
            }
        }
    }
    parent.check()
}

pub(crate) fn remove(path: &Path) -> io::Result<bool> {
    let Some(owner) = owner() else {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        };
    };
    let Some(parent) = owner.parent(path, false)? else {
        return Ok(false);
    };
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("location file has no name"))?;
    parent.check()?;
    #[cfg(unix)]
    let observed = solstone_core_journal_io::flat_directory::read_observed_file_bounded(
        &parent.directory,
        name,
        usize::MAX,
    );
    #[cfg(windows)]
    let observed = solstone_core_journal_io::windows_sync_dir::read_windows_observed_file_bounded(
        &parent.directory,
        name,
        usize::MAX,
    );
    let Some(observed) = observed.map_err(io::Error::other)? else {
        return Ok(false);
    };
    parent.check()?;
    #[cfg(unix)]
    let outcome = {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let claim = solstone_core_journal_io::ClaimName::parse(&format!(
            "!solstone-claim-{:08x}-{:016x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ))
        .map_err(|error| io::Error::other(error.to_string()))?;
        solstone_core_journal_io::claim_remove::claim_and_remove_observed_bounded(
            &parent.directory,
            name,
            &observed,
            &claim,
            usize::MAX,
        )
    };
    #[cfg(windows)]
    let outcome = solstone_core_journal_io::windows_sync_dir::remove_windows_observed_file_bounded(
        &parent.directory,
        name,
        &observed,
        usize::MAX,
    );
    match outcome.map_err(io::Error::other)? {
        solstone_core_journal_io::errors::ClaimRemovalOutcome::Removed
        | solstone_core_journal_io::errors::ClaimRemovalOutcome::RemovedUnflushed => {}
        other => {
            return Err(io::Error::other(format!(
                "location removal requires reconciliation: {other:?}"
            )));
        }
    }
    parent.check()?;
    Ok(true)
}

pub(crate) fn enqueue(journal: &Path, path: &Path) -> io::Result<()> {
    match owner() {
        Some(owner) => {
            owner.check()?;
            if journal != owner.root.canonical_path() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "location index journal changed",
                ));
            }
            let fallback;
            let namespace = match &owner.namespace {
                Some(namespace) => namespace,
                None => {
                    fallback = JournalNamespace::new(Arc::clone(&owner.root));
                    &fallback
                }
            };
            let result = solstone_core_indexer_store::todo::enqueue_path_bound(namespace, path);
            if namespace.revalidate().is_err() {
                owner.refused.store(true, Ordering::Release);
            }
            result?;
            owner.check()
        }
        None => solstone_core_indexer_store::todo::enqueue_path(journal, path),
    }
}

#[cfg(unix)]
#[cfg(all(test, feature = "full-tests"))]
#[path = "reconcile_io_tests.rs"]
mod tests;
