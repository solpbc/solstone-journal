// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixed-directory authority for Cortex's journal-owned namespace.

use std::error::Error;
use std::fmt;
use std::io;

#[cfg(any(unix, windows))]
use std::ffi::OsStr;
#[cfg(all(test, unix))]
use std::path::Path;
#[cfg(all(test, unix))]
use std::process::{Command, ExitStatus};

use crate::errors::FlatDirectoryError;
#[cfg(unix)]
use crate::flat_directory::{FlatDirectory, create_or_open_flat_directory_bound};
use crate::journal_root::JournalRoot;
#[cfg(windows)]
use crate::windows_sync_dir::{WindowsFlatDirectory, create_or_open_windows_flat_directory_bound};

/// Retained authority for the fixed Cortex `health/` and `talents/` directories.
pub struct CortexNamespaceAuthority {
    root: JournalRoot,
    #[cfg(unix)]
    health: FlatDirectory,
    #[cfg(windows)]
    health: WindowsFlatDirectory,
    #[cfg(unix)]
    talents: FlatDirectory,
    #[cfg(windows)]
    talents: WindowsFlatDirectory,
}

impl CortexNamespaceAuthority {
    /// Borrow the admitted journal root retained by this authority.
    pub fn root(&self) -> &JournalRoot {
        &self.root
    }

    /// Borrow the admitted direct `health/` directory.
    #[cfg(unix)]
    pub fn health(&self) -> &FlatDirectory {
        &self.health
    }

    /// Borrow the admitted direct `health/` directory.
    #[cfg(windows)]
    pub fn health(&self) -> &WindowsFlatDirectory {
        &self.health
    }

    /// Borrow the admitted direct `talents/` directory.
    #[cfg(unix)]
    pub fn talents(&self) -> &FlatDirectory {
        &self.talents
    }

    /// Borrow the admitted direct `talents/` directory.
    #[cfg(windows)]
    pub fn talents(&self) -> &WindowsFlatDirectory {
        &self.talents
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CortexNamespaceStage {
    Health,
    Talents,
}

impl CortexNamespaceStage {
    const fn token(self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Talents => "talents",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CortexNamespaceClass {
    Unsafe,
    IdentityChanged,
    Io,
}

impl CortexNamespaceClass {
    const fn token(self) -> &'static str {
        match self {
            Self::Unsafe => "unsafe",
            Self::IdentityChanged => "identity_changed",
            Self::Io => "io",
        }
    }
}

/// Bounded failure while admitting Cortex's fixed journal directories.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct CortexNamespaceError {
    stage: CortexNamespaceStage,
    class: CortexNamespaceClass,
}

impl CortexNamespaceError {
    const fn new(stage: CortexNamespaceStage, class: CortexNamespaceClass) -> Self {
        Self { stage, class }
    }

    fn token(self) -> String {
        format!(
            "cortex_namespace_{}_{}",
            self.stage.token(),
            self.class.token()
        )
    }
}

impl fmt::Display for CortexNamespaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.token())
    }
}

impl fmt::Debug for CortexNamespaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for CortexNamespaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Create or admit the fixed direct `health/` and `talents/` children of `root`.
pub fn create_or_admit_cortex_namespace(
    root: JournalRoot,
) -> Result<CortexNamespaceAuthority, CortexNamespaceError> {
    #[cfg(unix)]
    let health = create_or_open_flat_directory_bound(
        &root,
        OsStr::new("health"),
        0o700,
        root.canonical_path(),
    )
    .map_err(|error| map_flat_directory_error(CortexNamespaceStage::Health, error))?;
    #[cfg(windows)]
    let health = create_or_open_windows_flat_directory_bound(
        &root,
        OsStr::new("health"),
        root.canonical_path(),
    )
    .map_err(|error| map_flat_directory_error(CortexNamespaceStage::Health, error))?;

    #[cfg(test)]
    cortex_namespace_composition_checkpoint();

    #[cfg(unix)]
    let talents = create_or_open_flat_directory_bound(
        &root,
        OsStr::new("talents"),
        0o700,
        root.canonical_path(),
    )
    .map_err(|error| map_flat_directory_error(CortexNamespaceStage::Talents, error))?;
    #[cfg(windows)]
    let talents = create_or_open_windows_flat_directory_bound(
        &root,
        OsStr::new("talents"),
        root.canonical_path(),
    )
    .map_err(|error| map_flat_directory_error(CortexNamespaceStage::Talents, error))?;

    Ok(CortexNamespaceAuthority {
        root,
        health,
        talents,
    })
}

fn map_flat_directory_error(
    stage: CortexNamespaceStage,
    error: FlatDirectoryError,
) -> CortexNamespaceError {
    let class = match error {
        FlatDirectoryError::InvalidRelativePath { .. }
        | FlatDirectoryError::InvalidName { .. }
        | FlatDirectoryError::NotDirectory { .. }
        | FlatDirectoryError::SymlinkRefused { .. }
        | FlatDirectoryError::NotRegular { .. }
        | FlatDirectoryError::SizeLimitExceeded { .. } => CortexNamespaceClass::Unsafe,
        FlatDirectoryError::IdentityChanged { .. }
        | FlatDirectoryError::EnumerationChanged { .. } => CortexNamespaceClass::IdentityChanged,
        FlatDirectoryError::Io { source, .. } => match source.kind() {
            io::ErrorKind::AlreadyExists
            | io::ErrorKind::NotADirectory
            | io::ErrorKind::IsADirectory => CortexNamespaceClass::Unsafe,
            _ => CortexNamespaceClass::Io,
        },
    };
    CortexNamespaceError::new(stage, class)
}

#[cfg(test)]
struct CortexNamespaceTestHook {
    callback: Box<dyn FnOnce()>,
}

#[cfg(test)]
thread_local! {
    static CORTEX_NAMESPACE_TEST_HOOK: std::cell::RefCell<Option<CortexNamespaceTestHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
fn run_with_cortex_namespace_test_hook<T>(
    callback: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    CORTEX_NAMESPACE_TEST_HOOK.with(|hook| {
        assert!(
            hook.borrow().is_none(),
            "Cortex namespace test hook is already active"
        );
        *hook.borrow_mut() = Some(CortexNamespaceTestHook {
            callback: Box::new(callback),
        });
    });
    let result = operation();
    let pending = CORTEX_NAMESPACE_TEST_HOOK.with(|hook| hook.borrow_mut().take());
    (result, pending.is_none())
}

#[cfg(test)]
fn cortex_namespace_composition_checkpoint() {
    let callback =
        CORTEX_NAMESPACE_TEST_HOOK.with(|hook| hook.borrow_mut().take().map(|hook| hook.callback));
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(all(test, unix))]
fn run_cortex_namespace_umask_helper(root: &Path) -> ExitStatus {
    Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "cortex_use::namespace::tests::cortex_namespace_umask_helper",
            "--nocapture",
        ])
        .env("JOURNAL_IO_CORTEX_NAMESPACE_UMASK_ROOT", root)
        .status()
        .expect("run isolated umask helper")
}

#[cfg(test)]
/// Unix filesystem fixtures exercise this authority; Windows fixture execution is deliberately out
/// of scope for this library-only landing.
mod tests {
    #[cfg(unix)]
    use nix::sys::stat::{Mode, umask};
    #[cfg(unix)]
    use std::error::Error;
    #[cfg(unix)]
    use std::ffi::OsString;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::io;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use tempfile::{Builder, TempDir};

    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use crate::journal_root::JournalEntryKind;
    #[cfg(unix)]
    use crate::name_admission::NameAdmissionReason;

    #[cfg(unix)]
    const UMASK_ROOT_ENV: &str = "JOURNAL_IO_CORTEX_NAMESPACE_UMASK_ROOT";

    #[cfg(unix)]
    fn temporary() -> TempDir {
        Builder::new()
            .prefix("solstone-cortex-namespace-")
            .tempdir_in("/var/tmp")
            .expect("temporary journal root")
    }

    #[cfg(unix)]
    fn entry_mode(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .expect("entry metadata")
            .permissions()
            .mode()
            & 0o7777
    }

    #[cfg(unix)]
    fn expect_namespace_error(
        result: Result<CortexNamespaceAuthority, CortexNamespaceError>,
        message: &str,
    ) -> CortexNamespaceError {
        match result {
            Ok(_) => panic!("{message}"),
            Err(error) => error,
        }
    }

    #[cfg(unix)]
    struct UmaskRestore(Mode);

    #[cfg(unix)]
    impl UmaskRestore {
        fn set(mask: u32) -> Self {
            Self(umask(Mode::from_bits_truncate(mask as nix::libc::mode_t)))
        }
    }

    #[cfg(unix)]
    impl Drop for UmaskRestore {
        fn drop(&mut self) {
            umask(self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn admission_creates_and_preserves_fixed_siblings() {
        let temporary = temporary();
        let first = create_or_admit_cortex_namespace(
            JournalRoot::open(temporary.path()).expect("admit journal root"),
        )
        .expect("admit Cortex namespace");
        let health_identity = first.health().identity();
        let talents_identity = first.talents().identity();

        assert!(temporary.path().join("health").is_dir());
        assert!(temporary.path().join("talents").is_dir());
        assert!(!temporary.path().join("health/talents").exists());
        first.root().revalidate().expect("retained root");
        first.health().revalidate().expect("retained health");
        first.talents().revalidate().expect("retained talents");
        drop(first);

        let second = create_or_admit_cortex_namespace(
            JournalRoot::open(temporary.path()).expect("readmit journal root"),
        )
        .expect("readmit Cortex namespace");
        assert_eq!(second.health().identity(), health_identity);
        assert_eq!(second.talents().identity(), talents_identity);
    }

    #[cfg(unix)]
    #[test]
    fn cortex_namespace_umask_helper() {
        let Some(root) = std::env::var_os(UMASK_ROOT_ENV) else {
            return;
        };
        let root = PathBuf::from(root);
        let _restore = UmaskRestore::set(0o077);
        let authority = create_or_admit_cortex_namespace(
            JournalRoot::open(&root).expect("admit child-test root"),
        )
        .expect("admit Cortex namespace with restrictive umask");
        assert_eq!(entry_mode(&root.join("health")), 0o700);
        assert_eq!(entry_mode(&root.join("talents")), 0o700);
        authority.health().revalidate().expect("retained health");
        authority.talents().revalidate().expect("retained talents");
    }

    #[cfg(unix)]
    #[test]
    fn admission_leaves_umask_restricted_children_owner_only() {
        let temporary = temporary();
        let root = temporary.path().join("journal");
        fs::create_dir(&root).expect("create journal root");
        let status = run_cortex_namespace_umask_helper(&root);
        assert!(status.success());
        assert_eq!(entry_mode(&root.join("health")), 0o700);
        assert_eq!(entry_mode(&root.join("talents")), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn mapper_is_exhaustive_and_bounded() {
        let path = PathBuf::from("diagnostic-only");
        let unsafe_errors = vec![
            FlatDirectoryError::InvalidRelativePath {
                path: path.clone(),
                reason: "test",
            },
            FlatDirectoryError::InvalidName {
                name: OsString::from("invalid"),
                reason: NameAdmissionReason::Empty,
            },
            FlatDirectoryError::NotDirectory { path: path.clone() },
            FlatDirectoryError::SymlinkRefused { path: path.clone() },
            FlatDirectoryError::NotRegular { path: path.clone() },
            FlatDirectoryError::SizeLimitExceeded {
                path: path.clone(),
                kind: JournalEntryKind::RegularFile,
                size: 1,
                limit: 0,
            },
            FlatDirectoryError::Io {
                operation: "test",
                path: path.clone(),
                source: io::Error::from(io::ErrorKind::AlreadyExists),
            },
            FlatDirectoryError::Io {
                operation: "test",
                path: path.clone(),
                source: io::Error::from(io::ErrorKind::NotADirectory),
            },
            FlatDirectoryError::Io {
                operation: "test",
                path: path.clone(),
                source: io::Error::from(io::ErrorKind::IsADirectory),
            },
        ];
        for error in unsafe_errors {
            let mapped = map_flat_directory_error(CortexNamespaceStage::Health, error);
            assert_eq!(mapped.to_string(), "cortex_namespace_health_unsafe");
            assert_eq!(format!("{mapped:?}"), "cortex_namespace_health_unsafe");
            assert!(mapped.source().is_none());
        }

        for error in [
            FlatDirectoryError::IdentityChanged { path: path.clone() },
            FlatDirectoryError::EnumerationChanged { path: path.clone() },
        ] {
            assert_eq!(
                map_flat_directory_error(CortexNamespaceStage::Health, error).to_string(),
                "cortex_namespace_health_identity_changed"
            );
        }
        assert_eq!(
            map_flat_directory_error(
                CortexNamespaceStage::Talents,
                FlatDirectoryError::Io {
                    operation: "test",
                    path,
                    source: io::Error::from(io::ErrorKind::PermissionDenied),
                },
            )
            .to_string(),
            "cortex_namespace_talents_io"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_slots_and_permission_failures_are_bounded() {
        let temporary = temporary();
        std::os::unix::fs::symlink("missing", temporary.path().join("health"))
            .expect("create unsafe health symlink");
        let result = create_or_admit_cortex_namespace(
            JournalRoot::open(temporary.path()).expect("admit journal root"),
        );
        let error = expect_namespace_error(result, "symlink health must be refused");
        assert_eq!(error.to_string(), "cortex_namespace_health_unsafe");
        fs::remove_file(temporary.path().join("health")).expect("remove health symlink");

        fs::write(temporary.path().join("talents"), b"not a directory")
            .expect("create unsafe talents file");
        let result = create_or_admit_cortex_namespace(
            JournalRoot::open(temporary.path()).expect("admit journal root"),
        );
        let error = expect_namespace_error(result, "file talents must be refused");
        assert_eq!(error.to_string(), "cortex_namespace_talents_unsafe");
        assert!(temporary.path().join("health").is_dir());

        fs::remove_file(temporary.path().join("talents")).expect("remove talents file");
        fs::remove_dir(temporary.path().join("health")).expect("remove accepted health directory");
        let original_mode = entry_mode(temporary.path());
        let root = JournalRoot::open(temporary.path()).expect("admit journal root");
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o500))
            .expect("remove root write permission");
        let result = create_or_admit_cortex_namespace(root);
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(original_mode))
            .expect("restore root permission");
        let error = expect_namespace_error(result, "inaccessible root must be an I/O refusal");
        assert_eq!(error.to_string(), "cortex_namespace_health_io");
    }

    #[cfg(unix)]
    #[test]
    fn retained_capabilities_survive_root_rename() {
        let temporary = temporary();
        let root_path = temporary.path().join("journal");
        let moved_path = temporary.path().join("journal-moved");
        fs::create_dir(&root_path).expect("create journal root");
        let authority = create_or_admit_cortex_namespace(
            JournalRoot::open(&root_path).expect("admit journal root"),
        )
        .expect("admit Cortex namespace");
        let root_identity = authority.root().identity();
        let health_identity = authority.health().identity();
        let talents_identity = authority.talents().identity();

        fs::rename(&root_path, &moved_path).expect("rename journal root");

        authority.root().revalidate().expect("retained root");
        authority.health().revalidate().expect("retained health");
        authority.talents().revalidate().expect("retained talents");
        assert_eq!(authority.root().identity(), root_identity);
        assert_eq!(authority.health().identity(), health_identity);
        assert_eq!(authority.talents().identity(), talents_identity);
        assert!(moved_path.join("health").is_dir());
        assert!(moved_path.join("talents").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn composition_failure_leaves_only_health_without_an_authority() {
        let temporary = temporary();
        let root_path = temporary.path().to_path_buf();
        let original_mode = entry_mode(&root_path);
        let (result, fired) = run_with_cortex_namespace_test_hook(
            {
                let root_path = root_path.clone();
                move || {
                    fs::set_permissions(&root_path, fs::Permissions::from_mode(0o500))
                        .expect("inject talents admission fault");
                }
            },
            || {
                create_or_admit_cortex_namespace(
                    JournalRoot::open(&root_path).expect("admit journal root"),
                )
            },
        );
        fs::set_permissions(&root_path, fs::Permissions::from_mode(original_mode))
            .expect("restore root permission");

        assert!(fired);
        let error =
            expect_namespace_error(result, "fault must prevent namespace authority admission");
        assert_eq!(error.to_string(), "cortex_namespace_talents_io");
        let entries = fs::read_dir(&root_path)
            .expect("read residual namespace entries")
            .map(|entry| entry.expect("residual entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries, vec![OsString::from("health")]);
    }
}
