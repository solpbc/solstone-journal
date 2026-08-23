// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stable sidecar advisory locks.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsFd;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nix::errno::Errno;
use nix::fcntl::{AT_FDCWD, AtFlags, Flock, FlockArg, OFlag, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};

use crate::errors::{ExistingParentLockError, LockError, LockTimeout};

/// Python-compatible default lock-acquisition timeout.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// Python-compatible maximum lock poll interval.
pub const DEFAULT_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Sidecar lock acquisition options.
#[derive(Debug, Clone, Copy)]
pub struct LockOptions {
    /// Maximum time to wait for the advisory lock.
    pub timeout: Duration,
    /// Upper bound for each randomized retry delay.
    pub poll_interval: Duration,
    /// Sidecar file mode at creation.
    pub mode: Option<u32>,
}

impl Default for LockOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_LOCK_TIMEOUT,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: None,
        }
    }
}

/// An exclusive `flock(2)` guard. Dropping it releases the lock.
#[derive(Debug)]
pub struct FileLock {
    _guard: Flock<File>,
    path: PathBuf,
}

impl FileLock {
    /// The protected path, rather than the sidecar lock path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// An exclusive persistent lock entry beneath an existing parent directory.
///
/// Dropping this guard releases its advisory lock but does not remove the entry.
#[derive(Debug)]
pub struct ExistingParentLock {
    _guard: Flock<File>,
    path: PathBuf,
}

impl ExistingParentLock {
    /// The persistent lock entry this guard holds.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// An exclusive persistent lock entry beneath a caller-supplied parent directory.
///
/// Dropping this guard releases its advisory lock but does not remove the entry.
/// There is no `path()`: the parent is a retained descriptor, not a pathname.
#[derive(Debug)]
pub struct BoundParentLock {
    _guard: Flock<File>,
}

/// Acquire a caller-selected persistent lock entry under an existing parent.
///
/// This refuses to follow a symlink at the final parent component or lock entry
/// and binds subsequent operations to the inspected directory identity. It does
/// not resolve or pin ancestors of `parent`, nor detect a non-cooperating
/// mutation after final verification; cooperating callers never replace or
/// unlink the persistent entry.
pub fn acquire_existing_parent_lock(
    parent: &Path,
    name: &OsStr,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<ExistingParentLock, ExistingParentLockError> {
    let name =
        NormalLockName::parse(name).ok_or_else(|| ExistingParentLockError::InvalidLockPath {
            name: name.to_os_string(),
        })?;
    let path = parent.join(name.as_os_str());
    let inspected_parent = inspect_parent(parent)?;
    run_race_hook(AFTER_PARENT_INSPECTION);
    let parent_fd = open_bound_parent(parent, inspected_parent)?;
    let guard = acquire_lock_in_parent(&parent_fd, &name, &path, timeout, poll_interval)?;
    Ok(ExistingParentLock {
        _guard: guard,
        path,
    })
}

/// Acquire a persistent lock entry beneath an already-bound parent directory.
///
/// The parent is the caller-supplied directory descriptor. This never opens a
/// parent via `AT_FDCWD`. Cooperating callers never replace or unlink the
/// persistent entry. The returned guard has no pathname.
pub fn acquire_existing_parent_lock_bound(
    parent: &impl AsFd,
    name: &OsStr,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<BoundParentLock, ExistingParentLockError> {
    let name =
        NormalLockName::parse(name).ok_or_else(|| ExistingParentLockError::InvalidLockPath {
            name: name.to_os_string(),
        })?;
    let path = PathBuf::from(name.as_os_str());
    let status = fstat(parent)
        .map_err(|source| existing_io("stat bound persistent lock parent", &path, source))?;
    if !is_kind(&status, SFlag::S_IFDIR) {
        return Err(ExistingParentLockError::UnsafeParent {
            parent: path,
            kind: file_kind(&status),
        });
    }
    let guard = acquire_lock_in_parent(parent, &name, &path, timeout, poll_interval)?;
    Ok(BoundParentLock { _guard: guard })
}

fn acquire_lock_in_parent(
    parent_fd: &impl AsFd,
    name: &NormalLockName,
    path: &Path,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<Flock<File>, ExistingParentLockError> {
    let deadline = Instant::now() + timeout;
    loop {
        let existing = inspect_lock_entry(parent_fd, name, path)?;
        let file = match existing {
            Some(_) => match openat(parent_fd, name.as_os_str(), ENTRY_OPEN_FLAGS, Mode::empty()) {
                Ok(fd) => File::from(fd),
                Err(Errno::ENOENT) => {
                    return Err(ExistingParentLockError::NamespaceChanged {
                        path: path.to_path_buf(),
                    });
                }
                Err(source) => {
                    return Err(existing_io("open persistent lock entry", path, source));
                }
            },
            None => match openat(
                parent_fd,
                name.as_os_str(),
                ENTRY_CREATE_FLAGS,
                Mode::from_bits_truncate(nix::libc::mode_t::from(0o600u16)),
            ) {
                Ok(fd) => File::from(fd),
                Err(Errno::EEXIST) => {
                    wait_or_expire(deadline, poll_interval, path, timeout)?;
                    continue;
                }
                Err(source) => {
                    return Err(existing_io("create persistent lock entry", path, source));
                }
            },
        };

        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(guard) => {
                run_race_hook(AFTER_LOCK_FLOCK);
                verify_final_lock_entry(parent_fd, name, &guard, existing, path)?;
                if Instant::now() >= deadline {
                    drop(guard);
                    return Err(timeout_error(path, timeout));
                }
                return Ok(guard);
            }
            Err((file, error)) if is_contention(error) => {
                drop(file);
                wait_or_expire(deadline, poll_interval, path, timeout)?;
            }
            Err((file, source)) => {
                drop(file);
                return Err(existing_io("lock persistent entry", path, source));
            }
        }
    }
}

/// Acquire the stable `path.name + ".lock"` sidecar with bounded jittered polling.
///
/// The kernel releases the advisory lock automatically when this process dies,
/// because the RAII guard owns the locked file descriptor.
pub fn hold_lock(path: impl AsRef<Path>, options: LockOptions) -> Result<FileLock, LockError> {
    let path = path.as_ref();
    let parent = parent_dir(path);
    fs::create_dir_all(parent).map_err(|source| io_error(path, source))?;
    let file_name = path.file_name().ok_or_else(|| {
        io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "lock path has no file name"),
        )
    })?;
    let sidecar = derive_sidecar_path(parent, file_name);
    let mut open_options = OpenOptions::new();
    open_options.write(true).create(true);
    #[cfg(unix)]
    open_options
        .mode(options.mode.unwrap_or(0o666))
        .custom_flags(nix::libc::O_NOFOLLOW);
    let deadline = Instant::now() + options.timeout;
    loop {
        let file = open_options
            .open(&sidecar)
            .map_err(|source| io_error(path, source))?;
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(guard) => {
                return Ok(FileLock {
                    _guard: guard,
                    path: path.to_path_buf(),
                });
            }
            Err((file, Errno::EACCES | Errno::EAGAIN)) => {
                drop(file);
                if Instant::now() >= deadline {
                    return Err(LockError::Timeout(LockTimeout {
                        path: path.to_path_buf(),
                        timeout: options.timeout,
                    }));
                }
                thread::sleep(retry_delay(options.poll_interval));
            }
            Err((file, error)) => {
                drop(file);
                return Err(io_error(path, io::Error::from_raw_os_error(error as i32)));
            }
        }
    }
}

/// Reports whether another process currently holds `path`'s sidecar lock.
///
/// This probe never creates the sidecar; a missing sidecar means no lock is held.
pub fn lock_is_held(path: impl AsRef<Path>) -> Result<bool, LockError> {
    let path = path.as_ref();
    let parent = parent_dir(path);
    let file_name = path.file_name().ok_or_else(|| {
        io_error(
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "lock path has no file name"),
        )
    })?;
    let sidecar = derive_sidecar_path(parent, file_name);
    let mut open_options = OpenOptions::new();
    open_options.read(true).create(false);
    #[cfg(unix)]
    open_options.custom_flags(nix::libc::O_NOFOLLOW);
    let file = match open_options.open(&sidecar) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(io_error(path, source)),
    };

    match Flock::lock(file, FlockArg::LockSharedNonblock) {
        Ok(guard) => {
            drop(guard);
            Ok(false)
        }
        Err((file, Errno::EACCES | Errno::EAGAIN)) => {
            drop(file);
            Ok(true)
        }
        Err((file, source)) => {
            drop(file);
            Err(io_error(path, io::Error::from_raw_os_error(source as i32)))
        }
    }
}

/// Sidecar path: parent joined with file_name's native bytes plus ".lock".
fn derive_sidecar_path(parent: &Path, file_name: &OsStr) -> PathBuf {
    let mut name = file_name.to_os_string();
    name.push(".lock");
    parent.join(name)
}

const PARENT_OPEN_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const ENTRY_OPEN_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const ENTRY_CREATE_FLAGS: OFlag = ENTRY_OPEN_FLAGS.union(OFlag::O_CREAT).union(OFlag::O_EXCL);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LockEntryIdentity {
    device: nix::libc::dev_t,
    inode: nix::libc::ino_t,
}

struct NormalLockName(OsString);

impl NormalLockName {
    fn parse(value: &OsStr) -> Option<Self> {
        let mut components = Path::new(value).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(component)), None) if component == value => {
                Some(Self(component.to_os_string()))
            }
            _ => None,
        }
    }

    fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

fn inspect_parent(parent: &Path) -> Result<LockEntryIdentity, ExistingParentLockError> {
    let status = match fstatat(AT_FDCWD, parent, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(status) => status,
        Err(Errno::ENOENT) => {
            return Err(ExistingParentLockError::MissingParent {
                parent: parent.to_path_buf(),
            });
        }
        Err(source) => return Err(existing_io("stat persistent lock parent", parent, source)),
    };
    if !is_kind(&status, SFlag::S_IFDIR) {
        return Err(ExistingParentLockError::UnsafeParent {
            parent: parent.to_path_buf(),
            kind: file_kind(&status),
        });
    }
    Ok(identity(&status))
}

fn open_bound_parent(
    parent: &Path,
    inspected: LockEntryIdentity,
) -> Result<std::os::fd::OwnedFd, ExistingParentLockError> {
    let fd = match openat(AT_FDCWD, parent, PARENT_OPEN_FLAGS, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ENOENT | Errno::ENOTDIR | Errno::ELOOP) => {
            return Err(ExistingParentLockError::ParentChanged {
                parent: parent.to_path_buf(),
            });
        }
        Err(source) => return Err(existing_io("open persistent lock parent", parent, source)),
    };
    let status = fstat(&fd)
        .map_err(|source| existing_io("stat opened persistent lock parent", parent, source))?;
    if !is_kind(&status, SFlag::S_IFDIR) || identity(&status) != inspected {
        return Err(ExistingParentLockError::ParentChanged {
            parent: parent.to_path_buf(),
        });
    }
    Ok(fd)
}

fn inspect_lock_entry(
    parent: &impl AsFd,
    name: &NormalLockName,
    path: &Path,
) -> Result<Option<LockEntryIdentity>, ExistingParentLockError> {
    let status = match fstatat(parent, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(status) => status,
        Err(Errno::ENOENT) => return Ok(None),
        Err(source) => return Err(existing_io("stat persistent lock entry", path, source)),
    };
    validate_lock_entry(&status, path)?;
    Ok(Some(identity(&status)))
}

fn verify_final_lock_entry(
    parent: &impl AsFd,
    name: &NormalLockName,
    opened: &File,
    expected_existing: Option<LockEntryIdentity>,
    path: &Path,
) -> Result<LockEntryIdentity, ExistingParentLockError> {
    let opened_status = fstat(opened)
        .map_err(|source| existing_io("stat opened persistent lock entry", path, source))?;
    validate_lock_entry(&opened_status, path)?;
    let named_status = match fstatat(parent, name.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(status) => status,
        Err(Errno::ENOENT) => {
            return Err(ExistingParentLockError::NamespaceChanged {
                path: path.to_path_buf(),
            });
        }
        Err(source) => return Err(existing_io("stat persistent lock entry", path, source)),
    };
    validate_lock_entry(&named_status, path)?;
    let opened_identity = identity(&opened_status);
    if opened_identity != identity(&named_status)
        || expected_existing.is_some_and(|expected| expected != opened_identity)
    {
        return Err(ExistingParentLockError::NamespaceChanged {
            path: path.to_path_buf(),
        });
    }
    Ok(opened_identity)
}

fn validate_lock_entry(status: &FileStat, path: &Path) -> Result<(), ExistingParentLockError> {
    if !is_kind(status, SFlag::S_IFREG) {
        return Err(ExistingParentLockError::UnsafeLockEntry {
            path: path.to_path_buf(),
            kind: file_kind(status),
        });
    }
    if permission_mode(status) != mode_to_u32(nix::libc::mode_t::from(0o600u16)) {
        return Err(ExistingParentLockError::WrongMode {
            path: path.to_path_buf(),
            observed: permission_mode(status),
        });
    }
    Ok(())
}

fn identity(status: &FileStat) -> LockEntryIdentity {
    LockEntryIdentity {
        device: status.st_dev,
        inode: status.st_ino,
    }
}

fn is_kind(status: &FileStat, kind: SFlag) -> bool {
    SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == kind
}

fn file_kind(status: &FileStat) -> &'static str {
    match SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT {
        SFlag::S_IFREG => "regular file",
        SFlag::S_IFDIR => "directory",
        SFlag::S_IFLNK => "symlink",
        SFlag::S_IFIFO => "fifo",
        SFlag::S_IFSOCK => "socket",
        _ => "other",
    }
}

fn permission_mode(status: &FileStat) -> u32 {
    // `mode_t` is u32 on Linux and u16 on Apple targets.
    mode_to_u32(status.st_mode & nix::libc::mode_t::from(0o7777u16))
}

#[cfg(target_vendor = "apple")]
fn mode_to_u32(mode: nix::libc::mode_t) -> u32 {
    u32::from(mode)
}

#[cfg(not(target_vendor = "apple"))]
fn mode_to_u32(mode: nix::libc::mode_t) -> u32 {
    mode
}

fn is_contention(error: Errno) -> bool {
    error == Errno::EACCES || error == Errno::EAGAIN || error == Errno::EWOULDBLOCK
}

fn timeout_error(path: &Path, timeout: Duration) -> ExistingParentLockError {
    ExistingParentLockError::Timeout(LockTimeout {
        path: path.to_path_buf(),
        timeout,
    })
}

fn wait_or_expire(
    deadline: Instant,
    poll_interval: Duration,
    path: &Path,
    timeout: Duration,
) -> Result<(), ExistingParentLockError> {
    if Instant::now() >= deadline {
        return Err(timeout_error(path, timeout));
    }
    thread::sleep(retry_delay(poll_interval));
    Ok(())
}

fn existing_io(operation: &'static str, path: &Path, source: Errno) -> ExistingParentLockError {
    ExistingParentLockError::Io {
        operation,
        path: path.to_path_buf(),
        source: io::Error::from_raw_os_error(source as i32),
    }
}

const AFTER_PARENT_INSPECTION: usize = 0;
const AFTER_LOCK_FLOCK: usize = 1;

#[cfg(test)]
type RaceHooks = [Option<fn()>; 2];

#[cfg(test)]
thread_local! {
    static RACE_HOOKS: std::cell::Cell<RaceHooks> =
        const { std::cell::Cell::new([None; 2]) };
}

#[cfg(test)]
fn run_race_hook(point: usize) {
    if let Some(callback) = RACE_HOOKS.with(|hooks| hooks.get()[point]) {
        callback();
    }
}

#[cfg(not(test))]
fn run_race_hook(_point: usize) {}

#[cfg(test)]
struct RaceHook(usize);

#[cfg(test)]
impl RaceHook {
    fn install(point: usize, callback: fn()) -> Self {
        RACE_HOOKS.with(|hooks| {
            let mut values = hooks.get();
            assert!(values[point].is_none());
            values[point] = Some(callback);
            hooks.set(values);
        });
        Self(point)
    }
}

#[cfg(test)]
impl Drop for RaceHook {
    fn drop(&mut self) {
        RACE_HOOKS.with(|hooks| {
            let mut values = hooks.get();
            values[self.0] = None;
            hooks.set(values);
        });
    }
}

fn retry_delay(poll_interval: Duration) -> Duration {
    let sleep_max = if poll_interval.is_zero() {
        DEFAULT_LOCK_POLL_INTERVAL
    } else {
        poll_interval.min(DEFAULT_LOCK_POLL_INTERVAL)
    };
    let sleep_min = Duration::from_millis(10).min(sleep_max);
    if sleep_min >= sleep_max {
        return sleep_max;
    }
    let span = sleep_max - sleep_min;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let entropy = now ^ u128::from(std::process::id());
    let offset = entropy % (span.as_nanos() + 1);
    sleep_min + Duration::from_nanos(offset as u64)
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn io_error(path: &Path, source: io::Error) -> LockError {
    LockError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::fs;
    use std::io;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::Duration;

    use super::*;
    use crate::test_support::TempDir;

    thread_local! {
        static PARENT_SWAP: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
        static ENTRY_REPLACEMENT: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
    }

    #[derive(Debug, PartialEq, Eq)]
    struct EntrySnapshot {
        kind: &'static str,
        mode: u32,
        identity: LockEntryIdentity,
        bytes: Vec<u8>,
    }

    fn snapshot(path: &Path) -> EntrySnapshot {
        let status = fs::symlink_metadata(path).unwrap();
        let kind = if status.file_type().is_symlink() {
            "symlink"
        } else if status.is_dir() {
            "directory"
        } else {
            "regular file"
        };
        EntrySnapshot {
            kind,
            mode: status.permissions().mode() & 0o7777,
            identity: LockEntryIdentity {
                device: std::os::unix::fs::MetadataExt::dev(&status) as nix::libc::dev_t,
                inode: std::os::unix::fs::MetadataExt::ino(&status) as nix::libc::ino_t,
            },
            bytes: if status.is_file() {
                fs::read(path).unwrap()
            } else {
                Vec::new()
            },
        }
    }

    fn acquire(
        parent: &Path,
        name: &OsStr,
        timeout: Duration,
    ) -> Result<ExistingParentLock, ExistingParentLockError> {
        acquire_existing_parent_lock(parent, name, timeout, Duration::from_millis(10))
    }

    fn swap_parent_hook() {
        PARENT_SWAP.with(|state| {
            let (parent, replacement) = state.borrow_mut().take().unwrap();
            fs::rename(&parent, parent.with_file_name("old-parent")).unwrap();
            fs::rename(replacement, parent).unwrap();
        });
    }

    fn replace_entry_hook() {
        ENTRY_REPLACEMENT.with(|state| {
            let (entry, replacement) = state.borrow_mut().take().unwrap();
            fs::remove_file(&entry).unwrap();
            fs::rename(replacement, entry).unwrap();
        });
    }

    #[test]
    fn lock_sidecar_symlink_is_refused_without_opening_its_target() {
        let temporary = TempDir::new();
        let protected = temporary.path().join("health-dedupe.sqlite");
        let outside = temporary.path().join("outside");
        fs::write(&outside, b"outside-owner-sentinel").unwrap();
        symlink(&outside, temporary.path().join("health-dedupe.sqlite.lock")).unwrap();

        assert!(hold_lock(&protected, LockOptions::default()).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"outside-owner-sentinel");
    }

    #[test]
    fn timeout_reports_the_protected_path_and_timeout() {
        let temporary = TempDir::new();
        let path = temporary.path().join("config.json");
        let _first = hold_lock(&path, LockOptions::default()).unwrap();
        let options = LockOptions {
            timeout: Duration::from_millis(50),
            ..LockOptions::default()
        };

        let error = hold_lock(&path, options).unwrap_err();
        match error {
            LockError::Timeout(timeout) => {
                assert_eq!(timeout.path, path);
                assert_eq!(timeout.timeout, Duration::from_millis(50));
            }
            LockError::Io { .. } => panic!("expected timeout"),
        }
    }

    #[test]
    fn existing_parent_lock_persists_at_0600_with_a_stable_inode() {
        let temporary = TempDir::new();
        let parent = temporary.path().join("locks");
        fs::create_dir(&parent).unwrap();
        let first = acquire(&parent, OsStr::new("state.lock"), Duration::from_secs(1)).unwrap();
        assert_eq!(first.path(), parent.join("state.lock"));
        assert!(!parent.join("state.lock.lock").exists());
        let entry = parent.join("state.lock");
        let identity = snapshot(&entry).identity;
        drop(first);
        assert_eq!(snapshot(&entry).mode, 0o600);
        let second = acquire(&parent, OsStr::new("state.lock"), Duration::from_secs(1)).unwrap();
        assert_eq!(snapshot(&entry).identity, identity);
        drop(second);
        assert_eq!(snapshot(&entry).identity, identity);
    }

    #[test]
    fn existing_parent_lock_refusals_do_not_mutate_entries() {
        let temporary = TempDir::new();
        let parent = temporary.path().join("parent");
        fs::create_dir(&parent).unwrap();
        for name in [
            OsStr::new(""),
            OsStr::new("/"),
            OsStr::new("."),
            OsStr::new(".."),
        ] {
            assert!(matches!(
                acquire(&parent, name, Duration::ZERO),
                Err(ExistingParentLockError::InvalidLockPath { .. })
            ));
            assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        }
        assert!(matches!(
            acquire(&parent.join("missing"), OsStr::new("lock"), Duration::ZERO),
            Err(ExistingParentLockError::MissingParent { .. })
        ));
        let ordinary_parent = temporary.path().join("ordinary-parent");
        fs::write(&ordinary_parent, b"keep").unwrap();
        let before = snapshot(&ordinary_parent);
        assert!(matches!(
            acquire(&ordinary_parent, OsStr::new("lock"), Duration::ZERO),
            Err(ExistingParentLockError::UnsafeParent {
                kind: "regular file",
                ..
            })
        ));
        assert_eq!(snapshot(&ordinary_parent), before);
        let linked_parent = temporary.path().join("linked-parent");
        symlink(&parent, &linked_parent).unwrap();
        assert!(matches!(
            acquire(&linked_parent, OsStr::new("lock"), Duration::ZERO),
            Err(ExistingParentLockError::UnsafeParent {
                kind: "symlink",
                ..
            })
        ));
        for (name, directory) in [("directory", true), ("link", false)] {
            let entry = parent.join(name);
            if directory {
                fs::create_dir(&entry).unwrap();
            } else {
                symlink(&ordinary_parent, &entry).unwrap();
            }
            let before = snapshot(&entry);
            assert!(matches!(
                acquire(&parent, OsStr::new(name), Duration::ZERO),
                Err(ExistingParentLockError::UnsafeLockEntry { .. })
            ));
            assert_eq!(snapshot(&entry), before);
        }
        let wrong_mode = parent.join("wrong-mode");
        fs::write(&wrong_mode, b"keep").unwrap();
        fs::set_permissions(&wrong_mode, fs::Permissions::from_mode(0o644)).unwrap();
        let before = snapshot(&wrong_mode);
        assert!(matches!(
            acquire(&parent, OsStr::new("wrong-mode"), Duration::ZERO),
            Err(ExistingParentLockError::WrongMode {
                observed: 0o644,
                ..
            })
        ));
        assert_eq!(snapshot(&wrong_mode), before);
        let nul = OsString::from_vec(b"nul\0lock".to_vec());
        // nix translates an interior NUL in an otherwise normal component to EINVAL.
        assert!(matches!(
            acquire(&parent, &nul, Duration::ZERO),
            Err(ExistingParentLockError::Io { .. })
        ));
    }

    #[test]
    fn existing_parent_lock_detects_parent_and_entry_replacement() {
        let temporary = TempDir::new();
        let root = temporary.path().join("root");
        let parent = root.join("parent");
        let replacement = root.join("replacement");
        fs::create_dir_all(&parent).unwrap();
        fs::create_dir(&replacement).unwrap();
        fs::write(parent.join("original"), b"original").unwrap();
        fs::write(replacement.join("replacement"), b"replacement").unwrap();
        PARENT_SWAP.with(|state| state.replace(Some((parent.clone(), replacement))));
        let _hook = RaceHook::install(AFTER_PARENT_INSPECTION, swap_parent_hook);
        assert!(matches!(
            acquire(&parent, OsStr::new("lock"), Duration::ZERO),
            Err(ExistingParentLockError::ParentChanged { .. })
        ));
        assert_eq!(
            fs::read(root.join("old-parent/original")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(parent.join("replacement")).unwrap(),
            b"replacement"
        );
        drop(_hook);
        let entry = parent.join("lock");
        let staged = parent.join("staged");
        fs::write(&entry, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
        let expected = snapshot(&staged);
        ENTRY_REPLACEMENT.with(|state| state.replace(Some((entry.clone(), staged))));
        let _hook = RaceHook::install(AFTER_LOCK_FLOCK, replace_entry_hook);
        assert!(matches!(
            acquire(&parent, OsStr::new("lock"), Duration::from_secs(1)),
            Err(ExistingParentLockError::NamespaceChanged { .. })
        ));
        assert_eq!(snapshot(&entry), expected);
    }

    #[test]
    fn existing_parent_lock_namespace_race_beats_coincident_timeout() {
        let temporary = TempDir::new();
        let parent = temporary.path().join("locks");
        fs::create_dir(&parent).unwrap();
        let entry = parent.join("lock");
        let staged = parent.join("staged");
        fs::write(&entry, b"old").unwrap();
        fs::write(&staged, b"new").unwrap();
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
        ENTRY_REPLACEMENT.with(|state| state.replace(Some((entry, staged))));
        let _hook = RaceHook::install(AFTER_LOCK_FLOCK, replace_entry_hook);
        let error = acquire(&parent, OsStr::new("lock"), Duration::ZERO).unwrap_err();
        assert!(matches!(
            &error,
            ExistingParentLockError::NamespaceChanged { .. }
        ));
        assert!(!matches!(&error, ExistingParentLockError::Timeout(_)));
    }

    #[test]
    fn existing_parent_lock_matching_identity_yields_timeout_at_expiry() {
        let temporary = TempDir::new();
        let parent = temporary.path().join("locks");
        fs::create_dir(&parent).unwrap();
        let entry = parent.join("lock");
        fs::write(&entry, b"lock").unwrap();
        fs::set_permissions(&entry, fs::Permissions::from_mode(0o600)).unwrap();
        let error = acquire(&parent, OsStr::new("lock"), Duration::ZERO).unwrap_err();
        assert!(matches!(
            error,
            ExistingParentLockError::Timeout(LockTimeout { timeout, .. })
                if timeout == Duration::ZERO
        ));
    }

    const FF: &[u8] = b"seg-\xff";
    const FE: &[u8] = b"seg-\xfe";
    const TWIN: &[u8] = b"seg-\xef\xbf\xbd";
    const FF_LOCK: &[u8] = b"seg-\xff.lock";
    const FE_LOCK: &[u8] = b"seg-\xfe.lock";
    const TWIN_LOCK: &[u8] = b"seg-\xef\xbf\xbd.lock";

    fn os_path(dir: &Path, bytes: &[u8]) -> PathBuf {
        dir.join(OsString::from_vec(bytes.to_vec()))
    }

    fn dir_entries(dir: &Path) -> BTreeSet<OsString> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    }

    fn is_name_too_long(error: &io::Error) -> bool {
        error.raw_os_error() == Some(Errno::ENAMETOOLONG as i32)
    }

    fn try_create_len(dir: &Path, len: usize) -> io::Result<()> {
        let path = dir.join(OsString::from_vec(vec![b'x'; len]));
        fs::File::create(path).map(drop)
    }

    fn filesystem_name_max(dir: &Path) -> usize {
        fs::create_dir_all(dir).unwrap();
        let mut lo = 1usize;
        let mut hi = 2usize;
        loop {
            match try_create_len(dir, hi) {
                Ok(()) => {
                    lo = hi;
                    hi = hi.saturating_mul(2);
                    assert!(hi <= 8192, "NAME_MAX probe exceeded 8192");
                }
                Err(error) if is_name_too_long(&error) => break,
                Err(error) => panic!("unexpected NAME_MAX probe error at {hi}: {error}"),
            }
        }
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            match try_create_len(dir, mid) {
                Ok(()) => lo = mid,
                Err(error) if is_name_too_long(&error) => hi = mid,
                Err(error) => panic!("unexpected NAME_MAX probe error at {mid}: {error}"),
            }
        }
        lo
    }

    fn short_lock_options() -> LockOptions {
        LockOptions {
            timeout: Duration::from_millis(50),
            ..LockOptions::default()
        }
    }

    fn layout() -> (TempDir, PathBuf, PathBuf) {
        let temporary = TempDir::new();
        let probe = temporary.path().join("probe");
        let locks = temporary.path().join("locks");
        fs::create_dir(&locks).unwrap();
        (temporary, probe, locks)
    }

    #[test]
    fn utf8_name_takes_a_single_exclusive_sidecar() {
        let (_temporary, _probe, locks) = layout();
        let path = locks.join("health.sqlite");
        let first = hold_lock(&path, LockOptions::default()).unwrap();
        assert_eq!(
            dir_entries(&locks),
            BTreeSet::from([OsString::from("health.sqlite.lock")])
        );
        assert!(lock_is_held(&path).unwrap());
        match hold_lock(&path, short_lock_options()) {
            Err(LockError::Timeout(_)) => {}
            other => panic!("expected timeout, got {other:?}"),
        }
        drop(first);
        assert!(!lock_is_held(&path).unwrap());
        assert_eq!(
            dir_entries(&locks),
            BTreeSet::from([OsString::from("health.sqlite.lock")])
        );
    }

    #[test]
    fn invalid_name_and_its_lossy_twin_lock_independently() {
        let (_temporary, _probe, locks) = layout();
        let ff = os_path(&locks, FF);
        let twin = os_path(&locks, TWIN);
        let ff_lock = hold_lock(&ff, LockOptions::default()).unwrap();
        let twin_lock = hold_lock(&twin, LockOptions::default()).unwrap();
        assert!(lock_is_held(&ff).unwrap());
        assert!(lock_is_held(&twin).unwrap());
        assert_eq!(
            dir_entries(&locks),
            BTreeSet::from([
                OsString::from_vec(FF_LOCK.to_vec()),
                OsString::from_vec(TWIN_LOCK.to_vec()),
            ])
        );
        drop(ff_lock);
        drop(twin_lock);
        assert!(!lock_is_held(&ff).unwrap());
        assert!(!lock_is_held(&twin).unwrap());
    }

    #[test]
    fn invalid_utf8_collisions_are_independent_and_same_name_contends() {
        let (_temporary, _probe, locks) = layout();
        let ff = os_path(&locks, FF);
        let fe = os_path(&locks, FE);
        let ff_lock = hold_lock(&ff, LockOptions::default()).unwrap();
        let fe_lock = hold_lock(&fe, LockOptions::default()).unwrap();
        let entries = dir_entries(&locks);
        assert_eq!(
            entries,
            BTreeSet::from([
                OsString::from_vec(FF_LOCK.to_vec()),
                OsString::from_vec(FE_LOCK.to_vec()),
            ])
        );
        assert!(!entries.contains(&OsString::from_vec(TWIN_LOCK.to_vec())));
        assert!(lock_is_held(&ff).unwrap());
        assert!(lock_is_held(&fe).unwrap());
        match hold_lock(&ff, short_lock_options()) {
            Err(LockError::Timeout(_)) => {}
            other => panic!("expected same-name timeout, got {other:?}"),
        }
        drop(ff_lock);
        drop(fe_lock);
        assert!(!lock_is_held(&ff).unwrap());
        assert!(!lock_is_held(&fe).unwrap());
    }

    #[test]
    fn sidecar_at_name_max_holds_and_one_byte_over_is_io() {
        let (_temporary, probe, locks) = layout();
        let name_max = filesystem_name_max(&probe);
        let fit_name = "a".repeat(name_max.saturating_sub(5));
        let fit_path = locks.join(&fit_name);
        let fit_sidecar = derive_sidecar_path(parent_dir(&fit_path), fit_path.file_name().unwrap());
        assert_eq!(
            fit_sidecar.file_name().unwrap().as_encoded_bytes().len(),
            name_max
        );
        let held = hold_lock(&fit_path, LockOptions::default()).unwrap();
        assert!(lock_is_held(&fit_path).unwrap());
        drop(held);
        assert!(!lock_is_held(&fit_path).unwrap());
        let after_fit = dir_entries(&locks);

        let over_path = locks.join("b".repeat(name_max.saturating_sub(4)));
        match hold_lock(&over_path, LockOptions::default()) {
            Err(LockError::Io { path, source }) => {
                assert_eq!(path, over_path);
                assert_eq!(source.raw_os_error(), Some(Errno::ENAMETOOLONG as i32));
            }
            other => panic!("expected Io ENAMETOOLONG, got {other:?}"),
        }
        match lock_is_held(&over_path) {
            Err(LockError::Io { path, source }) => {
                assert_eq!(path, over_path);
                assert_eq!(source.raw_os_error(), Some(Errno::ENAMETOOLONG as i32));
            }
            other => panic!("expected Io ENAMETOOLONG, got {other:?}"),
        }
        assert_eq!(dir_entries(&locks), after_fit);
    }

    #[test]
    fn probe_does_not_create_sidecar_and_inode_survives_reacquire() {
        let (_temporary, _probe, locks) = layout();
        let path = locks.join("state.json");
        assert!(!lock_is_held(&path).unwrap());
        assert!(dir_entries(&locks).is_empty());
        let first = hold_lock(&path, LockOptions::default()).unwrap();
        let sidecar = locks.join("state.json.lock");
        let held = fs::File::open(&sidecar).unwrap();
        drop(first);
        let _second = hold_lock(&path, LockOptions::default()).unwrap();
        // The held fd pins the original inode, so a recycled number cannot match.
        let held_meta = held.metadata().unwrap();
        assert_eq!(
            LockEntryIdentity {
                device: std::os::unix::fs::MetadataExt::dev(&held_meta) as nix::libc::dev_t,
                inode: std::os::unix::fs::MetadataExt::ino(&held_meta) as nix::libc::ino_t,
            },
            snapshot(&sidecar).identity
        );
    }

    #[test]
    fn acquire_existing_parent_lock_bound_holds_without_a_path() {
        let temporary = TempDir::new();
        let parent = temporary.path().join("locks");
        fs::create_dir(&parent).unwrap();
        let parent_fd = nix::fcntl::openat(
            nix::fcntl::AT_FDCWD,
            &parent,
            super::PARENT_OPEN_FLAGS,
            nix::sys::stat::Mode::empty(),
        )
        .unwrap();
        let first = acquire_existing_parent_lock_bound(
            &parent_fd,
            OsStr::new("state.lock"),
            Duration::from_secs(1),
            Duration::from_millis(10),
        )
        .unwrap();
        let entry = parent.join("state.lock");
        assert!(entry.exists());
        assert_eq!(snapshot(&entry).mode, 0o600);
        let contended = acquire_existing_parent_lock_bound(
            &parent_fd,
            OsStr::new("state.lock"),
            Duration::from_millis(50),
            Duration::from_millis(10),
        );
        assert!(matches!(
            contended,
            Err(ExistingParentLockError::Timeout(_))
        ));
        drop(first);
        let _second = acquire_existing_parent_lock_bound(
            &parent_fd,
            OsStr::new("state.lock"),
            Duration::from_secs(1),
            Duration::from_millis(10),
        )
        .unwrap();
        assert_eq!(snapshot(&entry).mode, 0o600);
    }
}
