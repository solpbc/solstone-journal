// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Narrow convergence for health artifacts written by the Python supervisor.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::{AtFlags, Flock, FlockArg, OFlag, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};
use solstone_core_journal_io::{JournalRoot, open_flat_directory_bound};

const NATIVE_MODE: u32 = 0o600;
const LOCK_NAME: &str = "supervisor.lock";
const LOCK_OPEN_FLAGS: OFlag = OFlag::O_RDWR
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_NONBLOCK);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacySupervisorLockOutcome {
    Absent,
    AlreadyCurrent,
    Upgraded,
}

/// Converge only the exact, quiescent lock entry written by the V1 supervisor.
///
/// Both the journal and `health` directory remain descriptor-bound throughout.
/// The legacy entry must be an owner-held regular file with one link and mode
/// 0644. An exclusive nonblocking advisory lock proves that its former
/// supervisor is no longer using it before mode is narrowed to 0600.
pub fn upgrade_legacy_supervisor_lock(
    journal: &Path,
) -> Result<LegacySupervisorLockOutcome, String> {
    upgrade_legacy_supervisor_lock_with(journal, || {}, || {})
}

fn upgrade_legacy_supervisor_lock_with<AfterLock, BeforeUpgrade>(
    journal: &Path,
    after_lock: AfterLock,
    before_upgrade: BeforeUpgrade,
) -> Result<LegacySupervisorLockOutcome, String>
where
    AfterLock: FnOnce(),
    BeforeUpgrade: FnOnce(),
{
    let root = JournalRoot::open(journal)
        .map_err(|error| format!("bind journal for legacy supervisor lock: {error}"))?;
    let Some(health) =
        open_flat_directory_bound(&root, OsStr::new("health"), root.canonical_path())
            .map_err(|error| format!("bind health for legacy supervisor lock: {error}"))?
    else {
        return Ok(LegacySupervisorLockOutcome::Absent);
    };
    let health_identity = fstat(&health)
        .map_err(|error| format!("stat bound health for legacy supervisor lock: {error}"))?;
    validate_health_binding(&root, &health, &health_identity)?;
    let name = OsStr::new(LOCK_NAME);
    let observed = match fstatat(&health, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(status) => status,
        Err(Errno::ENOENT) => return Ok(LegacySupervisorLockOutcome::Absent),
        Err(error) => return Err(format!("inspect legacy supervisor lock: {error}")),
    };
    validate_exact_entry(&observed)?;
    let mode = permission_mode(&observed);
    if mode == native_mode_bits() {
        return Ok(LegacySupervisorLockOutcome::AlreadyCurrent);
    }
    if mode != legacy_mode_bits() {
        return Err(format!(
            "legacy supervisor lock cannot be upgraded safely (mode {mode:o})"
        ));
    }

    let descriptor = openat(&health, name, LOCK_OPEN_FLAGS, Mode::empty())
        .map(File::from)
        .map_err(|error| format!("open legacy supervisor lock: {error}"))?;
    let opened = fstat(&descriptor)
        .map_err(|error| format!("stat opened legacy supervisor lock: {error}"))?;
    validate_unchanged(&observed, &opened, legacy_mode_bits(), "during open")?;
    let locked = match Flock::lock(descriptor, FlockArg::LockExclusiveNonblock) {
        Ok(locked) => locked,
        Err((descriptor, Errno::EACCES | Errno::EAGAIN)) => {
            drop(descriptor);
            return Err("legacy supervisor lock is still active".to_owned());
        }
        Err((descriptor, error)) => {
            drop(descriptor);
            return Err(format!("lock legacy supervisor lock: {error}"));
        }
    };

    after_lock();
    validate_health_binding(&root, &health, &health_identity)?;
    root.revalidate_canonical_binding()
        .map_err(|error| format!("revalidate journal for legacy supervisor lock: {error}"))?;
    let named = fstatat(&health, name, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|error| format!("revalidate named legacy supervisor lock: {error}"))?;
    validate_unchanged(&opened, &named, legacy_mode_bits(), "before upgrade")?;

    before_upgrade();
    locked
        .set_permissions(fs::Permissions::from_mode(NATIVE_MODE))
        .map_err(|error| format!("restrict legacy supervisor lock: {error}"))?;
    let publication = (|| {
        locked
            .sync_all()
            .map_err(|error| format!("sync legacy supervisor lock: {error}"))?;
        let upgraded = fstat(&*locked)
            .map_err(|error| format!("verify opened legacy supervisor lock: {error}"))?;
        validate_unchanged(&opened, &upgraded, native_mode_bits(), "after upgrade")?;
        validate_health_binding(&root, &health, &health_identity)?;
        root.revalidate_canonical_binding().map_err(|error| {
            format!("revalidate journal after legacy supervisor lock upgrade: {error}")
        })?;
        let final_named = fstatat(&health, name, AtFlags::AT_SYMLINK_NOFOLLOW)
            .map_err(|error| format!("revalidate upgraded legacy supervisor lock: {error}"))?;
        validate_unchanged(
            &upgraded,
            &final_named,
            native_mode_bits(),
            "after publication",
        )
    })();
    if let Err(error) = publication {
        rollback_legacy_mode(&locked).map_err(|rollback| {
            format!("{error}; rollback legacy supervisor lock mode: {rollback}")
        })?;
        return Err(error);
    }
    Ok(LegacySupervisorLockOutcome::Upgraded)
}

fn rollback_legacy_mode(locked: &Flock<File>) -> Result<(), String> {
    locked
        .set_permissions(fs::Permissions::from_mode(0o644))
        .map_err(|error| format!("restore permissions: {error}"))?;
    locked
        .sync_all()
        .map_err(|error| format!("sync restored permissions: {error}"))?;
    let restored = fstat(&**locked).map_err(|error| format!("verify restored mode: {error}"))?;
    if permission_mode(&restored) != legacy_mode_bits() {
        return Err("mode did not return to 644".to_owned());
    }
    Ok(())
}

fn validate_health_binding(
    root: &JournalRoot,
    health: &solstone_core_journal_io::FlatDirectory,
    expected: &FileStat,
) -> Result<(), String> {
    health
        .revalidate()
        .map_err(|error| format!("revalidate bound health for legacy supervisor lock: {error}"))?;
    root.revalidate()
        .map_err(|error| format!("revalidate journal for legacy supervisor lock: {error}"))?;
    let named = fstatat(root, OsStr::new("health"), AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|error| format!("revalidate named health for legacy supervisor lock: {error}"))?;
    if SFlag::from_bits_truncate(named.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR
        || expected.st_dev != named.st_dev
        || expected.st_ino != named.st_ino
    {
        return Err("legacy supervisor lock health directory changed".to_owned());
    }
    Ok(())
}

fn validate_exact_entry(status: &FileStat) -> Result<(), String> {
    if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
        return Err("legacy supervisor lock is not a regular file".to_owned());
    }
    let owner = nix::unistd::Uid::effective().as_raw();
    if status.st_uid != owner || status.st_nlink != 1 {
        return Err(format!(
            "legacy supervisor lock cannot be upgraded safely (owner {}, links {})",
            status.st_uid, status.st_nlink
        ));
    }
    Ok(())
}

fn validate_unchanged(
    expected: &FileStat,
    observed: &FileStat,
    mode: nix::libc::mode_t,
    stage: &str,
) -> Result<(), String> {
    validate_exact_entry(observed)?;
    if expected.st_dev != observed.st_dev
        || expected.st_ino != observed.st_ino
        || expected.st_uid != observed.st_uid
        || permission_mode(observed) != mode
    {
        return Err(format!("legacy supervisor lock changed {stage}"));
    }
    Ok(())
}

fn permission_mode(status: &FileStat) -> nix::libc::mode_t {
    status.st_mode & nix::libc::mode_t::from(0o7777u16)
}

fn legacy_mode_bits() -> nix::libc::mode_t {
    nix::libc::mode_t::from(0o644u16)
}

fn native_mode_bits() -> nix::libc::mode_t {
    nix::libc::mode_t::from(0o600u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_system::lifecycle::{SupervisorLifecycle, WriterId, readiness_is_valid};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Self {
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-legacy-health-{name}-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn legacy_lock(root: &TestRoot) -> PathBuf {
        let health = root.path().join("health");
        fs::create_dir(&health).unwrap();
        let lock = health.join(LOCK_NAME);
        fs::write(&lock, b"").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        lock
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn exact_legacy_state_converges_through_native_readiness() {
        let root = TestRoot::new("readiness");
        let lock = legacy_lock(&root);
        fs::write(root.path().join("health/supervisor.pid"), b"4294967295").unwrap();
        fs::write(root.path().join("health/supervisor.start_time"), b"0.0").unwrap();

        assert_eq!(
            upgrade_legacy_supervisor_lock(root.path()).unwrap(),
            LegacySupervisorLockOutcome::Upgraded
        );
        assert_eq!(mode(&lock), NATIVE_MODE);
        let writer = WriterId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let mut lifecycle = SupervisorLifecycle::boot(root.path(), writer).unwrap();
        lifecycle
            .signal_ready(100.0, serde_json::Map::new())
            .unwrap();
        assert!(readiness_is_valid(root.path()));
    }

    #[test]
    fn absent_and_native_entries_are_idempotent() {
        let root = TestRoot::new("idempotent");
        fs::create_dir(root.path().join("health")).unwrap();
        assert_eq!(
            upgrade_legacy_supervisor_lock(root.path()).unwrap(),
            LegacySupervisorLockOutcome::Absent
        );
        let lock = root.path().join("health").join(LOCK_NAME);
        fs::write(&lock, b"").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(NATIVE_MODE)).unwrap();
        assert_eq!(
            upgrade_legacy_supervisor_lock(root.path()).unwrap(),
            LegacySupervisorLockOutcome::AlreadyCurrent
        );
    }

    #[test]
    fn hostile_shapes_and_active_or_replaced_entries_refuse_without_mutation() {
        let root = TestRoot::new("hostile");
        let lock = legacy_lock(&root);

        fs::set_permissions(&lock, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(upgrade_legacy_supervisor_lock(root.path()).is_err());
        assert_eq!(mode(&lock), 0o640);

        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        let second = root.path().join("second-link");
        fs::hard_link(&lock, &second).unwrap();
        assert!(upgrade_legacy_supervisor_lock(root.path()).is_err());
        assert_eq!(mode(&lock), 0o644);
        fs::remove_file(second).unwrap();

        let held = Flock::lock(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock)
                .unwrap(),
            FlockArg::LockExclusiveNonblock,
        )
        .unwrap();
        let error = upgrade_legacy_supervisor_lock(root.path()).unwrap_err();
        assert!(error.contains("still active"));
        assert_eq!(mode(&lock), 0o644);
        drop(held);

        let replacement = lock.clone();
        let result = upgrade_legacy_supervisor_lock_with(
            root.path(),
            move || {
                fs::remove_file(&replacement).unwrap();
                fs::write(&replacement, b"replacement").unwrap();
                fs::set_permissions(&replacement, fs::Permissions::from_mode(0o644)).unwrap();
            },
            || {},
        );
        assert!(result.unwrap_err().contains("changed before upgrade"));
        assert_eq!(mode(&lock), 0o644);

        fs::remove_file(&lock).unwrap();
        let outside = root.path().join("outside.lock");
        fs::write(&outside, b"").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&outside, &lock).unwrap();
        assert!(upgrade_legacy_supervisor_lock(root.path()).is_err());
        assert_eq!(mode(&outside), 0o644);
    }

    #[test]
    fn replaced_health_directory_refuses_before_lock_mutation() {
        let root = TestRoot::new("health-race");
        legacy_lock(&root);
        let health = root.path().join("health");
        let displaced = root.path().join("displaced-health");
        let replacement_lock = health.join(LOCK_NAME);
        let result = upgrade_legacy_supervisor_lock_with(
            root.path(),
            {
                let health = health.clone();
                let displaced = displaced.clone();
                move || {
                    fs::rename(&health, &displaced).unwrap();
                    fs::create_dir(&health).unwrap();
                    fs::write(health.join(LOCK_NAME), b"replacement").unwrap();
                    fs::set_permissions(health.join(LOCK_NAME), fs::Permissions::from_mode(0o644))
                        .unwrap();
                }
            },
            || {},
        );
        assert!(result.unwrap_err().contains("health directory changed"));
        assert_eq!(mode(&displaced.join(LOCK_NAME)), 0o644);
        assert_eq!(mode(&replacement_lock), 0o644);
    }

    #[test]
    fn health_replacement_at_upgrade_boundary_rolls_back_displaced_lock() {
        let root = TestRoot::new("health-upgrade-race");
        legacy_lock(&root);
        let health = root.path().join("health");
        let displaced = root.path().join("displaced-health");
        let replacement_lock = health.join(LOCK_NAME);
        let result = upgrade_legacy_supervisor_lock_with(root.path(), || {}, {
            let health = health.clone();
            let displaced = displaced.clone();
            move || {
                fs::rename(&health, &displaced).unwrap();
                fs::create_dir(&health).unwrap();
                fs::write(health.join(LOCK_NAME), b"replacement").unwrap();
                fs::set_permissions(health.join(LOCK_NAME), fs::Permissions::from_mode(0o644))
                    .unwrap();
            }
        });
        assert!(result.unwrap_err().contains("health directory changed"));
        assert_eq!(mode(&displaced.join(LOCK_NAME)), 0o644);
        assert_eq!(mode(&replacement_lock), 0o644);
    }
}
