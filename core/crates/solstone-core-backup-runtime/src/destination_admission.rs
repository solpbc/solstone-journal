// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(windows)]
use solstone_core_journal_io::JournalRoot;

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
    root: JournalRoot,
    #[cfg(unix)]
    path: PathBuf,
}

impl AdmittedDestination {
    pub fn admit(path: &Path) -> Result<Self, DestinationAdmissionError> {
        #[cfg(any(test, feature = "test-hooks", feature = "test-support"))]
        {
            if FORCE_ADMISSION_FAILURE.with(|cell| cell.get()) {
                return Err(DestinationAdmissionError::Failed);
            }
        }
        #[cfg(windows)]
        {
            let root = JournalRoot::open(path).map_err(|_| DestinationAdmissionError::Failed)?;
            root.revalidate_canonical_binding()
                .map_err(|_| DestinationAdmissionError::Failed)?;
            Ok(Self { root })
        }
        #[cfg(unix)]
        {
            Ok(Self {
                path: path.to_path_buf(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
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
        #[cfg(unix)]
        {
            Ok(&self.path)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(DestinationAdmissionError::Failed)
        }
    }
}

pub fn admit_restore_destination(
    path: &Path,
) -> Result<AdmittedDestination, DestinationAdmissionError> {
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
    #[cfg(not(windows))]
    AdmittedDestination::admit(path)
}
