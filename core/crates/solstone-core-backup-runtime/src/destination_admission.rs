// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::path::Path;
#[cfg(all(unix, not(target_os = "ios")))]
use std::path::PathBuf;

use solstone_core_journal_io::JournalRoot;
use std::sync::Arc;
#[cfg(any(target_os = "linux", target_os = "android"))]
use {
    solstone_core_journal_io::{
        activity::ActivityLease, activity_obligations::observe_obligations,
    },
    std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
};

#[derive(Debug)]
pub enum DestinationAdmissionError {
    Failed,
}

impl fmt::Display for DestinationAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "destination admission failed")
    }
}

impl std::error::Error for DestinationAdmissionError {}

#[cfg(any(test, feature = "test-hooks", feature = "test-support"))]
thread_local! {
    #[cfg(target_os = "android")]
    static FORCE_ADMISSION_FAILURE: Box<std::cell::Cell<bool>> = Box::new(std::cell::Cell::new(false));
    #[cfg(not(target_os = "android"))]
    static FORCE_ADMISSION_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(test, feature = "test-hooks", feature = "test-support"))]
pub fn with_forced_admission_failure<T>(f: impl FnOnce() -> T) -> T {
    FORCE_ADMISSION_FAILURE.with(|cell| {
        struct Reset<'a>(&'a std::cell::Cell<bool>, bool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let previous = cell.replace(true);
        let _reset = Reset(cell, previous);
        f()
    })
}

pub struct AdmittedDestination {
    #[cfg(windows)]
    root: Arc<JournalRoot>,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    root: Arc<JournalRoot>,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    activity: Arc<ActivityLease>,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    child_lock: OwnedFd,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    callback_path: PathBuf,
    #[cfg(all(
        unix,
        not(any(target_os = "linux", target_os = "android", target_os = "ios"))
    ))]
    path: PathBuf,
    #[cfg(all(
        unix,
        not(any(target_os = "linux", target_os = "android", target_os = "ios"))
    ))]
    retained_root: Option<Arc<JournalRoot>>,
    #[cfg(target_os = "ios")]
    target: crate::ios_callable::RestoreTarget,
}

impl AdmittedDestination {
    pub fn admit(path: &Path) -> Result<Self, DestinationAdmissionError> {
        #[cfg(any(test, feature = "test-hooks", feature = "test-support"))]
        {
            if FORCE_ADMISSION_FAILURE.with(|cell| cell.get()) {
                return Err(DestinationAdmissionError::Failed);
            }
        }
        #[cfg(any(windows, target_os = "linux", target_os = "android", target_os = "ios"))]
        {
            Self::admit_bound(Arc::new(
                JournalRoot::open(path).map_err(|_| DestinationAdmissionError::Failed)?,
            ))
        }
        #[cfg(all(
            unix,
            not(any(target_os = "linux", target_os = "android", target_os = "ios"))
        ))]
        {
            Ok(Self {
                path: path.to_path_buf(),
                retained_root: None,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(DestinationAdmissionError::Failed)
        }
    }

    /// Admit the host's original target without reopening its pathname.
    pub fn admit_bound(root: Arc<JournalRoot>) -> Result<Self, DestinationAdmissionError> {
        #[cfg(any(test, feature = "test-hooks", feature = "test-support"))]
        if FORCE_ADMISSION_FAILURE.with(|cell| cell.get()) {
            return Err(DestinationAdmissionError::Failed);
        }
        root.revalidate_canonical_binding()
            .map_err(|_| DestinationAdmissionError::Failed)?;
        #[cfg(windows)]
        {
            Ok(Self { root })
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            root.canonical_path()
                .to_str()
                .ok_or(DestinationAdmissionError::Failed)?;
            let activity = ActivityLease::try_acquire(Arc::clone(&root))
                .map_err(|_| DestinationAdmissionError::Failed)?;
            let child_lock = activity
                .duplicate_descriptor_for_child()
                .map_err(|_| DestinationAdmissionError::Failed)?;
            let callback_path = PathBuf::from(format!(
                "/proc/self/fd/{}/.",
                root.as_ref().as_fd().as_raw_fd()
            ));
            let admitted = Self {
                root,
                activity,
                child_lock,
                callback_path,
            };
            admitted.revalidate_for_target()?;
            Ok(admitted)
        }
        #[cfg(target_os = "ios")]
        {
            // The native activity lease on the journal root lasts until this
            // admission drops, through every later restore step.
            let target = crate::ios_callable::RestoreTarget::admit(root)
                .map_err(|_| DestinationAdmissionError::Failed)?;
            Ok(Self { target })
        }
        #[cfg(all(
            unix,
            not(any(target_os = "linux", target_os = "android", target_os = "ios"))
        ))]
        {
            Ok(Self {
                path: root.canonical_path().to_owned(),
                retained_root: Some(root),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = root;
            Err(DestinationAdmissionError::Failed)
        }
    }

    pub fn revalidate_for_target(&self) -> Result<&Path, DestinationAdmissionError> {
        #[cfg(any(test, feature = "test-hooks", feature = "test-support"))]
        {
            if FORCE_ADMISSION_FAILURE.with(|cell| cell.get()) {
                return Err(DestinationAdmissionError::Failed);
            }
        }
        #[cfg(windows)]
        {
            self.root
                .revalidate_canonical_binding()
                .map_err(|_| DestinationAdmissionError::Failed)?;
            Ok(self.root.canonical_path())
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            self.activity
                .revalidate()
                .map_err(|_| DestinationAdmissionError::Failed)?;
            let observations =
                observe_obligations(&self.root).map_err(|_| DestinationAdmissionError::Failed)?;
            if !observations.entries().is_empty() {
                return Err(DestinationAdmissionError::Failed);
            }
            self.activity
                .revalidate()
                .map_err(|_| DestinationAdmissionError::Failed)?;
            Ok(self.root.canonical_path())
        }
        #[cfg(all(
            unix,
            not(any(target_os = "linux", target_os = "android", target_os = "ios"))
        ))]
        {
            if let Some(root) = &self.retained_root {
                root.revalidate_canonical_binding()
                    .map_err(|_| DestinationAdmissionError::Failed)?;
            }
            Ok(&self.path)
        }
        #[cfg(target_os = "ios")]
        {
            self.target
                .revalidate()
                .map_err(|_| DestinationAdmissionError::Failed)?;
            Ok(self.target.path())
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(DestinationAdmissionError::Failed)
        }
    }

    pub(crate) fn callback_target(&self) -> &Path {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            &self.callback_path
        }
        #[cfg(windows)]
        {
            self.root.canonical_path()
        }
        #[cfg(all(
            unix,
            not(any(target_os = "linux", target_os = "android", target_os = "ios"))
        ))]
        {
            &self.path
        }
        #[cfg(target_os = "ios")]
        {
            self.target.path()
        }
        #[cfg(not(any(unix, windows)))]
        {
            Path::new("")
        }
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn ios_restore_target(&self) -> &crate::ios_callable::RestoreTarget {
        &self.target
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(crate) fn child_lock(&self) -> BorrowedFd<'_> {
        self.child_lock.as_fd()
    }
}

pub fn admit_restore_destination(
    path: &Path,
) -> Result<AdmittedDestination, DestinationAdmissionError> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let absolute = std::path::absolute(path).map_err(|_| DestinationAdmissionError::Failed)?;
        let mut existing = absolute.as_path();
        let mut missing = Vec::new();
        loop {
            match std::fs::symlink_metadata(existing) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(
                        existing
                            .file_name()
                            .ok_or(DestinationAdmissionError::Failed)?
                            .to_owned(),
                    );
                    existing = existing.parent().ok_or(DestinationAdmissionError::Failed)?;
                }
                Err(_) => return Err(DestinationAdmissionError::Failed),
            }
        }
        let mut parent =
            JournalRoot::open(existing).map_err(|_| DestinationAdmissionError::Failed)?;
        for name in missing.into_iter().rev() {
            parent
                .revalidate_canonical_binding()
                .map_err(|_| DestinationAdmissionError::Failed)?;
            let retained = PathBuf::from(format!("/proc/self/fd/{}/.", parent.as_fd().as_raw_fd()))
                .join(&name);
            std::fs::create_dir(&retained).map_err(|_| DestinationAdmissionError::Failed)?;
            parent
                .revalidate_canonical_binding()
                .map_err(|_| DestinationAdmissionError::Failed)?;
            parent = JournalRoot::open(&parent.canonical_path().join(name))
                .map_err(|_| DestinationAdmissionError::Failed)?;
        }
        AdmittedDestination::admit(parent.canonical_path())
    }
    #[cfg(windows)]
    {
        use solstone_core_journal_io::name_admission::check_portable_component;
        use std::path::Component;
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(DestinationAdmissionError::Failed);
        }
        let mut existing = path;
        let mut missing = Vec::new();
        loop {
            match std::fs::symlink_metadata(existing) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let name = existing
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or(DestinationAdmissionError::Failed)?;
                    check_portable_component(name)
                        .map_err(|_| DestinationAdmissionError::Failed)?;
                    missing.push(name.to_owned());
                    existing = existing.parent().ok_or(DestinationAdmissionError::Failed)?;
                }
                Err(_) => return Err(DestinationAdmissionError::Failed),
            }
        }
        // Admit every existing ancestor before the first directory creation. Each
        // new directory is then retained before it becomes a creation parent.
        let mut admitted = AdmittedDestination::admit(existing)?;
        for name in missing.into_iter().rev() {
            let target = admitted.revalidate_for_target()?.join(name);
            std::fs::create_dir(&target).map_err(|_| DestinationAdmissionError::Failed)?;
            admitted.revalidate_for_target()?;
            admitted = AdmittedDestination::admit(&target)?;
        }
        Ok(admitted)
    }
    #[cfg(all(not(windows), not(any(target_os = "linux", target_os = "android"))))]
    AdmittedDestination::admit(path)
}
