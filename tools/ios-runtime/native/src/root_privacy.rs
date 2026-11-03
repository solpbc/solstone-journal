// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_journal_io::JournalRoot;
use std::os::fd::{AsFd, AsRawFd};
use std::path::Path;

pub(super) fn create_root(requested: &Path) -> Result<JournalRoot, String> {
    // Engine derives this fixed layout from its signed private/app-group container.
    // Admit that existing container before creating either direct child.
    let documents_path = requested
        .parent()
        .filter(|path| path.file_name() == Some(std::ffi::OsStr::new("Documents")))
        .ok_or("journal root is not beneath container Documents")?;
    let container_path = documents_path.parent().ok_or("missing journal container")?;
    let name = requested
        .file_name()
        .ok_or("missing journal directory name")?;
    let container =
        JournalRoot::open_no_follow(container_path).map_err(|error| error.to_string())?;
    let documents = solstone_core_journal_io::create_or_open_flat_directory_bound(
        &container,
        std::ffi::OsStr::new("Documents"),
        0o700,
        container.canonical_path(),
    )
    .map_err(|error| error.to_string())?;
    container
        .revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    let directory = solstone_core_journal_io::create_or_open_flat_directory_bound(
        &documents,
        name,
        0o700,
        &container.canonical_path().join("Documents"),
    )
    .map_err(|error| error.to_string())?;
    let root = JournalRoot::open_no_follow(requested).map_err(|error| error.to_string())?;
    if root.identity() != directory.identity() {
        return Err("journal root changed during creation".into());
    }
    container
        .revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    Ok(root)
}

/// Change only the admitted directory, including an existing root. Never follow
/// a mutable path to select the object whose permissions are changed.
pub(super) fn apply_private_mode(root: &JournalRoot) -> Result<(), String> {
    root.revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    // SAFETY: the borrowed descriptor remains owned by root for this call.
    if unsafe { libc::fchmod(root.as_fd().as_raw_fd(), 0o700) } != 0 {
        return Err(format!(
            "journal root privacy mode: {}",
            std::io::Error::last_os_error()
        ));
    }
    root.revalidate_canonical_binding()
        .map_err(|error| error.to_string())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ios-root-privacy-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn new_and_existing_roots_keep_identity_and_contents() {
        let f = Fixture::new();
        let path = f.0.join("Documents/journal");
        let root = create_root(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
        std::fs::write(path.join("existing"), b"retained").unwrap();
        std::fs::set_permissions(
            path.join("existing"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(
            f.0.join("Documents"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let identity = root.identity();
        let reopened = create_root(&path).unwrap();
        apply_private_mode(&reopened).unwrap();
        assert_eq!(reopened.identity(), identity);
        assert_eq!(
            std::fs::metadata(f.0.join("Documents")).unwrap().mode() & 0o777,
            0o755
        );
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
        assert_eq!(std::fs::read(path.join("existing")).unwrap(), b"retained");
        assert_eq!(
            std::fs::metadata(path.join("existing")).unwrap().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn alias_and_rebound_root_do_not_change_other_directory() {
        let f = Fixture::new();
        let path = f.0.join("Documents/journal");
        let other = f.0.join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(f.0.join("Documents")).unwrap();
        symlink(&other, &path).unwrap();
        assert!(create_root(&path).is_err());
        assert_eq!(std::fs::metadata(&other).unwrap().mode() & 0o777, 0o755);
        std::fs::remove_file(&path).unwrap();
        let root = create_root(&path).unwrap();
        std::fs::rename(&path, f.0.join("retained")).unwrap();
        symlink(&other, &path).unwrap();
        assert!(apply_private_mode(&root).is_err());
        assert_eq!(std::fs::metadata(&other).unwrap().mode() & 0o777, 0o755);
    }

    #[test]
    fn aliased_creation_parent_cannot_create_a_missing_leaf_elsewhere() {
        let f = Fixture::new();
        let other = f.0.join("other");
        std::fs::create_dir(&other).unwrap();
        symlink(&other, f.0.join("Documents")).unwrap();
        assert!(create_root(&f.0.join("Documents/missing")).is_err());
        assert!(!other.join("missing").exists());
        let alias = f.0.join("container-alias");
        symlink(&other, &alias).unwrap();
        assert!(create_root(&alias.join("Documents/missing")).is_err());
        assert!(!other.join("Documents").exists());
    }
}
