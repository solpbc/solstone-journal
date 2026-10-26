// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Nested directory acquisition from an already retained journal root.
//! Diagnostic pathnames never supply authority for creation or descent.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::JournalRoot;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One operation's retained namespace. Every admitted ancestor remains pinned,
/// so a later read/write cannot switch to a different parent under an old lock.
pub struct JournalNamespace {
    root: Arc<JournalRoot>,
    directories: Mutex<BTreeMap<PathBuf, Arc<JournalDirectory>>>,
    refused: AtomicBool,
}

impl JournalNamespace {
    /// Capture authority only. Every directory operation renews the binding.
    pub fn new(root: Arc<JournalRoot>) -> Self {
        Self {
            root,
            directories: Mutex::new(BTreeMap::new()),
            refused: AtomicBool::new(false),
        }
    }

    pub fn root(&self) -> &Arc<JournalRoot> {
        &self.root
    }

    pub fn revalidate(&self) -> io::Result<()> {
        let directories = self
            .directories
            .lock()
            .map_err(|_| io::Error::other("journal namespace lock poisoned"))?;
        let result = self.revalidate_locked(&directories);
        if result.is_err() {
            self.refused.store(true, Ordering::Release);
        }
        result
    }

    pub fn directory(
        &self,
        relative: &Path,
        create: bool,
    ) -> io::Result<Option<Arc<JournalDirectory>>> {
        let result = self.directory_with_step(relative, create, &mut |_| {});
        if result.is_err() {
            self.refused.store(true, Ordering::Release);
        }
        result
    }

    fn directory_with_step(
        &self,
        relative: &Path,
        create: bool,
        after_check: &mut dyn FnMut(usize),
    ) -> io::Result<Option<Arc<JournalDirectory>>> {
        let names = admitted_components(relative)?;
        let mut directories = self
            .directories
            .lock()
            .map_err(|_| io::Error::other("journal namespace lock poisoned"))?;
        self.revalidate_locked(&directories)?;
        let mut prefix = PathBuf::new();
        let mut result = None;
        for (ordinal, name) in names.into_iter().enumerate() {
            self.root
                .revalidate_canonical_binding()
                .map_err(io::Error::other)?;
            let next_path = prefix.join(&name);
            if let Some(retained) = directories.get(&next_path) {
                let current = self
                    .child(&directories, &prefix, &name, false)?
                    .ok_or_else(|| io::Error::other("journal ancestor disappeared"))?;
                if current.identity() != retained.identity() {
                    return Err(io::Error::other("journal ancestor changed"));
                }
                result = Some(Arc::clone(retained));
            } else {
                after_check(ordinal);
                let Some(opened) = self.child(&directories, &prefix, &name, create)? else {
                    return Ok(None);
                };
                let opened = Arc::new(opened);
                directories.insert(next_path.clone(), Arc::clone(&opened));
                result = Some(opened);
            }
            prefix = next_path;
        }
        self.revalidate_locked(&directories)?;
        Ok(result)
    }

    fn child(
        &self,
        directories: &BTreeMap<PathBuf, Arc<JournalDirectory>>,
        prefix: &Path,
        name: &OsStr,
        create: bool,
    ) -> io::Result<Option<JournalDirectory>> {
        let diagnostic = self.root.canonical_path().join(prefix);
        match directories.get(prefix) {
            Some(parent) => child_from(parent.as_ref(), name, create, &diagnostic),
            None if prefix.as_os_str().is_empty() => {
                child_from(self.root.as_ref(), name, create, &diagnostic)
            }
            None => Err(io::Error::other("journal ancestor was not retained")),
        }
    }

    fn revalidate_locked(
        &self,
        directories: &BTreeMap<PathBuf, Arc<JournalDirectory>>,
    ) -> io::Result<()> {
        if self.refused.load(Ordering::Acquire) {
            return Err(io::Error::other("journal namespace authority was lost"));
        }
        self.root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        for (relative, retained) in directories {
            let prefix = relative
                .parent()
                .expect("retained relative directory has parent");
            let name = relative
                .file_name()
                .expect("retained relative directory has name");
            let current = self
                .child(directories, prefix, name, false)?
                .ok_or_else(|| io::Error::other("journal ancestor disappeared"))?;
            if current.identity() != retained.identity() {
                return Err(io::Error::other("journal ancestor changed"));
            }
        }
        self.root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)
    }
}

fn admitted_components(relative: &Path) -> io::Result<Vec<OsString>> {
    let mut names = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid journal directory",
            ));
        };
        let name = name.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal directory is not UTF-8",
            )
        })?;
        crate::check_portable_component(name)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        names.push(OsString::from(name));
    }
    if names.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty journal directory",
        ));
    }
    Ok(names)
}

#[cfg(unix)]
fn child_from(
    parent: &impl std::os::fd::AsFd,
    name: &OsStr,
    create: bool,
    diagnostic: &Path,
) -> io::Result<Option<JournalDirectory>> {
    if create {
        create_or_open_flat_directory_bound(parent, name, 0o777, diagnostic)
            .map(Some)
            .map_err(io::Error::other)
    } else {
        open_flat_directory_bound(parent, name, diagnostic).map_err(io::Error::other)
    }
}

#[cfg(unix)]
#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    #[test]
    fn replacement_between_binding_check_and_native_create_receives_no_entry() {
        let temporary = crate::test_support::TempDir::new();
        let path = temporary.path().join("journal");
        let displaced = temporary.path().join("original");
        fs::create_dir(&path).unwrap();
        let root = Arc::new(JournalRoot::open(&path).unwrap());
        let namespace = JournalNamespace::new(root);
        let mut reached = false;
        let result = namespace.directory_with_step(
            Path::new("cache/location/accuracy"),
            true,
            &mut |ordinal| {
                if ordinal == 0 {
                    reached = true;
                    fs::rename(&path, &displaced).unwrap();
                    fs::create_dir(&path).unwrap();
                }
            },
        );
        assert!(reached);
        assert!(result.is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        assert!(
            displaced.join("cache").is_dir(),
            "native create was anchored to original descriptor"
        );
    }

    #[test]
    fn nested_substitution_cannot_rebind_an_operation_and_failure_stays_latched() {
        let temporary = crate::test_support::TempDir::new();
        let root = Arc::new(JournalRoot::open(temporary.path()).unwrap());
        let namespace = JournalNamespace::new(root);
        let relative = Path::new("solstone/apps/map");
        let original = namespace.directory(relative, true).unwrap().unwrap();
        let path = temporary.path().join(relative);
        let displaced = path.with_file_name("map-original");
        fs::rename(&path, &displaced).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(namespace.directory(&relative.join("new"), true).is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        fs::remove_dir(&path).unwrap();
        fs::rename(&displaced, &path).unwrap();
        assert!(
            namespace.revalidate().is_err(),
            "restoring a name does not revive a refused operation"
        );
        drop(original);
    }

    #[test]
    fn bound_sidecars_preserve_legacy_mode_and_actual_shared_exclusion() {
        let temporary = crate::test_support::TempDir::new();
        let root = Arc::new(JournalRoot::open(temporary.path()).unwrap());
        let namespace = JournalNamespace::new(root);
        let relative = Path::new("cache/location/accuracy");
        let parent = namespace.directory(relative, true).unwrap().unwrap();
        let diagnostic = temporary.path().join(relative);
        let protected = diagnostic.join("generation");
        let legacy =
            crate::locking::hold_shared_lock(&protected, crate::LockOptions::default()).unwrap();
        let sidecar = diagnostic.join("generation.lock");
        let before = fs::metadata(&sidecar).unwrap().permissions().mode();
        let check = || namespace.revalidate().map_err(|error| error.to_string());
        let shared = crate::hold_lock_bound_checked(
            parent.as_ref(),
            OsStr::new("generation"),
            &diagnostic,
            crate::LockOptions::default(),
            true,
            &check,
        )
        .unwrap();
        let second = crate::hold_lock_bound_checked(
            parent.as_ref(),
            OsStr::new("generation"),
            &diagnostic,
            crate::LockOptions::default(),
            true,
            &check,
        )
        .unwrap();
        assert_eq!(fs::metadata(&sidecar).unwrap().permissions().mode(), before);
        assert!(
            crate::hold_lock(
                &protected,
                crate::LockOptions {
                    timeout: Duration::ZERO,
                    ..Default::default()
                }
            )
            .is_err()
        );
        drop(second);
        drop(shared);
        drop(legacy);
        crate::hold_lock_bound_checked(
            parent.as_ref(),
            OsStr::new("generation"),
            &diagnostic,
            crate::LockOptions {
                timeout: Duration::ZERO,
                ..Default::default()
            },
            false,
            &check,
        )
        .unwrap();
    }
}

#[cfg(windows)]
fn child_from(
    parent: &impl std::os::windows::io::AsHandle,
    name: &OsStr,
    create: bool,
    diagnostic: &Path,
) -> io::Result<Option<JournalDirectory>> {
    if create {
        create_or_open_windows_flat_directory_bound(parent, name, diagnostic)
            .map(Some)
            .map_err(io::Error::other)
    } else {
        open_windows_flat_directory_bound(parent, name, diagnostic).map_err(io::Error::other)
    }
}

#[cfg(unix)]
pub type JournalDirectory = crate::flat_directory::FlatDirectory;
#[cfg(windows)]
pub type JournalDirectory = crate::windows_sync_dir::WindowsFlatDirectory;

#[cfg(unix)]
use crate::flat_directory::{create_or_open_flat_directory_bound, open_flat_directory_bound};
#[cfg(windows)]
use crate::windows_sync_dir::{
    create_or_open_windows_flat_directory_bound, open_windows_flat_directory_bound,
};
