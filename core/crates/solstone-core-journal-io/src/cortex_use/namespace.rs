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

    #[cfg(all(test, unix))]
    cortex_namespace_composition_checkpoint()?;

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

#[cfg(all(test, unix))]
struct CortexNamespaceTestHook {
    callback: Box<dyn FnOnce() -> Result<(), CortexNamespaceError>>,
}

#[cfg(all(test, unix))]
thread_local! {
    static CORTEX_NAMESPACE_TEST_HOOK: std::cell::RefCell<Option<CortexNamespaceTestHook>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(all(test, unix))]
fn run_with_cortex_namespace_test_hook<T>(
    callback: impl FnOnce() -> Result<(), CortexNamespaceError> + 'static,
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

#[cfg(all(test, unix))]
fn cortex_namespace_composition_checkpoint() -> Result<(), CortexNamespaceError> {
    let callback =
        CORTEX_NAMESPACE_TEST_HOOK.with(|hook| hook.borrow_mut().take().map(|hook| hook.callback));
    if let Some(callback) = callback {
        callback()?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
fn run_cortex_namespace_umask_helper(root: &Path, mask: &str) -> ExitStatus {
    Command::new(std::env::current_exe().expect("current test executable"))
        .args([
            "--exact",
            "cortex_use::namespace::tests::cortex_namespace_umask_helper",
            "--nocapture",
        ])
        .env("JOURNAL_IO_CORTEX_NAMESPACE_UMASK_ROOT", root)
        .env("JOURNAL_IO_CORTEX_NAMESPACE_UMASK", mask)
        .status()
        .expect("run isolated umask helper")
}

#[cfg(all(test, unix))]
fn create_inert_socket(path: &Path) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    nix::sys::stat::mknod(
        path,
        nix::sys::stat::SFlag::S_IFSOCK,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        0,
    )
    .unwrap();
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    drop(std::os::unix::net::UnixListener::bind(path).unwrap());
}

#[cfg(all(test, unix))]
/// Unix filesystem fixtures exercise this authority; Windows execution is caller-owned.
mod tests {
    use std::error::Error;
    use std::ffi::OsString;
    use std::fs;
    use std::io;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::path::{Path, PathBuf};

    use nix::sys::stat::{Mode, umask};
    use nix::unistd::mkfifo;

    use super::*;
    use crate::journal_root::JournalEntryKind;
    use crate::name_admission::NameAdmissionReason;

    const UMASK_ROOT_ENV: &str = "JOURNAL_IO_CORTEX_NAMESPACE_UMASK_ROOT";
    const UMASK_ENV: &str = "JOURNAL_IO_CORTEX_NAMESPACE_UMASK";

    fn entry_mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    fn entry_identity(path: &Path) -> (u64, u64, fs::FileType) {
        let metadata = fs::symlink_metadata(path).expect("entry metadata");
        (metadata.dev(), metadata.ino(), metadata.file_type())
    }

    fn identities(paths: &[PathBuf]) -> Vec<(u64, u64, fs::FileType)> {
        paths.iter().map(|path| entry_identity(path)).collect()
    }

    fn seed_directory(root: &Path, name: &str, mode: u32) {
        let directory = root.join(name);
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(mode)).unwrap();
        fs::write(directory.join("sentinel"), name).unwrap();
        let socket = directory.join("sentinel.sock");
        create_inert_socket(&socket);
    }

    fn expect_error(
        result: Result<CortexNamespaceAuthority, CortexNamespaceError>,
    ) -> CortexNamespaceError {
        result.err().expect("Cortex namespace admission must fail")
    }

    fn assert_mapped(stage: CortexNamespaceStage, error: FlatDirectoryError, class: &str) {
        let mapped = map_flat_directory_error(stage, error);
        let expected = format!("cortex_namespace_{}_{}", stage.token(), class);
        assert_eq!(mapped.to_string(), expected);
        assert_eq!(format!("{mapped:?}"), expected);
        assert!(mapped.source().is_none());
        assert!(!expected.contains("controlled-secret"));
    }

    struct UmaskRestore(Mode);

    impl UmaskRestore {
        fn set(mask: u32) -> Self {
            Self(umask(Mode::from_bits_truncate(mask as nix::libc::mode_t)))
        }
    }

    impl Drop for UmaskRestore {
        fn drop(&mut self) {
            umask(self.0);
        }
    }

    #[test]
    fn admission_preserves_existing_directories_and_siblings() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let root = temporary.path();
        seed_directory(root, "health", 0o750);
        seed_directory(root, "talents", 0o710);
        fs::write(root.join("root-sentinel"), b"root").unwrap();
        let sentinel_paths = [
            root.join("root-sentinel"),
            root.join("health/sentinel"),
            root.join("health/sentinel.sock"),
            root.join("talents/sentinel"),
            root.join("talents/sentinel.sock"),
        ];
        let sentinel_identities = identities(&sentinel_paths);
        let witness = JournalRoot::open(root).unwrap();
        let health = FlatDirectory::open(&witness, Path::new("health"))
            .unwrap()
            .identity();
        let talents = FlatDirectory::open(&witness, Path::new("talents"))
            .unwrap()
            .identity();

        let authority = create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap()).unwrap();

        assert_eq!(authority.health().identity(), health);
        assert_eq!(authority.talents().identity(), talents);
        assert_eq!(entry_mode(&root.join("health")), 0o750);
        assert_eq!(entry_mode(&root.join("talents")), 0o710);
        assert_eq!(fs::read(root.join("root-sentinel")).unwrap(), b"root");
        assert_eq!(fs::read(root.join("health/sentinel")).unwrap(), b"health");
        assert_eq!(fs::read(root.join("talents/sentinel")).unwrap(), b"talents");
        assert_eq!(identities(&sentinel_paths), sentinel_identities);
    }

    #[test]
    fn cortex_namespace_umask_helper() {
        let Some(root) = std::env::var_os(UMASK_ROOT_ENV) else {
            return;
        };
        let root = PathBuf::from(root);
        let mask = u32::from_str_radix(&std::env::var(UMASK_ENV).unwrap(), 8).unwrap();
        let _restore = UmaskRestore::set(mask);
        create_or_admit_cortex_namespace(JournalRoot::open(&root).unwrap()).unwrap();
        assert_eq!(entry_mode(&root.join("health")), 0o700 & !mask);
        assert_eq!(entry_mode(&root.join("talents")), 0o700 & !mask);
    }

    #[test]
    fn admission_leaves_umask_restricted_children_owner_only() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        for (name, mask, expected) in [("ordinary", "022", 0o700), ("owner", "100", 0o600)] {
            let root = temporary.path().join(name);
            fs::create_dir(&root).unwrap();
            assert!(run_cortex_namespace_umask_helper(&root, mask).success());
            assert_eq!(entry_mode(&root.join("health")), expected);
            assert_eq!(entry_mode(&root.join("talents")), expected);
        }
    }

    #[test]
    fn mapper_is_exhaustive_bounded_and_stage_complete() {
        for stage in [CortexNamespaceStage::Health, CortexNamespaceStage::Talents] {
            let path = PathBuf::from("controlled-secret");
            let io_error = |source| FlatDirectoryError::Io {
                operation: "controlled-secret",
                path: path.clone(),
                source,
            };
            for error in [
                FlatDirectoryError::InvalidRelativePath {
                    path: path.clone(),
                    reason: "secret",
                },
                FlatDirectoryError::InvalidName {
                    name: OsString::from("controlled-secret"),
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
            ] {
                assert_mapped(stage, error, "unsafe");
            }
            for kind in [
                io::ErrorKind::AlreadyExists,
                io::ErrorKind::NotADirectory,
                io::ErrorKind::IsADirectory,
            ] {
                assert_mapped(stage, io_error(kind.into()), "unsafe");
            }
            for error in [
                FlatDirectoryError::IdentityChanged { path: path.clone() },
                FlatDirectoryError::EnumerationChanged { path: path.clone() },
            ] {
                assert_mapped(stage, error, "identity_changed");
            }
            let denied = io::ErrorKind::PermissionDenied.into();
            assert_mapped(stage, io_error(denied), "io");
        }
    }

    #[test]
    fn wrong_kind_fixed_slots_are_bounded_and_unchanged() {
        for (slot, other) in [("health", "talents"), ("talents", "health")] {
            for kind in ["symlink", "file", "fifo"] {
                let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
                let root = temporary.path();
                seed_directory(root, other, 0o750);
                let entry = root.join(slot);
                let sentinel = root.join("root-sentinel");
                fs::write(&sentinel, b"unchanged-root").unwrap();
                match kind {
                    "symlink" => symlink("outside-target", &entry).unwrap(),
                    "file" => fs::write(&entry, b"unchanged").unwrap(),
                    "fifo" => mkfifo(&entry, Mode::S_IRUSR | Mode::S_IWUSR).unwrap(),
                    _ => unreachable!(),
                }
                let before = fs::symlink_metadata(&entry).unwrap();
                let preserved = [
                    sentinel.clone(),
                    root.join(other),
                    root.join(other).join("sentinel"),
                    root.join(other).join("sentinel.sock"),
                ];
                let preserved_before = identities(&preserved);
                let error = expect_error(create_or_admit_cortex_namespace(
                    JournalRoot::open(root).unwrap(),
                ));
                assert_eq!(error.to_string(), format!("cortex_namespace_{slot}_unsafe"));
                let after = fs::symlink_metadata(&entry).unwrap();
                assert_eq!(
                    (before.dev(), before.ino(), before.file_type()),
                    (after.dev(), after.ino(), after.file_type())
                );
                assert_eq!(identities(&preserved), preserved_before);
                assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged-root");
                let other_bytes = fs::read(root.join(other).join("sentinel")).unwrap();
                assert_eq!(other_bytes, other.as_bytes());
                assert_eq!(entry_mode(&root.join(other)), 0o750);
                assert!(!root.join("outside-target").exists());
                if kind == "symlink" {
                    assert_eq!(fs::read_link(&entry).unwrap(), Path::new("outside-target"));
                }
                if kind == "file" {
                    assert_eq!(fs::read(&entry).unwrap(), b"unchanged");
                }
                let mut entries = fs::read_dir(root)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect::<Vec<_>>();
                entries.sort();
                let mut expected = vec![OsString::from("root-sentinel"), slot.into(), other.into()];
                expected.sort();
                assert_eq!(entries, expected);
            }
        }
    }

    #[test]
    fn admission_uses_retained_root_after_path_replacement() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let original = temporary.path().join("journal");
        let moved = temporary.path().join("journal-moved");
        fs::create_dir(&original).unwrap();
        let root = JournalRoot::open(&original).unwrap();
        let root_identity = root.identity();
        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        let marker = original.join("replacement");
        fs::write(&marker, b"untouched").unwrap();

        let authority = create_or_admit_cortex_namespace(root).unwrap();

        assert_eq!(authority.root().identity(), root_identity);
        assert!(moved.join("health").is_dir() && moved.join("talents").is_dir());
        assert_eq!(fs::read(marker).unwrap(), b"untouched");
        assert!(!original.join("health").exists() && !original.join("talents").exists());
    }

    #[test]
    fn authority_retains_original_health_after_path_replacement() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let root = temporary.path().to_path_buf();
        let moved = root.join("health-moved");
        let replacement = root.join("health");
        let marker = replacement.join("replacement");
        let (result, fired) = run_with_cortex_namespace_test_hook(
            {
                let root = root.clone();
                let marker = marker.clone();
                move || {
                    fs::rename(root.join("health"), &moved).unwrap();
                    fs::create_dir(&replacement).unwrap();
                    fs::write(marker, b"untouched").unwrap();
                    Ok(())
                }
            },
            || create_or_admit_cortex_namespace(JournalRoot::open(&root).unwrap()),
        );
        let authority = result.unwrap();
        assert!(fired);
        let moved_health = FlatDirectory::open(authority.root(), Path::new("health-moved"))
            .unwrap()
            .identity();
        assert_eq!(authority.health().identity(), moved_health);
        assert_eq!(fs::read(marker).unwrap(), b"untouched");
        assert!(root.join("talents").is_dir());
    }

    #[test]
    fn composition_failure_leaves_only_health_without_authority() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let root = temporary.path().to_path_buf();
        seed_directory(&root, "health", 0o750);
        let preserved = [
            root.join("health"),
            root.join("health/sentinel"),
            root.join("health/sentinel.sock"),
        ];
        let preserved_before = identities(&preserved);
        let injected =
            CortexNamespaceError::new(CortexNamespaceStage::Talents, CortexNamespaceClass::Io);
        let (result, fired) = run_with_cortex_namespace_test_hook(
            move || Err(injected),
            || create_or_admit_cortex_namespace(JournalRoot::open(&root).unwrap()),
        );
        assert!(fired);
        let error = expect_error(result);
        assert_eq!(error.to_string(), "cortex_namespace_talents_io");
        assert_eq!(identities(&preserved), preserved_before);
        assert_eq!(fs::read(root.join("health/sentinel")).unwrap(), b"health");
        assert_eq!(entry_mode(&root.join("health")), 0o750);
        let entries = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries, vec![OsString::from("health")]);
    }
}
