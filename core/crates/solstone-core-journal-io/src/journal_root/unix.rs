// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, open, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};

use super::backend::Backend;
use super::{JournalEntryKind, JournalRootError, ObjectIdentity};

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const REQUESTED_ROOT_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquisitionPrimitive {
    RequestedRootOpen,
    AuthoritativeFstat,
    Canonicalize,
    FilesystemRootOpen,
    FilesystemRootFstat,
    ComponentStat,
    ComponentOpen,
    ComponentFstat,
    FinalComponentStat,
    FinalComponentOpen,
    FinalComponentFstat,
    FinalRestat,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy)]
pub(crate) struct InjectedFault {
    pub(crate) primitive: AcquisitionPrimitive,
    pub(crate) ordinal: usize,
    pub(crate) error: Errno,
}

#[cfg(any(test, feature = "test-hooks"))]
struct TraceState {
    successful: Vec<AcquisitionPrimitive>,
    attempted: Vec<AcquisitionPrimitive>,
    barrier: Option<(usize, Box<dyn FnOnce()>)>,
    #[cfg(test)]
    barrier_fired: bool,
    fault: Option<InjectedFault>,
    fault_consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug)]
pub(crate) struct TraceOutcome {
    #[cfg(test)]
    pub(crate) successful: Vec<AcquisitionPrimitive>,
    #[cfg(test)]
    pub(crate) attempted: Vec<AcquisitionPrimitive>,
    #[cfg(test)]
    pub(crate) barrier_fired: bool,
    pub(crate) fault_consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static ACQUISITION_TRACE: std::cell::RefCell<Option<TraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
thread_local! {
    static ROOT_STAT_SUBSTITUTION: std::cell::RefCell<Option<RootStatSubstitution>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
struct RootStatSubstitution {
    ordinal: usize,
    seen: usize,
    consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
struct TraceGuard;

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for TraceGuard {
    fn drop(&mut self) {
        ACQUISITION_TRACE.with(|trace| {
            trace.borrow_mut().take();
        });
    }
}

#[cfg(test)]
fn trace_acquisition<T>(
    barrier: Option<(usize, Box<dyn FnOnce()>)>,
    operation: impl FnOnce() -> T,
) -> (T, Vec<AcquisitionPrimitive>) {
    let expects_barrier = barrier.is_some();
    let (result, outcome) = trace_scenario(barrier, None, operation);
    assert!(
        !expects_barrier || outcome.barrier_fired,
        "configured acquisition barrier did not fire"
    );
    (result, outcome.successful)
}

#[cfg(feature = "test-hooks")]
pub fn run_with_acquisition_fault<T>(
    primitive: AcquisitionPrimitive,
    ordinal: usize,
    raw_errno: i32,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    let fault = InjectedFault {
        primitive,
        ordinal,
        error: Errno::from_raw(raw_errno),
    };
    let (result, outcome) = trace_scenario(None, Some(fault), op);
    (result, outcome.fault_consumed)
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn trace_scenario<T>(
    barrier: Option<(usize, Box<dyn FnOnce()>)>,
    fault: Option<InjectedFault>,
    operation: impl FnOnce() -> T,
) -> (T, TraceOutcome) {
    ACQUISITION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "acquisition trace is already active"
        );
        *trace.borrow_mut() = Some(TraceState {
            successful: Vec::new(),
            attempted: Vec::new(),
            barrier,
            #[cfg(test)]
            barrier_fired: false,
            fault,
            fault_consumed: false,
        });
    });
    let guard = TraceGuard;
    let result = operation();
    let state = ACQUISITION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("acquisition trace remains active")
    });
    drop(guard);
    (
        result,
        TraceOutcome {
            #[cfg(test)]
            successful: state.successful,
            #[cfg(test)]
            attempted: state.attempted,
            #[cfg(test)]
            barrier_fired: state.barrier_fired,
            fault_consumed: state.fault_consumed,
        },
    )
}

#[cfg(any(test, feature = "test-hooks"))]
fn attempt_acquisition(primitive: AcquisitionPrimitive) -> Result<(), Errno> {
    ACQUISITION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return Ok(());
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|attempted| **attempted == primitive)
            .count();
        if state
            .fault
            .is_some_and(|fault| fault.primitive == primitive && fault.ordinal == ordinal)
        {
            let fault = state.fault.take().expect("matching injected fault");
            state.fault_consumed = true;
            return Err(fault.error);
        }
        Ok(())
    })
}

#[cfg(any(test, feature = "test-hooks"))]
fn record_success(primitive: AcquisitionPrimitive) {
    let callback = ACQUISITION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state.successful.push(primitive);
        (state.barrier.as_ref().map(|(position, _)| *position) == Some(state.successful.len()))
            .then(|| {
                #[cfg(test)]
                {
                    state.barrier_fired = true;
                }
                state.barrier.take().expect("pending acquisition barrier").1
            })
    });
    if let Some(callback) = callback {
        callback();
    }
}

fn traced_nix<T>(
    primitive: AcquisitionPrimitive,
    operation: impl FnOnce() -> Result<T, Errno>,
) -> Result<T, Errno> {
    #[cfg(not(any(test, feature = "test-hooks")))]
    let _ = primitive;
    #[cfg(any(test, feature = "test-hooks"))]
    attempt_acquisition(primitive)?;
    let result = operation();
    #[cfg(any(test, feature = "test-hooks"))]
    if result.is_ok() {
        record_success(primitive);
    }
    result
}

fn traced_canonicalize(root: &Path) -> io::Result<PathBuf> {
    #[cfg(not(any(test, feature = "test-hooks")))]
    let _ = AcquisitionPrimitive::Canonicalize;
    #[cfg(any(test, feature = "test-hooks"))]
    if let Err(error) = attempt_acquisition(AcquisitionPrimitive::Canonicalize) {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let result = fs::canonicalize(root);
    #[cfg(any(test, feature = "test-hooks"))]
    if result.is_ok() {
        record_success(AcquisitionPrimitive::Canonicalize);
    }
    result
}

#[cfg(test)]
fn with_root_stat_identity_mismatch<T>(ordinal: usize, operation: impl FnOnce() -> T) -> (T, bool) {
    ROOT_STAT_SUBSTITUTION.with(|substitution| {
        assert!(
            substitution.borrow().is_none(),
            "root-stat substitution is already active"
        );
        *substitution.borrow_mut() = Some(RootStatSubstitution {
            ordinal,
            seen: 0,
            consumed: false,
        });
    });
    struct SubstitutionGuard;
    impl Drop for SubstitutionGuard {
        fn drop(&mut self) {
            ROOT_STAT_SUBSTITUTION.with(|substitution| {
                substitution.borrow_mut().take();
            });
        }
    }
    let guard = SubstitutionGuard;
    let result = operation();
    let state = ROOT_STAT_SUBSTITUTION.with(|substitution| {
        substitution
            .borrow_mut()
            .take()
            .expect("root-stat substitution remains active")
    });
    drop(guard);
    (result, state.consumed)
}

#[cfg(test)]
fn substitute_root_stat_identity(mut stat: FileStat) -> FileStat {
    ROOT_STAT_SUBSTITUTION.with(|substitution| {
        let mut substitution = substitution.borrow_mut();
        let Some(state) = substitution.as_mut() else {
            return;
        };
        state.seen += 1;
        if state.seen == state.ordinal {
            stat.st_ino ^= 1;
            state.consumed = true;
        }
    });
    stat
}

pub(crate) struct UnixRoot {
    root: OwnedFd,
    canonical: PathBuf,
    identity: ObjectIdentity,
}

impl AsFd for UnixRoot {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.root.as_fd()
    }
}

impl Backend for UnixRoot {
    fn identity(&self) -> ObjectIdentity {
        self.identity
    }

    fn diagnostic_path(&self) -> &Path {
        &self.canonical
    }

    fn revalidate(&self) -> Result<(), JournalRootError> {
        let stat = fstat(&self.root)
            .map_err(|error| source_io("stat acquired journal root", &self.canonical, error))?;
        if !is_directory(&stat) || object_identity(&stat)? != self.identity {
            return Err(JournalRootError::Changed);
        }
        Ok(())
    }

    fn revalidate_canonical_binding(&self) -> Result<(), JournalRootError> {
        let components = canonical_components(&self.canonical, &self.canonical)?;
        let mut current = open("/", DIRECTORY_FLAGS, Mode::empty())
            .map_err(|error| source_io("open filesystem root", Path::new("/"), error))?;
        if components.is_empty() {
            let stat = fstat(&current)
                .map_err(|error| source_io("stat opened filesystem root", Path::new("/"), error))?;
            if !is_directory(&stat) || object_identity(&stat)? != self.identity {
                return Err(JournalRootError::Changed);
            }
            return Ok(());
        }
        let (final_name, ancestors) = components
            .split_last()
            .expect("nonempty canonical components have a final name");
        for component in ancestors {
            let before = fstatat(
                &current,
                component.as_os_str(),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .map_err(|error| {
                if is_race_error(error) {
                    JournalRootError::Changed
                } else {
                    source_io("stat canonical root component", &self.canonical, error)
                }
            })?;
            if !is_directory(&before) {
                return Err(JournalRootError::Changed);
            }
            let opened = openat(
                &current,
                component.as_os_str(),
                DIRECTORY_FLAGS,
                Mode::empty(),
            )
            .map_err(|error| {
                if is_race_error(error) {
                    JournalRootError::Changed
                } else {
                    source_io("open journal directory", &self.canonical, error)
                }
            })?;
            let after = fstat(&opened).map_err(|error| {
                source_io(
                    "stat opened canonical root component",
                    &self.canonical,
                    error,
                )
            })?;
            if object_identity(&before)? != object_identity(&after)? {
                return Err(JournalRootError::Changed);
            }
            current = opened;
        }
        let before_final = fstatat(
            &current,
            final_name.as_os_str(),
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(|error| {
            if is_race_error(error) {
                JournalRootError::Changed
            } else {
                source_io("stat canonical root component", &self.canonical, error)
            }
        })?;
        if !is_directory(&before_final) || object_identity(&before_final)? != self.identity {
            return Err(JournalRootError::Changed);
        }
        let opened_final = openat(
            &current,
            final_name.as_os_str(),
            DIRECTORY_FLAGS,
            Mode::empty(),
        )
        .map_err(|error| {
            if is_race_error(error) {
                JournalRootError::Changed
            } else {
                source_io("open journal directory", &self.canonical, error)
            }
        })?;
        let after_final = fstat(&opened_final).map_err(|error| {
            source_io(
                "stat opened canonical root component",
                &self.canonical,
                error,
            )
        })?;
        if object_identity(&after_final)? != self.identity {
            return Err(JournalRootError::Changed);
        }
        Ok(())
    }
}

pub(crate) fn acquire(root: &Path) -> Result<UnixRoot, JournalRootError> {
    let (fd, canonical, identity) = acquire_root(root)?;
    Ok(UnixRoot {
        root: fd,
        canonical,
        identity,
    })
}

fn open_requested_root(root: &Path) -> Result<(OwnedFd, ObjectIdentity), JournalRootError> {
    let opened = traced_nix(AcquisitionPrimitive::RequestedRootOpen, || {
        open(root, REQUESTED_ROOT_FLAGS, Mode::empty())
    })
    .map_err(|error| match error {
        Errno::ENOENT => JournalRootError::Invalid {
            root: root.to_path_buf(),
            reason: "journal root does not exist",
            category: None,
        },
        Errno::ENOTDIR | Errno::ELOOP => JournalRootError::Invalid {
            root: root.to_path_buf(),
            reason: "journal root is not a directory",
            category: None,
        },
        other => source_io("open journal root", root, other),
    })?;

    let stat = traced_nix(AcquisitionPrimitive::AuthoritativeFstat, || fstat(&opened))
        .map_err(|error| source_io("stat acquired journal root", root, error))?;

    if !is_directory(&stat) {
        return Err(JournalRootError::Invalid {
            root: root.to_path_buf(),
            reason: "journal root is not a directory",
            category: None,
        });
    }
    Ok((opened, object_identity(&stat)?))
}

fn open_absolute_filesystem_root() -> Result<OwnedFd, JournalRootError> {
    traced_nix(AcquisitionPrimitive::FilesystemRootOpen, || {
        open("/", DIRECTORY_FLAGS, Mode::empty())
    })
    .map_err(|error| acquisition_error(source_io("open filesystem root", Path::new("/"), error)))
}

// Race errnos deliberately do not fold into Changed here, unlike every other post-authority primitive; `root_self_fault_matrix_pins_both_open_and_fstat_ordinals` pins it.
fn stat_filesystem_root(fd: &impl AsFd) -> Result<FileStat, JournalRootError> {
    let stat = traced_nix(AcquisitionPrimitive::FilesystemRootFstat, || fstat(fd))
        .map_err(|error| source_io("stat opened filesystem root", Path::new("/"), error))?;
    #[cfg(test)]
    let stat = substitute_root_stat_identity(stat);
    Ok(stat)
}

fn restat_canonical_root(
    parent: &impl AsFd,
    name: &OsStr,
    expected: ObjectIdentity,
    original: &Path,
) -> Result<(), JournalRootError> {
    let stat = traced_nix(AcquisitionPrimitive::FinalRestat, || {
        fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)
    })
    .map_err(|error| source_io("restat acquired journal root", original, error))
    .map_err(acquisition_error)?;

    if !is_directory(&stat) || object_identity(&stat)? != expected {
        return Err(JournalRootError::Changed);
    }
    Ok(())
}

fn acquire_root(root: &Path) -> Result<(OwnedFd, PathBuf, ObjectIdentity), JournalRootError> {
    if !root.is_absolute() {
        return Err(JournalRootError::Invalid {
            root: root.to_path_buf(),
            reason: "journal root must be absolute",
            category: None,
        });
    }

    let (authoritative, expected) = open_requested_root(root)?;
    let canonical = match traced_canonicalize(root) {
        Ok(path) => path,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                || error
                    .raw_os_error()
                    .is_some_and(|raw| is_race_error(Errno::from_raw(raw))) =>
        {
            return Err(JournalRootError::Changed);
        }
        Err(source) => {
            return Err(JournalRootError::Io {
                operation: "canonicalize journal root",
                path: root.to_path_buf(),
                source,
            });
        }
    };
    let components = canonical_components(&canonical, root)?;
    if components.is_empty() {
        let first = open_absolute_filesystem_root()?;
        let first_stat = stat_filesystem_root(&first)?;
        require_same_root_identity(&first_stat, expected)?;

        let second = open_absolute_filesystem_root()?;
        let second_stat = stat_filesystem_root(&second)?;
        require_same_root_identity(&second_stat, object_identity(&first_stat)?)?;
        require_same_root_identity(&second_stat, expected)?;

        return Ok((authoritative, canonical, expected));
    }

    let mut current = open_absolute_filesystem_root()?;
    let (final_name, ancestors) =
        components
            .split_last()
            .ok_or_else(|| JournalRootError::Invalid {
                root: root.to_path_buf(),
                reason: "canonical journal root has no final component",
                category: None,
            })?;
    let final_name = final_name.clone();
    for component in ancestors {
        let before = traced_nix(AcquisitionPrimitive::ComponentStat, || {
            fstatat(
                &current,
                component.as_os_str(),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            )
        })
        .map_err(|error| source_io("stat canonical root component", root, error))
        .map_err(acquisition_error)?;
        if !is_directory(&before) {
            return Err(JournalRootError::Changed);
        }
        let opened = traced_nix(AcquisitionPrimitive::ComponentOpen, || {
            openat(
                &current,
                component.as_os_str(),
                DIRECTORY_FLAGS,
                Mode::empty(),
            )
        })
        .map_err(|error| {
            if is_race_error(error) {
                JournalRootError::Changed
            } else {
                source_io("open journal directory", root, error)
            }
        })?;
        let after = traced_nix(AcquisitionPrimitive::ComponentFstat, || fstat(&opened))
            .map_err(|error| source_io("stat opened canonical root component", root, error))
            .map_err(acquisition_error)?;
        if object_identity(&before)? != object_identity(&after)? {
            return Err(JournalRootError::Changed);
        }
        current = opened;
    }

    let before_final = traced_nix(AcquisitionPrimitive::FinalComponentStat, || {
        fstatat(
            &current,
            final_name.as_os_str(),
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )
    })
    .map_err(|error| source_io("stat canonical root component", root, error))
    .map_err(acquisition_error)?;
    if !is_directory(&before_final) || object_identity(&before_final)? != expected {
        return Err(JournalRootError::Changed);
    }
    let opened_final = traced_nix(AcquisitionPrimitive::FinalComponentOpen, || {
        openat(
            &current,
            final_name.as_os_str(),
            DIRECTORY_FLAGS,
            Mode::empty(),
        )
    })
    .map_err(|error| {
        if is_race_error(error) {
            JournalRootError::Changed
        } else {
            source_io("open journal directory", root, error)
        }
    })?;
    let after_final = traced_nix(AcquisitionPrimitive::FinalComponentFstat, || {
        fstat(&opened_final)
    })
    .map_err(|error| source_io("stat opened canonical root component", root, error))
    .map_err(acquisition_error)?;
    if object_identity(&after_final)? != expected {
        return Err(JournalRootError::Changed);
    }

    restat_canonical_root(&current, &final_name, expected, root)?;

    Ok((authoritative, canonical, expected))
}

fn canonical_components(
    canonical: &Path,
    original: &Path,
) -> Result<Vec<OsString>, JournalRootError> {
    let mut components = Vec::new();
    for component in canonical.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) if std::str::from_utf8(name.as_bytes()).is_ok() => {
                components.push(name.to_os_string());
            }
            Component::Normal(_) => {
                return Err(JournalRootError::Invalid {
                    root: original.to_path_buf(),
                    reason: "canonical journal root has a non-UTF-8 ancestor",
                    category: None,
                });
            }
            _ => {
                return Err(JournalRootError::Invalid {
                    root: original.to_path_buf(),
                    reason: "canonical journal root is not absolute",
                    category: None,
                });
            }
        }
    }
    Ok(components)
}

fn is_directory(stat: &FileStat) -> bool {
    JournalEntryKind::from_mode(SFlag::from_bits_truncate(stat.st_mode))
        == JournalEntryKind::Directory
}

fn object_identity(stat: &FileStat) -> Result<ObjectIdentity, JournalRootError> {
    Ok(ObjectIdentity::from_device_inode(
        stat_identifier(stat.st_dev)?,
        stat_identifier(stat.st_ino)?,
    ))
}

fn require_same_root_identity(
    observed: &FileStat,
    expected: ObjectIdentity,
) -> Result<(), JournalRootError> {
    if !is_directory(observed) || object_identity(observed)? != expected {
        return Err(JournalRootError::Changed);
    }
    Ok(())
}

fn stat_identifier(value: impl TryInto<u64>) -> Result<u64, JournalRootError> {
    value.try_into().map_err(|_| JournalRootError::Io {
        operation: "read source file identity",
        path: PathBuf::from("/"),
        source: io::Error::new(io::ErrorKind::InvalidData, "source identity is negative"),
    })
}

fn source_io(operation: &'static str, path: &Path, error: Errno) -> JournalRootError {
    JournalRootError::Io {
        operation,
        path: path.to_path_buf(),
        source: io::Error::from_raw_os_error(error as i32),
    }
}

fn is_race_error(error: Errno) -> bool {
    matches!(error, Errno::ENOENT | Errno::ENOTDIR | Errno::ELOOP)
}

fn acquisition_error(error: JournalRootError) -> JournalRootError {
    match error {
        JournalRootError::Io {
            source,
            operation,
            path,
        } if source
            .raw_os_error()
            .is_some_and(|raw| is_race_error(Errno::from_raw(raw))) =>
        {
            let _ = (operation, path);
            JournalRootError::Changed
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::os::fd::AsFd;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::JournalRoot;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-journal-io-journal-root-{name}-{}",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create temporary directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn nested_journal(temporary: &TempDir, bytes: &[u8]) -> PathBuf {
        let root = temporary.path().join("outer/inner/journal");
        let source = root.join("imports/import-1/source.bin");
        fs::create_dir_all(source.parent().expect("source has parent"))
            .expect("create journal parents");
        fs::write(source, bytes).expect("write journal source");
        root
    }

    fn read_member(root: &impl AsFd, parts: &[&str]) -> Vec<u8> {
        let mut current = openat(root, ".", DIRECTORY_FLAGS, Mode::empty()).expect("open root");
        for (index, part) in parts.iter().enumerate() {
            if index + 1 == parts.len() {
                let flags = OFlag::O_RDONLY
                    .union(OFlag::O_CLOEXEC)
                    .union(OFlag::O_NOFOLLOW);
                let file = openat(&current, *part, flags, Mode::empty()).expect("open member");
                let mut bytes = Vec::new();
                fs::File::from(file)
                    .read_to_end(&mut bytes)
                    .expect("read member");
                return bytes;
            }
            current = openat(&current, *part, DIRECTORY_FLAGS, Mode::empty()).expect("open dir");
        }
        panic!("empty member path");
    }

    fn component_count(root: &Path) -> usize {
        canonical_components(
            &fs::canonicalize(root).expect("canonicalize test root"),
            root,
        )
        .expect("canonical test components")
        .len()
    }

    fn component_fstat_position(trace: &[AcquisitionPrimitive], ordinal: usize) -> usize {
        trace
            .iter()
            .enumerate()
            .filter(|(_, primitive)| **primitive == AcquisitionPrimitive::ComponentFstat)
            .nth(ordinal - 1)
            .map(|(index, _)| index + 1)
            .expect("component fstat position")
    }

    #[derive(Clone, Copy)]
    enum ExpectedFault {
        Invalid(&'static str),
        SourceIo(&'static str),
        Changed,
    }

    #[derive(Clone, Copy)]
    struct FaultCase {
        primitive: AcquisitionPrimitive,
        ordinal: usize,
        error: Errno,
        expected: ExpectedFault,
    }

    fn successful_prefix(
        primitive: AcquisitionPrimitive,
        ordinal: usize,
        ancestor_count: usize,
    ) -> Vec<AcquisitionPrimitive> {
        fn extend_ancestors(prefix: &mut Vec<AcquisitionPrimitive>, count: usize) {
            for _ in 0..count {
                prefix.extend([
                    AcquisitionPrimitive::ComponentStat,
                    AcquisitionPrimitive::ComponentOpen,
                    AcquisitionPrimitive::ComponentFstat,
                ]);
            }
        }

        let base = [
            AcquisitionPrimitive::RequestedRootOpen,
            AcquisitionPrimitive::AuthoritativeFstat,
            AcquisitionPrimitive::Canonicalize,
            AcquisitionPrimitive::FilesystemRootOpen,
        ];
        let mut prefix = Vec::new();
        match primitive {
            AcquisitionPrimitive::RequestedRootOpen => {}
            AcquisitionPrimitive::AuthoritativeFstat => {
                prefix.push(AcquisitionPrimitive::RequestedRootOpen);
            }
            AcquisitionPrimitive::Canonicalize => {
                prefix.extend_from_slice(&base[..2]);
            }
            AcquisitionPrimitive::FilesystemRootOpen => {
                prefix.extend_from_slice(&base[..3]);
            }
            AcquisitionPrimitive::FilesystemRootFstat => {
                prefix.extend(base);
            }
            AcquisitionPrimitive::ComponentStat => {
                prefix.extend(base);
                extend_ancestors(&mut prefix, ordinal.saturating_sub(1));
            }
            AcquisitionPrimitive::ComponentOpen => {
                prefix.extend(base);
                extend_ancestors(&mut prefix, ordinal.saturating_sub(1));
                prefix.push(AcquisitionPrimitive::ComponentStat);
            }
            AcquisitionPrimitive::ComponentFstat => {
                prefix.extend(base);
                extend_ancestors(&mut prefix, ordinal.saturating_sub(1));
                prefix.extend([
                    AcquisitionPrimitive::ComponentStat,
                    AcquisitionPrimitive::ComponentOpen,
                ]);
            }
            AcquisitionPrimitive::FinalComponentStat => {
                prefix.extend(base);
                extend_ancestors(&mut prefix, ancestor_count);
            }
            AcquisitionPrimitive::FinalComponentOpen => {
                prefix.extend(base);
                extend_ancestors(&mut prefix, ancestor_count);
                prefix.push(AcquisitionPrimitive::FinalComponentStat);
            }
            AcquisitionPrimitive::FinalComponentFstat => {
                prefix.extend(base);
                extend_ancestors(&mut prefix, ancestor_count);
                prefix.extend([
                    AcquisitionPrimitive::FinalComponentStat,
                    AcquisitionPrimitive::FinalComponentOpen,
                ]);
            }
            AcquisitionPrimitive::FinalRestat => {
                prefix.extend(base);
                extend_ancestors(&mut prefix, ancestor_count);
                prefix.extend([
                    AcquisitionPrimitive::FinalComponentStat,
                    AcquisitionPrimitive::FinalComponentOpen,
                    AcquisitionPrimitive::FinalComponentFstat,
                ]);
            }
        }
        prefix
    }

    fn assert_fault(error: JournalRootError, expected: ExpectedFault, root: &Path, errno: Errno) {
        match (error, expected) {
            (
                JournalRootError::Invalid {
                    root: actual_root,
                    reason: actual_reason,
                    category,
                },
                ExpectedFault::Invalid(expected_reason),
            ) => {
                assert_eq!(actual_root, root);
                assert_eq!(actual_reason, expected_reason);
                assert_eq!(category, None);
            }
            (
                JournalRootError::Io {
                    operation, source, ..
                },
                ExpectedFault::SourceIo(expected_operation),
            ) => {
                assert_eq!(operation, expected_operation);
                assert_eq!(source.raw_os_error(), Some(errno as i32));
            }
            (JournalRootError::Changed, ExpectedFault::Changed) => {}
            (actual, _) => panic!("unexpected acquisition error: {actual:?}"),
        }
    }

    fn root_self_prefix(
        primitive: AcquisitionPrimitive,
        ordinal: usize,
    ) -> Vec<AcquisitionPrimitive> {
        let sequence = [
            AcquisitionPrimitive::RequestedRootOpen,
            AcquisitionPrimitive::AuthoritativeFstat,
            AcquisitionPrimitive::Canonicalize,
            AcquisitionPrimitive::FilesystemRootOpen,
            AcquisitionPrimitive::FilesystemRootFstat,
            AcquisitionPrimitive::FilesystemRootOpen,
            AcquisitionPrimitive::FilesystemRootFstat,
        ];
        let position = sequence
            .iter()
            .enumerate()
            .filter(|(_, candidate)| **candidate == primitive)
            .nth(ordinal - 1)
            .map(|(position, _)| position)
            .expect("root-self primitive ordinal");
        sequence[..position].to_vec()
    }

    fn post_authority_cases(
        primitive: AcquisitionPrimitive,
        operation: &'static str,
    ) -> Vec<FaultCase> {
        [Errno::ENOENT, Errno::ENOTDIR, Errno::ELOOP]
            .into_iter()
            .map(|error| FaultCase {
                primitive,
                ordinal: 1,
                error,
                expected: ExpectedFault::Changed,
            })
            .chain(
                [Errno::EACCES, Errno::EIO]
                    .into_iter()
                    .map(|error| FaultCase {
                        primitive,
                        ordinal: 1,
                        error,
                        expected: ExpectedFault::SourceIo(operation),
                    }),
            )
            .collect()
    }

    #[test]
    fn acquisition_fault_matrix_uses_real_phase_mappers() {
        let temporary = TempDir::new("fault-matrix");
        let root = nested_journal(&temporary, b"source");
        let canonical = fs::canonicalize(&root).expect("canonical test root");
        let ancestor_count = canonical_components(&canonical, &root)
            .expect("canonical components")
            .len()
            .saturating_sub(1);
        let mut cases = vec![
            FaultCase {
                primitive: AcquisitionPrimitive::RequestedRootOpen,
                ordinal: 1,
                error: Errno::ENOENT,
                expected: ExpectedFault::Invalid("journal root does not exist"),
            },
            FaultCase {
                primitive: AcquisitionPrimitive::RequestedRootOpen,
                ordinal: 1,
                error: Errno::ENOTDIR,
                expected: ExpectedFault::Invalid("journal root is not a directory"),
            },
            FaultCase {
                primitive: AcquisitionPrimitive::RequestedRootOpen,
                ordinal: 1,
                error: Errno::ELOOP,
                expected: ExpectedFault::Invalid("journal root is not a directory"),
            },
            FaultCase {
                primitive: AcquisitionPrimitive::RequestedRootOpen,
                ordinal: 1,
                error: Errno::EACCES,
                expected: ExpectedFault::SourceIo("open journal root"),
            },
            FaultCase {
                primitive: AcquisitionPrimitive::RequestedRootOpen,
                ordinal: 1,
                error: Errno::EIO,
                expected: ExpectedFault::SourceIo("open journal root"),
            },
            FaultCase {
                primitive: AcquisitionPrimitive::AuthoritativeFstat,
                ordinal: 1,
                error: Errno::EACCES,
                expected: ExpectedFault::SourceIo("stat acquired journal root"),
            },
            FaultCase {
                primitive: AcquisitionPrimitive::AuthoritativeFstat,
                ordinal: 1,
                error: Errno::EIO,
                expected: ExpectedFault::SourceIo("stat acquired journal root"),
            },
        ];
        for (primitive, operation) in [
            (
                AcquisitionPrimitive::Canonicalize,
                "canonicalize journal root",
            ),
            (
                AcquisitionPrimitive::FilesystemRootOpen,
                "open filesystem root",
            ),
            (
                AcquisitionPrimitive::ComponentStat,
                "stat canonical root component",
            ),
            (
                AcquisitionPrimitive::ComponentOpen,
                "open journal directory",
            ),
            (
                AcquisitionPrimitive::ComponentFstat,
                "stat opened canonical root component",
            ),
            (
                AcquisitionPrimitive::FinalComponentStat,
                "stat canonical root component",
            ),
            (
                AcquisitionPrimitive::FinalComponentOpen,
                "open journal directory",
            ),
            (
                AcquisitionPrimitive::FinalComponentFstat,
                "stat opened canonical root component",
            ),
            (
                AcquisitionPrimitive::FinalRestat,
                "restat acquired journal root",
            ),
        ] {
            cases.extend(post_authority_cases(primitive, operation));
        }
        cases.push(FaultCase {
            primitive: AcquisitionPrimitive::ComponentStat,
            ordinal: 2,
            error: Errno::EIO,
            expected: ExpectedFault::SourceIo("stat canonical root component"),
        });

        for case in cases {
            let prefix = successful_prefix(case.primitive, case.ordinal, ancestor_count);
            let barrier_callback_fired = std::rc::Rc::new(std::cell::Cell::new(false));
            let callback_observer = std::rc::Rc::clone(&barrier_callback_fired);
            let (result, trace) = trace_scenario(
                Some((
                    prefix.len() + 1,
                    Box::new(move || callback_observer.set(true)),
                )),
                Some(InjectedFault {
                    primitive: case.primitive,
                    ordinal: case.ordinal,
                    error: case.error,
                }),
                || JournalRoot::open(&root),
            );
            assert!(
                trace.fault_consumed,
                "fault was not consumed: {:?}",
                case.primitive
            );
            assert!(!trace.barrier_fired);
            assert!(!barrier_callback_fired.get());
            assert_eq!(trace.successful, prefix, "wrong completion prefix");
            let mut attempts = prefix;
            attempts.push(case.primitive);
            assert_eq!(trace.attempted, attempts, "wrong attempted phase");
            let error = match result {
                Ok(_) => panic!("injected acquisition fault must fail"),
                Err(error) => error,
            };
            assert_fault(error, case.expected, &root, case.error);
        }
    }

    #[test]
    fn root_self_fault_matrix_pins_both_open_and_fstat_ordinals() {
        let mut cases = Vec::new();
        for ordinal in 1..=2 {
            for error in [
                Errno::ENOENT,
                Errno::ENOTDIR,
                Errno::ELOOP,
                Errno::EACCES,
                Errno::EIO,
            ] {
                cases.push(FaultCase {
                    primitive: AcquisitionPrimitive::FilesystemRootOpen,
                    ordinal,
                    error,
                    expected: if is_race_error(error) {
                        ExpectedFault::Changed
                    } else {
                        ExpectedFault::SourceIo("open filesystem root")
                    },
                });
                cases.push(FaultCase {
                    primitive: AcquisitionPrimitive::FilesystemRootFstat,
                    ordinal,
                    error,
                    expected: ExpectedFault::SourceIo("stat opened filesystem root"),
                });
            }
        }

        for case in cases {
            let prefix = root_self_prefix(case.primitive, case.ordinal);
            let barrier_callback_fired = std::rc::Rc::new(std::cell::Cell::new(false));
            let callback_observer = std::rc::Rc::clone(&barrier_callback_fired);
            let (result, trace) = trace_scenario(
                Some((
                    prefix.len() + 1,
                    Box::new(move || callback_observer.set(true)),
                )),
                Some(InjectedFault {
                    primitive: case.primitive,
                    ordinal: case.ordinal,
                    error: case.error,
                }),
                || JournalRoot::open(Path::new("/")),
            );
            assert!(trace.fault_consumed);
            assert!(!trace.barrier_fired);
            assert!(!barrier_callback_fired.get());
            assert_eq!(trace.successful, prefix, "wrong completion prefix");
            let mut attempts = prefix;
            attempts.push(case.primitive);
            assert_eq!(trace.attempted, attempts, "wrong attempted phase");
            let error = match result {
                Ok(_) => panic!("injected root-self acquisition fault must fail"),
                Err(error) => error,
            };
            assert_fault(error, case.expected, Path::new("/"), case.error);
        }
    }

    #[test]
    fn root_self_identity_checks_reject_each_substituted_fstat() {
        let root =
            open_absolute_filesystem_root().expect("open root for pure identity helper test");
        let mut observed = fstat(&root).expect("stat root for pure identity helper test");
        let expected = object_identity(&observed).expect("root directory proof");
        observed.st_ino ^= 1;
        assert!(matches!(
            require_same_root_identity(&observed, expected),
            Err(JournalRootError::Changed)
        ));

        for ordinal in 1..=2 {
            let ((result, trace), consumed) = with_root_stat_identity_mismatch(ordinal, || {
                trace_acquisition(None, || JournalRoot::open(Path::new("/")))
            });
            assert!(
                consumed,
                "root fstat substitution {ordinal} was not consumed"
            );
            assert!(matches!(result, Err(JournalRootError::Changed)));
            let mut expected_trace =
                root_self_prefix(AcquisitionPrimitive::FilesystemRootFstat, ordinal);
            expected_trace.push(AcquisitionPrimitive::FilesystemRootFstat);
            assert_eq!(
                trace, expected_trace,
                "wrong successful prefix after substituted root fstat"
            );
        }
    }

    #[test]
    fn acquisition_fault_is_one_shot_and_unreachable_fault_stays_pending() {
        let (root_result, root_trace) = trace_scenario(
            None,
            Some(InjectedFault {
                primitive: AcquisitionPrimitive::ComponentStat,
                ordinal: 1,
                error: Errno::EIO,
            }),
            || JournalRoot::open(Path::new("/")),
        );
        assert!(root_result.is_ok());
        assert!(!root_trace.fault_consumed);
        assert!(
            !root_trace
                .attempted
                .contains(&AcquisitionPrimitive::ComponentStat)
        );

        let temporary = TempDir::new("one-shot");
        let root = nested_journal(&temporary, b"source");
        let ((first, second), trace) = trace_scenario(
            None,
            Some(InjectedFault {
                primitive: AcquisitionPrimitive::Canonicalize,
                ordinal: 1,
                error: Errno::EIO,
            }),
            || (JournalRoot::open(&root), JournalRoot::open(&root)),
        );
        assert!(matches!(first, Err(JournalRootError::Io { .. })));
        assert!(second.is_ok());
        assert!(trace.fault_consumed);
        assert_eq!(
            trace
                .attempted
                .iter()
                .filter(|primitive| **primitive == AcquisitionPrimitive::Canonicalize)
                .count(),
            2
        );
        assert_eq!(
            trace
                .successful
                .iter()
                .filter(|primitive| **primitive == AcquisitionPrimitive::Canonicalize)
                .count(),
            1
        );
    }

    #[test]
    fn acquisition_sequence_opens_authoritative_root_before_canonicalizing() {
        let temporary = TempDir::new("sequence");
        let root = nested_journal(&temporary, b"source");
        let (result, trace) = trace_acquisition(None, || JournalRoot::open(&root));
        let admitted = result.expect("acquire root");

        assert_eq!(
            admitted.canonical_path(),
            fs::canonicalize(&root).expect("canonical root")
        );
        assert_eq!(
            &trace[..3],
            [
                AcquisitionPrimitive::RequestedRootOpen,
                AcquisitionPrimitive::AuthoritativeFstat,
                AcquisitionPrimitive::Canonicalize,
            ]
        );
        assert_eq!(
            trace
                .iter()
                .filter(|primitive| **primitive == AcquisitionPrimitive::Canonicalize)
                .count(),
            1
        );
    }

    #[test]
    fn replacing_unopened_canonical_component_after_authority_is_rejected() {
        let temporary = TempDir::new("replace-unopened");
        let root = nested_journal(&temporary, b"source");
        let (_, trace) = trace_acquisition(None, || JournalRoot::open(&root));
        let outer_position = component_fstat_position(&trace, component_count(&root) - 2);
        let inner = temporary.path().join("outer/inner");

        let (result, _) = trace_acquisition(
            Some((
                outer_position,
                Box::new(move || {
                    fs::remove_dir_all(&inner).expect("remove unopened inner component");
                    let outside = inner.join("journal/imports/import-1/source.bin");
                    fs::create_dir_all(outside.parent().expect("outside source has parent"))
                        .expect("create outside journal");
                    fs::write(outside, b"outside-marker").expect("write outside marker");
                }),
            )),
            || JournalRoot::open(&root),
        );

        assert!(matches!(result, Err(JournalRootError::Changed)));
    }

    #[test]
    fn replacing_final_name_before_final_restat_is_rejected() {
        let temporary = TempDir::new("replace-final");
        let root = nested_journal(&temporary, b"source");
        let (_, trace) = trace_acquisition(None, || JournalRoot::open(&root));
        let final_fstat_position = trace
            .iter()
            .position(|primitive| *primitive == AcquisitionPrimitive::FinalComponentFstat)
            .map(|index| index + 1)
            .expect("final component fstat position");
        let replacement = root.clone();

        let (result, _) = trace_acquisition(
            Some((
                final_fstat_position,
                Box::new(move || {
                    fs::remove_dir_all(&replacement).expect("remove final journal name");
                    let outside = replacement.join("imports/import-1/source.bin");
                    fs::create_dir_all(outside.parent().expect("outside source has parent"))
                        .expect("create replacement journal");
                    fs::write(outside, b"outside-marker").expect("write outside marker");
                }),
            )),
            || JournalRoot::open(&root),
        );

        assert!(matches!(result, Err(JournalRootError::Changed)));
    }

    #[test]
    fn renaming_already_opened_ancestor_mid_walk_still_succeeds() {
        let temporary = TempDir::new("rename-ancestor");
        let root = nested_journal(&temporary, b"source");
        let (_, trace) = trace_acquisition(None, || JournalRoot::open(&root));
        let outer_position = component_fstat_position(&trace, component_count(&root) - 2);
        let outer = temporary.path().join("outer");
        let moved = temporary.path().join("outer-moved");

        let (result, _) = trace_acquisition(
            Some((
                outer_position,
                Box::new(move || {
                    fs::rename(&outer, &moved).expect("rename opened ancestor");
                }),
            )),
            || JournalRoot::open(&root),
        );
        let admitted = result.expect("acquire through retained ancestor");
        assert_eq!(
            read_member(&admitted, &["imports", "import-1", "source.bin"]),
            b"source"
        );
    }

    #[test]
    fn root_self_uses_two_independent_filesystem_root_opens() {
        let (result, trace) = trace_acquisition(None, || JournalRoot::open(Path::new("/")));
        let admitted = result.expect("acquire filesystem root");

        assert_eq!(admitted.canonical_path(), Path::new("/"));
        assert_eq!(
            trace.as_slice(),
            [
                AcquisitionPrimitive::RequestedRootOpen,
                AcquisitionPrimitive::AuthoritativeFstat,
                AcquisitionPrimitive::Canonicalize,
                AcquisitionPrimitive::FilesystemRootOpen,
                AcquisitionPrimitive::FilesystemRootFstat,
                AcquisitionPrimitive::FilesystemRootOpen,
                AcquisitionPrimitive::FilesystemRootFstat,
            ]
        );
    }

    #[test]
    fn root_symlink_to_filesystem_root_uses_two_independent_filesystem_root_opens() {
        let temporary = TempDir::new("root-symlink");
        let root = temporary.path().join("root");
        std::os::unix::fs::symlink("/", &root).expect("create root symlink");

        let (result, trace) = trace_acquisition(None, || JournalRoot::open(&root));
        let admitted = result.expect("acquire filesystem root symlink");

        assert_eq!(admitted.canonical_path(), Path::new("/"));
        assert_eq!(
            trace.as_slice(),
            [
                AcquisitionPrimitive::RequestedRootOpen,
                AcquisitionPrimitive::AuthoritativeFstat,
                AcquisitionPrimitive::Canonicalize,
                AcquisitionPrimitive::FilesystemRootOpen,
                AcquisitionPrimitive::FilesystemRootFstat,
                AcquisitionPrimitive::FilesystemRootOpen,
                AcquisitionPrimitive::FilesystemRootFstat,
            ]
        );
    }

    #[test]
    fn admitted_root_survives_namespace_rename() {
        let temporary = TempDir::new("admit-rename");
        let root = nested_journal(&temporary, b"source");
        let admitted = JournalRoot::open(&root).expect("admit root");
        let identity = admitted.identity();
        let ancestor = temporary.path().join("outer");
        let moved = temporary.path().join("outer-moved");
        fs::rename(&ancestor, &moved).expect("rename admitted ancestor");
        admitted.revalidate().expect("revalidate retained root");
        assert_eq!(admitted.identity(), identity);
        assert_eq!(
            read_member(&admitted, &["imports", "import-1", "source.bin"]),
            b"source"
        );
    }

    #[test]
    fn sentinel_read_through_retained_descriptor() {
        let temporary = TempDir::new("sentinel");
        let root = nested_journal(&temporary, b"source");
        let admitted = JournalRoot::open(&root).expect("admit root");
        let moved = temporary.path().join("journal-moved");
        fs::rename(&root, &moved).expect("move admitted namespace name");
        fs::create_dir(&root).expect("create replacement tree");
        fs::write(root.join("imports"), b"not-the-source").expect("plant replacement");
        assert_eq!(
            read_member(&admitted, &["imports", "import-1", "source.bin"]),
            b"source"
        );
    }

    #[test]
    fn canonical_binding_rejects_symlink_ancestor_while_retained_descriptor_revalidates() {
        let temporary = tempfile::tempdir_in("/var/tmp").unwrap();
        let root = temporary.path().join("outer/inner/journal");
        fs::create_dir_all(&root).unwrap();
        let admitted = JournalRoot::open(&root).expect("admit root");
        admitted.revalidate().expect("retained descriptor");
        admitted
            .revalidate_canonical_binding()
            .expect("recorded pathname still resolves");

        let inner = temporary.path().join("outer/inner");
        let moved = temporary.path().join("outer/inner-moved");
        fs::rename(&inner, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &inner).unwrap();

        admitted
            .revalidate()
            .expect("retained descriptor still names the admitted directory");
        assert!(
            matches!(
                admitted.revalidate_canonical_binding(),
                Err(JournalRootError::Changed)
            ),
            "symlink substitution at an intermediate canonical component must fail the pathname walk"
        );
    }
}
