// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, open, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};

use crate::entry::{DirectoryEntryProof, DirectoryProof, EntryProof, FileProof};
use crate::{
    ArchiveError, ArchiveMemberName, Inventory, InventoryEntry, JournalEntryKind,
    OpenedInventoryFile,
};

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_NONBLOCK);
const REQUESTED_ROOT_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcquisitionPrimitive {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DescendantPrimitive {
    ListDirectoryOpen,
    Metadata,
    DirectoryOpen,
    LeafOpen,
    Fstat,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct InjectedFault {
    primitive: AcquisitionPrimitive,
    ordinal: usize,
    error: Errno,
}

#[cfg(test)]
struct TraceState {
    successful: Vec<AcquisitionPrimitive>,
    attempted: Vec<AcquisitionPrimitive>,
    barrier: Option<(usize, Box<dyn FnOnce()>)>,
    barrier_fired: bool,
    fault: Option<InjectedFault>,
    fault_consumed: bool,
}

#[cfg(test)]
#[derive(Debug)]
struct TraceOutcome {
    successful: Vec<AcquisitionPrimitive>,
    attempted: Vec<AcquisitionPrimitive>,
    barrier_fired: bool,
    fault_consumed: bool,
}

#[cfg(test)]
thread_local! {
    static ACQUISITION_TRACE: std::cell::RefCell<Option<TraceState>> = const {
        std::cell::RefCell::new(None)
    };
    static ROOT_STAT_SUBSTITUTION: std::cell::RefCell<Option<RootStatSubstitution>> = const {
        std::cell::RefCell::new(None)
    };
    static DESCENDANT_TRACE: std::cell::RefCell<Option<DescendantTraceState>> = const {
        std::cell::RefCell::new(None)
    };
    static DESCENDANT_STAT_SUBSTITUTION: std::cell::RefCell<Option<DescendantStatSubstitution>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
struct RootStatSubstitution {
    ordinal: usize,
    seen: usize,
    consumed: bool,
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct DescendantEvent {
    primitive: DescendantPrimitive,
    member: Option<String>,
}

#[cfg(test)]
struct DescendantFault {
    primitive: DescendantPrimitive,
    member: Option<String>,
    ordinal: usize,
    error: Errno,
}

#[cfg(test)]
struct DescendantBarrier {
    primitive: DescendantPrimitive,
    member: Option<String>,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(test)]
struct DescendantTraceState {
    attempted: Vec<DescendantEvent>,
    successful: Vec<DescendantEvent>,
    fault: Option<DescendantFault>,
    fault_consumed: bool,
    barrier: Option<DescendantBarrier>,
    barrier_fired: bool,
}

#[cfg(test)]
#[derive(Debug)]
struct DescendantTraceOutcome {
    attempted: Vec<DescendantEvent>,
    successful: Vec<DescendantEvent>,
    fault_consumed: bool,
    barrier_fired: bool,
}

#[cfg(test)]
struct DescendantStatSubstitution {
    member: String,
    ordinal: usize,
    seen: usize,
    consumed: bool,
}

#[cfg(test)]
struct TraceGuard;

#[cfg(test)]
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

#[cfg(test)]
fn trace_scenario<T>(
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
            successful: state.successful,
            attempted: state.attempted,
            barrier_fired: state.barrier_fired,
            fault_consumed: state.fault_consumed,
        },
    )
}

#[cfg(test)]
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

#[cfg(test)]
fn record_success(primitive: AcquisitionPrimitive) {
    let callback = ACQUISITION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state.successful.push(primitive);
        (state.barrier.as_ref().map(|(position, _)| *position) == Some(state.successful.len()))
            .then(|| {
                state.barrier_fired = true;
                state.barrier.take().expect("pending acquisition barrier").1
            })
    });
    if let Some(callback) = callback {
        callback();
    }
}

#[cfg(test)]
struct DescendantTraceGuard;

#[cfg(test)]
impl Drop for DescendantTraceGuard {
    fn drop(&mut self) {
        DESCENDANT_TRACE.with(|trace| {
            trace.borrow_mut().take();
        });
    }
}

#[cfg(test)]
fn trace_descendants<T>(
    fault: Option<DescendantFault>,
    barrier: Option<DescendantBarrier>,
    operation: impl FnOnce() -> T,
) -> (T, DescendantTraceOutcome) {
    DESCENDANT_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "descendant trace is already active"
        );
        *trace.borrow_mut() = Some(DescendantTraceState {
            attempted: Vec::new(),
            successful: Vec::new(),
            fault,
            fault_consumed: false,
            barrier,
            barrier_fired: false,
        });
    });
    let guard = DescendantTraceGuard;
    let result = operation();
    let state = DESCENDANT_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("descendant trace remains active")
    });
    drop(guard);
    (
        result,
        DescendantTraceOutcome {
            attempted: state.attempted,
            successful: state.successful,
            fault_consumed: state.fault_consumed,
            barrier_fired: state.barrier_fired,
        },
    )
}

#[cfg(test)]
fn descendant_event(
    primitive: DescendantPrimitive,
    member: Option<&ArchiveMemberName>,
) -> DescendantEvent {
    DescendantEvent {
        primitive,
        member: member.map(|member| member.as_str().to_owned()),
    }
}

#[cfg(test)]
fn matching_ordinal(events: &[DescendantEvent], candidate: &DescendantEvent) -> usize {
    events.iter().filter(|event| *event == candidate).count()
}

#[cfg(test)]
fn attempt_descendant(event: &DescendantEvent) -> Result<(), Errno> {
    DESCENDANT_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return Ok(());
        };
        state.attempted.push(event.clone());
        let ordinal = matching_ordinal(&state.attempted, event);
        if state.fault.as_ref().is_some_and(|fault| {
            fault.primitive == event.primitive
                && fault.member == event.member
                && fault.ordinal == ordinal
        }) {
            let fault = state.fault.take().expect("matching descendant fault");
            state.fault_consumed = true;
            return Err(fault.error);
        }
        Ok(())
    })
}

#[cfg(test)]
fn record_descendant_success(event: DescendantEvent) {
    let callback = DESCENDANT_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state.successful.push(event.clone());
        let ordinal = matching_ordinal(&state.successful, &event);
        let should_fire = state.barrier.as_ref().is_some_and(|barrier| {
            barrier.primitive == event.primitive
                && barrier.member == event.member
                && barrier.ordinal == ordinal
        });
        should_fire.then(|| {
            state.barrier_fired = true;
            state
                .barrier
                .take()
                .expect("matching descendant barrier")
                .callback
        })
    });
    if let Some(callback) = callback {
        callback();
    }
}

fn traced_descendant_nix<T>(
    primitive: DescendantPrimitive,
    member: Option<&ArchiveMemberName>,
    operation: impl FnOnce() -> Result<T, Errno>,
) -> Result<T, Errno> {
    #[cfg(not(test))]
    let _ = (primitive, member);
    #[cfg(test)]
    let event = descendant_event(primitive, member);
    #[cfg(test)]
    attempt_descendant(&event)?;
    let result = operation();
    #[cfg(test)]
    if result.is_ok() {
        record_descendant_success(event);
    }
    result
}

#[cfg(test)]
fn with_descendant_fstat_special_type<T>(
    member: &str,
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    DESCENDANT_STAT_SUBSTITUTION.with(|substitution| {
        assert!(
            substitution.borrow().is_none(),
            "descendant-stat substitution is already active"
        );
        *substitution.borrow_mut() = Some(DescendantStatSubstitution {
            member: member.to_owned(),
            ordinal,
            seen: 0,
            consumed: false,
        });
    });
    struct SubstitutionGuard;
    impl Drop for SubstitutionGuard {
        fn drop(&mut self) {
            DESCENDANT_STAT_SUBSTITUTION.with(|substitution| {
                substitution.borrow_mut().take();
            });
        }
    }
    let guard = SubstitutionGuard;
    let result = operation();
    let state = DESCENDANT_STAT_SUBSTITUTION.with(|substitution| {
        substitution
            .borrow_mut()
            .take()
            .expect("descendant-stat substitution remains active")
    });
    drop(guard);
    (result, state.consumed)
}

#[cfg(test)]
fn substitute_descendant_fstat_type(
    mut stat: FileStat,
    member: Option<&ArchiveMemberName>,
) -> FileStat {
    DESCENDANT_STAT_SUBSTITUTION.with(|substitution| {
        let mut substitution = substitution.borrow_mut();
        let Some(state) = substitution.as_mut() else {
            return;
        };
        if member.map(ArchiveMemberName::as_str) != Some(state.member.as_str()) {
            return;
        }
        state.seen += 1;
        if state.seen == state.ordinal {
            let mut mode = SFlag::from_bits_truncate(stat.st_mode);
            mode.remove(SFlag::S_IFMT);
            mode.insert(SFlag::S_IFIFO);
            stat.st_mode = mode.bits();
            state.consumed = true;
        }
    });
    stat
}

fn traced_nix<T>(
    primitive: AcquisitionPrimitive,
    operation: impl FnOnce() -> Result<T, Errno>,
) -> Result<T, Errno> {
    #[cfg(not(test))]
    let _ = primitive;
    #[cfg(test)]
    attempt_acquisition(primitive)?;
    let result = operation();
    #[cfg(test)]
    if result.is_ok() {
        record_success(primitive);
    }
    result
}

fn traced_canonicalize(root: &Path) -> io::Result<PathBuf> {
    #[cfg(not(test))]
    let _ = AcquisitionPrimitive::Canonicalize;
    #[cfg(test)]
    if let Err(error) = attempt_acquisition(AcquisitionPrimitive::Canonicalize) {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let result = fs::canonicalize(root);
    #[cfg(test)]
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

/// A frozen, capability-rooted portable archive source.
pub struct ArchiveSource {
    root: OwnedFd,
    canonical: PathBuf,
    inventory: Inventory,
}

impl ArchiveSource {
    /// Acquire `root` once and immediately freeze its portable archive inventory.
    pub fn open(root: &Path) -> Result<Self, ArchiveError> {
        let (retained_root, canonical) = acquire_root(root)?;
        let inventory = crate::inventory::build(&retained_root)?;
        Ok(Self {
            root: retained_root,
            canonical,
            inventory,
        })
    }

    /// Return the inventory frozen when this source was opened.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// Return the verified canonical path acquired when this source was opened.
    pub fn canonical_source(&self) -> &Path {
        &self.canonical
    }

    /// Re-open a frozen entry through the retained journal descriptor.
    pub fn open_file(&self, entry: &InventoryEntry) -> Result<OpenedInventoryFile, ArchiveError> {
        let (file, proof) = open_verified_file(&self.root, entry.member_name(), entry.proof())?;
        Ok(OpenedInventoryFile::new(File::from(file), proof.size))
    }

    /// Confirm every directory and regular-file identity in the frozen inventory.
    pub fn revalidate(&self) -> Result<(), ArchiveError> {
        for proof in &self.inventory.directory_proofs {
            revalidate_directory(&self.root, proof)?;
        }
        for entry in &self.inventory.entries {
            revalidate_file(&self.root, entry)?;
        }
        Ok(())
    }
}

pub(crate) fn open_initial_directory(
    parent: &impl AsFd,
    name: &OsStr,
    member: &ArchiveMemberName,
    before: &FileStat,
) -> Result<(OwnedFd, DirectoryProof), ArchiveError> {
    let kind = classify(before);
    if kind != JournalEntryKind::Directory {
        return Err(ArchiveError::UnsafeJournalEntry {
            member: member.clone(),
            kind,
        });
    }
    let opened = open_directory(parent, name, Some(member), true)?;
    let after = stat_fd(&opened, Some(member), "stat opened journal directory")?;
    let before_proof = directory_proof(before)?;
    if !is_directory(&after) || directory_proof(&after)? != before_proof {
        return Err(changed(Some(member)));
    }
    Ok((opened, before_proof))
}

pub(crate) fn open_initial_file(
    parent: &impl AsFd,
    name: &OsStr,
    member: &ArchiveMemberName,
    before: &FileStat,
) -> Result<FileProof, ArchiveError> {
    let kind = classify(before);
    if kind != JournalEntryKind::RegularFile {
        return Err(ArchiveError::UnsafeJournalEntry {
            member: member.clone(),
            kind,
        });
    }
    let opened = open_regular_file(parent, name, Some(member), true)?;
    let after = stat_fd(&opened, Some(member), "stat opened journal file")?;
    let before_proof = file_proof(before)?;
    if !is_regular(&after) || file_proof(&after)? != before_proof {
        return Err(changed(Some(member)));
    }
    Ok(before_proof)
}

pub(crate) fn list_directory(
    directory: &impl AsFd,
    member: Option<&ArchiveMemberName>,
) -> Result<Vec<OsString>, ArchiveError> {
    let mut directory =
        traced_descendant_nix(DescendantPrimitive::ListDirectoryOpen, member, || {
            nix::dir::Dir::openat(directory, ".", DIRECTORY_FLAGS, Mode::empty())
        })
        .map_err(|error| source_io("open journal directory for listing", member, error))?;
    let mut names = Vec::new();
    for entry in directory.iter() {
        let entry = entry.map_err(|error| source_io("list journal directory", member, error))?;
        let bytes = entry.file_name().to_bytes();
        if matches!(bytes, b"." | b"..") {
            continue;
        }
        names.push(OsString::from_vec(bytes.to_vec()));
    }
    names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    Ok(names)
}

pub(crate) fn root_entry_missing(root: &OwnedFd, name: &OsStr) -> Result<bool, ArchiveError> {
    match fstatat(root, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(false),
        Err(Errno::ENOENT) => Ok(true),
        Err(error) => Err(source_io("stat archive root", None, error)),
    }
}

pub(crate) fn utf8_component(
    name: &OsStr,
    member: &ArchiveMemberName,
) -> Result<String, ArchiveError> {
    std::str::from_utf8(name.as_bytes())
        .map(str::to_owned)
        .map_err(|_| ArchiveError::UnsafeJournalEntry {
            member: member.clone(),
            kind: JournalEntryKind::Other,
        })
}

pub(crate) fn member_name(components: &[OsString]) -> Result<ArchiveMemberName, ArchiveError> {
    let mut rendered = Vec::with_capacity(components.len());
    for component in components {
        let placeholder = ArchiveMemberName::new("<invalid>".to_owned());
        rendered.push(utf8_component(component, &placeholder)?);
    }
    Ok(ArchiveMemberName::new(rendered.join("/")))
}

pub(crate) fn is_directory(stat: &FileStat) -> bool {
    classify(stat) == JournalEntryKind::Directory
}

pub(crate) fn is_regular(stat: &FileStat) -> bool {
    classify(stat) == JournalEntryKind::RegularFile
}

pub(crate) fn stat_entry_for_count(
    parent: &impl AsFd,
    name: &OsStr,
    member: &ArchiveMemberName,
) -> Result<FileStat, ArchiveError> {
    stat_entry(parent, name, Some(member), "stat journal entry")
        .map_err(|error| revalidation_error(error, member))
}

pub(crate) fn classify(stat: &FileStat) -> JournalEntryKind {
    classify_mode(SFlag::from_bits_truncate(stat.st_mode))
}

pub(crate) fn classify_mode(mode: SFlag) -> JournalEntryKind {
    match mode & SFlag::S_IFMT {
        SFlag::S_IFREG => JournalEntryKind::RegularFile,
        SFlag::S_IFDIR => JournalEntryKind::Directory,
        SFlag::S_IFLNK => JournalEntryKind::Symlink,
        SFlag::S_IFIFO => JournalEntryKind::Fifo,
        SFlag::S_IFSOCK => JournalEntryKind::Socket,
        SFlag::S_IFCHR => JournalEntryKind::CharacterDevice,
        SFlag::S_IFBLK => JournalEntryKind::BlockDevice,
        _ => JournalEntryKind::Other,
    }
}

fn open_requested_root(root: &Path) -> Result<(OwnedFd, DirectoryProof), ArchiveError> {
    let opened = traced_nix(AcquisitionPrimitive::RequestedRootOpen, || {
        open(root, REQUESTED_ROOT_FLAGS, Mode::empty())
    })
    .map_err(|error| match error {
        Errno::ENOENT => ArchiveError::InvalidJournal {
            root: root.to_path_buf(),
            reason: "journal root does not exist",
        },
        Errno::ENOTDIR | Errno::ELOOP => ArchiveError::InvalidJournal {
            root: root.to_path_buf(),
            reason: "journal root is not a directory",
        },
        other => source_io("open journal root", None, other),
    })?;

    let stat = traced_nix(AcquisitionPrimitive::AuthoritativeFstat, || fstat(&opened))
        .map_err(|error| source_io("stat acquired journal root", None, error))?;

    if !is_directory(&stat) {
        return Err(ArchiveError::InvalidJournal {
            root: root.to_path_buf(),
            reason: "journal root is not a directory",
        });
    }
    Ok((opened, directory_proof(&stat)?))
}

fn open_absolute_filesystem_root() -> Result<OwnedFd, ArchiveError> {
    traced_nix(AcquisitionPrimitive::FilesystemRootOpen, || {
        open("/", DIRECTORY_FLAGS, Mode::empty())
    })
    .map_err(|error| acquisition_error(source_io("open filesystem root", None, error)))
}

fn stat_filesystem_root(fd: &impl AsFd) -> Result<FileStat, ArchiveError> {
    let stat = traced_nix(AcquisitionPrimitive::FilesystemRootFstat, || fstat(fd))
        .map_err(|error| source_io("stat opened filesystem root", None, error))?;
    #[cfg(test)]
    let stat = substitute_root_stat_identity(stat);
    Ok(stat)
}

fn restat_canonical_root(
    parent: &impl AsFd,
    name: &OsStr,
    expected: DirectoryProof,
) -> Result<(), ArchiveError> {
    let stat = traced_nix(AcquisitionPrimitive::FinalRestat, || {
        fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)
    })
    .map_err(|error| source_io("restat acquired journal root", None, error))
    .map_err(acquisition_error)?;

    if !is_directory(&stat) || directory_proof(&stat)? != expected {
        return Err(changed(None));
    }
    Ok(())
}

fn acquire_root(root: &Path) -> Result<(OwnedFd, PathBuf), ArchiveError> {
    if !root.is_absolute() {
        return Err(ArchiveError::InvalidJournal {
            root: root.to_path_buf(),
            reason: "journal root must be absolute",
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
            return Err(changed(None));
        }
        Err(source) => {
            return Err(ArchiveError::SourceIo {
                operation: "canonicalize journal root",
                member: None,
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
        require_same_root_identity(&second_stat, directory_proof(&first_stat)?)?;
        require_same_root_identity(&second_stat, expected)?;

        return Ok((authoritative, canonical));
    }

    let mut current = open_absolute_filesystem_root()?;
    let (final_name, ancestors) =
        components
            .split_last()
            .ok_or_else(|| ArchiveError::InvalidJournal {
                root: root.to_path_buf(),
                reason: "canonical journal root has no final component",
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
        .map_err(|error| source_io("stat canonical root component", None, error))
        .map_err(acquisition_error)?;
        if !is_directory(&before) {
            return Err(changed(None));
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
                changed(None)
            } else {
                source_io("open journal directory", None, error)
            }
        })?;
        let after = traced_nix(AcquisitionPrimitive::ComponentFstat, || fstat(&opened))
            .map_err(|error| source_io("stat opened canonical root component", None, error))
            .map_err(acquisition_error)?;
        if directory_proof(&before)? != directory_proof(&after)? {
            return Err(changed(None));
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
    .map_err(|error| source_io("stat canonical root component", None, error))
    .map_err(acquisition_error)?;
    if !is_directory(&before_final) || directory_proof(&before_final)? != expected {
        return Err(changed(None));
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
            changed(None)
        } else {
            source_io("open journal directory", None, error)
        }
    })?;
    let after_final = traced_nix(AcquisitionPrimitive::FinalComponentFstat, || {
        fstat(&opened_final)
    })
    .map_err(|error| source_io("stat opened canonical root component", None, error))
    .map_err(acquisition_error)?;
    if directory_proof(&after_final)? != expected {
        return Err(changed(None));
    }

    restat_canonical_root(&current, &final_name, expected)?;

    Ok((authoritative, canonical))
}

fn canonical_components(canonical: &Path, original: &Path) -> Result<Vec<OsString>, ArchiveError> {
    let mut components = Vec::new();
    for component in canonical.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) if std::str::from_utf8(name.as_bytes()).is_ok() => {
                components.push(name.to_os_string());
            }
            Component::Normal(_) => {
                return Err(ArchiveError::InvalidJournal {
                    root: original.to_path_buf(),
                    reason: "canonical journal root has a non-UTF-8 ancestor",
                });
            }
            _ => {
                return Err(ArchiveError::InvalidJournal {
                    root: original.to_path_buf(),
                    reason: "canonical journal root is not absolute",
                });
            }
        }
    }
    Ok(components)
}

fn open_verified_file(
    root: &OwnedFd,
    member: &ArchiveMemberName,
    proof: &EntryProof,
) -> Result<(OwnedFd, FileProof), ArchiveError> {
    let mut current = open_verified_route(root, member, proof)?;
    let Some(name) = proof.components.last() else {
        return Err(changed(Some(member)));
    };
    let before = stat_entry(&current, name, Some(member), "stat inventoried file")
        .map_err(|error| revalidation_error(error, member))?;
    if !is_regular(&before) || file_proof(&before)? != proof.file {
        return Err(changed(Some(member)));
    }
    let opened = open_regular_file(&current, name, Some(member), true)?;
    let after = stat_fd(&opened, Some(member), "stat opened inventoried file")?;
    if !is_regular(&after)
        || file_proof(&after)? != proof.file
        || file_proof(&after)? != file_proof(&before)?
    {
        return Err(changed(Some(member)));
    }
    current = opened;
    Ok((current, proof.file))
}

fn open_verified_route(
    root: &OwnedFd,
    member: &ArchiveMemberName,
    proof: &EntryProof,
) -> Result<OwnedFd, ArchiveError> {
    if proof.components.len() != proof.directories.len().saturating_add(1) {
        return Err(changed(Some(member)));
    }
    walk_verified_directories(
        root,
        member,
        &proof.components[..proof.components.len() - 1],
        &proof.directories,
    )
}

fn walk_verified_directories(
    root: &OwnedFd,
    member: &ArchiveMemberName,
    components: &[OsString],
    directories: &[DirectoryProof],
) -> Result<OwnedFd, ArchiveError> {
    if components.len() != directories.len() {
        return Err(changed(Some(member)));
    }
    let mut current = openat(root, ".", DIRECTORY_FLAGS, Mode::empty())
        .map_err(|error| source_io("open retained journal root", Some(member), error))?;
    for (name, expected) in components.iter().zip(directories.iter()) {
        let before = stat_entry(&current, name, Some(member), "stat inventoried directory")
            .map_err(|error| revalidation_error(error, member))?;
        if !is_directory(&before) || directory_proof(&before)? != *expected {
            return Err(changed(Some(member)));
        }
        let opened = open_directory(&current, name, Some(member), true)?;
        let after = stat_fd(&opened, Some(member), "stat opened inventoried directory")?;
        if !is_directory(&after)
            || directory_proof(&after)? != *expected
            || directory_proof(&after)? != directory_proof(&before)?
        {
            return Err(changed(Some(member)));
        }
        current = opened;
    }
    Ok(current)
}

fn revalidate_directory(root: &OwnedFd, proof: &DirectoryEntryProof) -> Result<(), ArchiveError> {
    let member = member_name(&proof.components)?;
    let directory =
        walk_verified_directories(root, &member, &proof.components, &proof.directories)?;
    drop(directory);
    Ok(())
}

fn revalidate_file(root: &OwnedFd, entry: &InventoryEntry) -> Result<(), ArchiveError> {
    let file = open_verified_file(root, entry.member_name(), entry.proof())?.0;
    drop(file);
    Ok(())
}

fn stat_entry(
    parent: &impl AsFd,
    name: &OsStr,
    member: Option<&ArchiveMemberName>,
    operation: &'static str,
) -> Result<FileStat, ArchiveError> {
    traced_descendant_nix(DescendantPrimitive::Metadata, member, || {
        fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)
    })
    .map_err(|error| source_io(operation, member, error))
}

fn stat_fd(
    fd: &impl AsFd,
    member: Option<&ArchiveMemberName>,
    operation: &'static str,
) -> Result<FileStat, ArchiveError> {
    let stat = traced_descendant_nix(DescendantPrimitive::Fstat, member, || fstat(fd))
        .map_err(|error| source_io(operation, member, error))?;
    #[cfg(test)]
    let stat = substitute_descendant_fstat_type(stat, member);
    Ok(stat)
}

fn open_directory(
    parent: &impl AsFd,
    name: &OsStr,
    member: Option<&ArchiveMemberName>,
    changed_on_race: bool,
) -> Result<OwnedFd, ArchiveError> {
    traced_descendant_nix(DescendantPrimitive::DirectoryOpen, member, || {
        openat(parent, name, DIRECTORY_FLAGS, Mode::empty())
    })
    .map_err(|error| {
        if changed_on_race && is_race_error(error) {
            changed(member)
        } else {
            source_io("open journal directory", member, error)
        }
    })
}

fn open_regular_file(
    parent: &impl AsFd,
    name: &OsStr,
    member: Option<&ArchiveMemberName>,
    changed_on_race: bool,
) -> Result<OwnedFd, ArchiveError> {
    traced_descendant_nix(DescendantPrimitive::LeafOpen, member, || {
        openat(parent, name, FILE_FLAGS, Mode::empty())
    })
    .map_err(|error| {
        if changed_on_race
            && (is_race_error(error) || matches!(error, Errno::ENXIO | Errno::ENODEV))
        {
            changed(member)
        } else {
            source_io("open journal file", member, error)
        }
    })
}

fn directory_proof(stat: &FileStat) -> Result<DirectoryProof, ArchiveError> {
    Ok(DirectoryProof {
        device: stat_identifier(stat.st_dev)?,
        inode: stat_identifier(stat.st_ino)?,
    })
}

fn require_same_root_identity(
    observed: &FileStat,
    expected: DirectoryProof,
) -> Result<(), ArchiveError> {
    if !is_directory(observed) || directory_proof(observed)? != expected {
        return Err(changed(None));
    }
    Ok(())
}

fn file_proof(stat: &FileStat) -> Result<FileProof, ArchiveError> {
    let size = u64::try_from(stat.st_size).map_err(|_| ArchiveError::SourceIo {
        operation: "read regular-file size",
        member: None,
        source: io::Error::new(io::ErrorKind::InvalidData, "regular-file size is negative"),
    })?;
    Ok(FileProof {
        device: stat_identifier(stat.st_dev)?,
        inode: stat_identifier(stat.st_ino)?,
        size,
    })
}

fn stat_identifier(value: impl TryInto<u64>) -> Result<u64, ArchiveError> {
    value.try_into().map_err(|_| ArchiveError::SourceIo {
        operation: "read source file identity",
        member: None,
        source: io::Error::new(io::ErrorKind::InvalidData, "source identity is negative"),
    })
}

fn source_io(
    operation: &'static str,
    member: Option<&ArchiveMemberName>,
    error: Errno,
) -> ArchiveError {
    ArchiveError::SourceIo {
        operation,
        member: member.cloned(),
        source: io::Error::from_raw_os_error(error as i32),
    }
}

fn changed(member: Option<&ArchiveMemberName>) -> ArchiveError {
    ArchiveError::SourceChanged {
        member: member.cloned(),
    }
}

fn is_race_error(error: Errno) -> bool {
    matches!(error, Errno::ENOENT | Errno::ENOTDIR | Errno::ELOOP)
}

fn acquisition_error(error: ArchiveError) -> ArchiveError {
    match error {
        ArchiveError::SourceIo {
            source,
            operation,
            member,
        } if source
            .raw_os_error()
            .is_some_and(|raw| is_race_error(Errno::from_raw(raw))) =>
        {
            let _ = (operation, member);
            changed(None)
        }
        other => other,
    }
}

fn revalidation_error(error: ArchiveError, member: &ArchiveMemberName) -> ArchiveError {
    match error {
        ArchiveError::SourceIo { source, .. }
            if source
                .raw_os_error()
                .is_some_and(|raw| is_race_error(Errno::from_raw(raw))) =>
        {
            changed(Some(member))
        }
        other => other,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use nix::unistd::mkfifo;

    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "solstone-core-journal-archive-source-{name}-{}",
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

    fn short_temp_dir() -> TempDir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from("/tmp").join(format!(
            "sja-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create short temporary directory");
        TempDir { path }
    }

    const DESCENDANT_BARRIER_ROOT: &str = "SOLSTONE_ARCHIVE_DESCENDANT_BARRIER_ROOT";
    const DESCENDANT_BARRIER_MODE: &str = "SOLSTONE_ARCHIVE_DESCENDANT_BARRIER_MODE";
    const DESCENDANT_BARRIER_KIND: &str = "SOLSTONE_ARCHIVE_DESCENDANT_BARRIER_KIND";
    const DESCENDANT_MEMBER: &str = "imports/import-1/source.bin";

    fn replacement_barrier(root: &Path, kind: &str) -> DescendantBarrier {
        let target = root.join(DESCENDANT_MEMBER);
        let kind = kind.to_owned();
        DescendantBarrier {
            primitive: DescendantPrimitive::Metadata,
            member: Some(DESCENDANT_MEMBER.to_owned()),
            ordinal: 1,
            callback: Box::new(move || {
                fs::remove_file(&target).expect("remove inventoried file at stat/open barrier");
                match kind.as_str() {
                    "fifo" => {
                        mkfifo(&target, Mode::S_IRUSR | Mode::S_IWUSR).expect("create barrier fifo")
                    }
                    "socket" => {
                        let listener = UnixListener::bind(&target).expect("create barrier socket");
                        drop(listener);
                    }
                    _ => panic!("unknown barrier replacement kind"),
                }
            }),
        }
    }

    #[test]
    fn descendant_barrier_child() {
        let Some(root) = std::env::var_os(DESCENDANT_BARRIER_ROOT).map(PathBuf::from) else {
            return;
        };
        let mode = std::env::var(DESCENDANT_BARRIER_MODE).expect("barrier child mode");
        let kind = std::env::var(DESCENDANT_BARRIER_KIND).expect("barrier child kind");

        let (result, trace) =
            if mode == "initial" {
                trace_descendants(None, Some(replacement_barrier(&root, &kind)), || {
                    ArchiveSource::open(&root).map(|_| ())
                })
            } else {
                let source = ArchiveSource::open(&root).expect("open source before barrier");
                let entry = source
                    .inventory()
                    .entries()
                    .iter()
                    .find(|entry| entry.member_name().as_str() == DESCENDANT_MEMBER)
                    .expect("barrier inventory entry");
                trace_descendants(None, Some(replacement_barrier(&root, &kind)), || match mode
                    .as_str()
                {
                    "open-file" => source.open_file(entry).map(|_| ()),
                    "revalidate" => source.revalidate(),
                    _ => panic!("unknown barrier child mode"),
                })
            };

        assert!(trace.barrier_fired, "stat/open barrier did not fire");
        assert!(matches!(
            result,
            Err(ArchiveError::SourceChanged { member: Some(member) })
                if member.as_str() == DESCENDANT_MEMBER
        ));
    }

    #[test]
    fn descendant_stat_to_open_swaps_are_bounded_and_changed() {
        for mode in ["initial", "open-file", "revalidate"] {
            for kind in ["fifo", "socket"] {
                let temporary = short_temp_dir();
                let root = nested_journal(&temporary, b"source");
                let mut child = Command::new(std::env::current_exe().expect("current test binary"))
                    .args(["--exact", "source::tests::descendant_barrier_child"])
                    .env(DESCENDANT_BARRIER_ROOT, &root)
                    .env(DESCENDANT_BARRIER_MODE, mode)
                    .env(DESCENDANT_BARRIER_KIND, kind)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn bounded barrier child");
                let deadline = Instant::now() + Duration::from_secs(3);
                let status = loop {
                    if let Some(status) = child.try_wait().expect("wait for barrier child") {
                        break status;
                    }
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("{mode}/{kind} barrier child exceeded bounded deadline");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                };
                let mut stdout = String::new();
                let mut stderr = String::new();
                child
                    .stdout
                    .take()
                    .expect("barrier child stdout")
                    .read_to_string(&mut stdout)
                    .expect("read barrier child stdout");
                child
                    .stderr
                    .take()
                    .expect("barrier child stderr")
                    .read_to_string(&mut stderr)
                    .expect("read barrier child stderr");
                assert!(
                    status.success(),
                    "{mode}/{kind} barrier child failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
                );
            }
        }
    }

    #[test]
    fn descendant_fault_matrix_preserves_phase_member_and_errno() {
        struct Case {
            primitive: DescendantPrimitive,
            member: Option<&'static str>,
            error: Errno,
            operation: Option<&'static str>,
        }
        let cases = [
            Case {
                primitive: DescendantPrimitive::ListDirectoryOpen,
                member: None,
                error: Errno::EIO,
                operation: Some("open journal directory for listing"),
            },
            Case {
                primitive: DescendantPrimitive::Metadata,
                member: Some("imports"),
                error: Errno::EIO,
                operation: Some("stat journal entry"),
            },
            Case {
                primitive: DescendantPrimitive::DirectoryOpen,
                member: Some("imports"),
                error: Errno::EACCES,
                operation: Some("open journal directory"),
            },
            Case {
                primitive: DescendantPrimitive::LeafOpen,
                member: Some(DESCENDANT_MEMBER),
                error: Errno::EACCES,
                operation: Some("open journal file"),
            },
            Case {
                primitive: DescendantPrimitive::Fstat,
                member: Some(DESCENDANT_MEMBER),
                error: Errno::EIO,
                operation: Some("stat opened journal file"),
            },
            Case {
                primitive: DescendantPrimitive::Metadata,
                member: Some(DESCENDANT_MEMBER),
                error: Errno::ENOENT,
                operation: None,
            },
            Case {
                primitive: DescendantPrimitive::DirectoryOpen,
                member: Some("imports"),
                error: Errno::ELOOP,
                operation: None,
            },
            Case {
                primitive: DescendantPrimitive::LeafOpen,
                member: Some(DESCENDANT_MEMBER),
                error: Errno::ENXIO,
                operation: None,
            },
            Case {
                primitive: DescendantPrimitive::LeafOpen,
                member: Some(DESCENDANT_MEMBER),
                error: Errno::ENODEV,
                operation: None,
            },
        ];

        for case in cases {
            let temporary = TempDir::new("descendant-fault");
            let root = nested_journal(&temporary, b"source");
            let event = DescendantEvent {
                primitive: case.primitive,
                member: case.member.map(str::to_owned),
            };
            let (result, trace) = trace_descendants(
                Some(DescendantFault {
                    primitive: case.primitive,
                    member: event.member.clone(),
                    ordinal: 1,
                    error: case.error,
                }),
                None,
                || ArchiveSource::open(&root),
            );
            assert!(trace.fault_consumed, "fault was not consumed: {event:?}");
            assert!(!trace.barrier_fired);
            assert_eq!(trace.attempted.last(), Some(&event));
            assert_eq!(
                trace
                    .successful
                    .iter()
                    .filter(|actual| **actual == event)
                    .count(),
                0,
                "faulted operation completed successfully"
            );
            let error = match result {
                Ok(_) => panic!("descendant fault must fail: {event:?}"),
                Err(error) => error,
            };
            match (error, case.operation) {
                (
                    ArchiveError::SourceIo {
                        operation,
                        member,
                        source,
                    },
                    Some(expected_operation),
                ) => {
                    assert_eq!(operation, expected_operation);
                    assert_eq!(member.as_ref().map(ArchiveMemberName::as_str), case.member);
                    assert_eq!(source.raw_os_error(), Some(case.error as i32));
                }
                (ArchiveError::SourceChanged { member }, None) => {
                    assert_eq!(member.as_ref().map(ArchiveMemberName::as_str), case.member);
                }
                (actual, _) => panic!("unexpected descendant fault: {actual:?}"),
            }
        }
    }

    #[test]
    fn proof_preserving_post_open_special_type_is_rejected() {
        for mode in ["initial", "open-file", "revalidate"] {
            let temporary = TempDir::new("post-open-special-type");
            let root = nested_journal(&temporary, b"");
            let leaf_fstat_ordinal = if mode == "initial" { 1 } else { 3 };
            let (result, consumed) = if mode == "initial" {
                with_descendant_fstat_special_type(DESCENDANT_MEMBER, leaf_fstat_ordinal, || {
                    ArchiveSource::open(&root).map(|_| ())
                })
            } else {
                let source = ArchiveSource::open(&root).expect("open source before substitution");
                let entry = source
                    .inventory()
                    .entries()
                    .iter()
                    .find(|entry| entry.member_name().as_str() == DESCENDANT_MEMBER)
                    .expect("substitution inventory entry");
                with_descendant_fstat_special_type(DESCENDANT_MEMBER, leaf_fstat_ordinal, || {
                    match mode {
                        "open-file" => source.open_file(entry).map(|_| ()),
                        "revalidate" => source.revalidate(),
                        _ => panic!("unknown substitution mode"),
                    }
                })
            };
            assert!(consumed, "{mode} fstat substitution was not consumed");
            let error = match result {
                Ok(()) => panic!("{mode} accepted a substituted special-file type"),
                Err(error) => error,
            };
            assert!(
                matches!(
                    &error,
                    ArchiveError::SourceChanged { member: Some(member) }
                        if member.as_str() == DESCENDANT_MEMBER
                ),
                "{mode} returned the wrong error: {error:?}"
            );
        }
    }

    #[test]
    fn proof_preserving_post_open_directory_type_is_rejected() {
        for mode in ["initial", "open-file", "revalidate"] {
            let temporary = TempDir::new("post-open-special-directory");
            let root = nested_journal(&temporary, b"source");
            let substituted_member = if mode == "initial" {
                "imports"
            } else {
                DESCENDANT_MEMBER
            };
            let (result, consumed) = if mode == "initial" {
                with_descendant_fstat_special_type(substituted_member, 1, || {
                    ArchiveSource::open(&root).map(|_| ())
                })
            } else {
                let source = ArchiveSource::open(&root).expect("open source before substitution");
                let entry = source
                    .inventory()
                    .entries()
                    .iter()
                    .find(|entry| entry.member_name().as_str() == DESCENDANT_MEMBER)
                    .expect("substitution inventory entry");
                with_descendant_fstat_special_type(substituted_member, 1, || match mode {
                    "open-file" => source.open_file(entry).map(|_| ()),
                    "revalidate" => source.revalidate(),
                    _ => panic!("unknown substitution mode"),
                })
            };
            assert!(
                consumed,
                "{mode} directory fstat substitution was not consumed"
            );
            let error = match result {
                Ok(()) => panic!("{mode} accepted a substituted special-directory type"),
                Err(error) => error,
            };
            assert!(
                matches!(
                    &error,
                    ArchiveError::SourceChanged { member: Some(member) }
                        if member.as_str() == substituted_member
                ),
                "{mode} returned the wrong error: {error:?}"
            );
        }
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

    fn assert_fault(error: ArchiveError, expected: ExpectedFault, root: &Path, errno: Errno) {
        match (error, expected) {
            (
                ArchiveError::InvalidJournal {
                    root: actual_root,
                    reason: actual_reason,
                },
                ExpectedFault::Invalid(expected_reason),
            ) => {
                assert_eq!(actual_root, root);
                assert_eq!(actual_reason, expected_reason);
            }
            (
                ArchiveError::SourceIo {
                    operation,
                    member,
                    source,
                },
                ExpectedFault::SourceIo(expected_operation),
            ) => {
                assert_eq!(operation, expected_operation);
                assert!(member.is_none());
                assert_eq!(source.raw_os_error(), Some(errno as i32));
            }
            (ArchiveError::SourceChanged { member: None }, ExpectedFault::Changed) => {}
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
                || ArchiveSource::open(&root),
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
                || acquire_root(Path::new("/")),
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
        let expected = directory_proof(&observed).expect("root directory proof");
        observed.st_ino ^= 1;
        assert!(matches!(
            require_same_root_identity(&observed, expected),
            Err(ArchiveError::SourceChanged { member: None })
        ));

        for ordinal in 1..=2 {
            let ((result, trace), consumed) = with_root_stat_identity_mismatch(ordinal, || {
                trace_acquisition(None, || acquire_root(Path::new("/")))
            });
            assert!(
                consumed,
                "root fstat substitution {ordinal} was not consumed"
            );
            assert!(matches!(
                result,
                Err(ArchiveError::SourceChanged { member: None })
            ));
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
            || ArchiveSource::open(Path::new("/")),
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
            || (ArchiveSource::open(&root), ArchiveSource::open(&root)),
        );
        assert!(matches!(first, Err(ArchiveError::SourceIo { .. })));
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
    fn acquisition_faults_are_thread_local() {
        let rendezvous = std::sync::Arc::new(std::sync::Barrier::new(3));
        std::thread::scope(|scope| {
            let first_rendezvous = std::sync::Arc::clone(&rendezvous);
            scope.spawn(move || {
                let (result, trace) = trace_scenario(
                    None,
                    Some(InjectedFault {
                        primitive: AcquisitionPrimitive::RequestedRootOpen,
                        ordinal: 1,
                        error: Errno::EACCES,
                    }),
                    || {
                        first_rendezvous.wait();
                        ArchiveSource::open(Path::new("/"))
                    },
                );
                assert!(trace.fault_consumed);
                let actual = match result {
                    Ok(_) => panic!("thread-local requested-root fault must fail"),
                    Err(actual) => actual,
                };
                assert_fault(
                    actual,
                    ExpectedFault::SourceIo("open journal root"),
                    Path::new("/"),
                    Errno::EACCES,
                );
            });

            let second_rendezvous = std::sync::Arc::clone(&rendezvous);
            scope.spawn(move || {
                let (result, trace) = trace_scenario(
                    None,
                    Some(InjectedFault {
                        primitive: AcquisitionPrimitive::Canonicalize,
                        ordinal: 1,
                        error: Errno::EIO,
                    }),
                    || {
                        second_rendezvous.wait();
                        ArchiveSource::open(Path::new("/"))
                    },
                );
                assert!(trace.fault_consumed);
                let actual = match result {
                    Ok(_) => panic!("thread-local canonicalize fault must fail"),
                    Err(actual) => actual,
                };
                assert_fault(
                    actual,
                    ExpectedFault::SourceIo("canonicalize journal root"),
                    Path::new("/"),
                    Errno::EIO,
                );
            });

            rendezvous.wait();
        });
    }

    #[test]
    fn acquisition_sequence_opens_authoritative_root_before_canonicalizing() {
        let temporary = TempDir::new("sequence");
        let root = nested_journal(&temporary, b"source");
        let (result, trace) = trace_acquisition(None, || acquire_root(&root));
        let (_root, canonical) = result.expect("acquire root");

        assert_eq!(canonical, fs::canonicalize(&root).expect("canonical root"));
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
        let (_, trace) = trace_acquisition(None, || acquire_root(&root));
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
            || ArchiveSource::open(&root),
        );

        assert!(matches!(
            result,
            Err(ArchiveError::SourceChanged { member: None })
        ));
    }

    #[test]
    fn replacing_final_name_before_final_restat_is_rejected() {
        let temporary = TempDir::new("replace-final");
        let root = nested_journal(&temporary, b"source");
        let (_, trace) = trace_acquisition(None, || acquire_root(&root));
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
            || ArchiveSource::open(&root),
        );

        assert!(matches!(
            result,
            Err(ArchiveError::SourceChanged { member: None })
        ));
    }

    #[test]
    fn renaming_already_opened_ancestor_mid_walk_still_succeeds() {
        let temporary = TempDir::new("rename-ancestor");
        let root = nested_journal(&temporary, b"source");
        let (_, trace) = trace_acquisition(None, || acquire_root(&root));
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
            || ArchiveSource::open(&root),
        );
        let source = result.expect("acquire through retained ancestor");
        let entry = source
            .inventory()
            .entries()
            .iter()
            .find(|entry| entry.member_name().as_str() == "imports/import-1/source.bin")
            .expect("inventoried source");
        let mut bytes = Vec::new();
        source
            .open_file(entry)
            .expect("open retained source")
            .into_file()
            .read_to_end(&mut bytes)
            .expect("read retained source");
        assert_eq!(bytes, b"source");
    }

    #[test]
    fn root_self_uses_two_independent_filesystem_root_opens() {
        let (result, trace) = trace_acquisition(None, || acquire_root(Path::new("/")));
        let (_root, canonical) = result.expect("acquire filesystem root");

        assert_eq!(canonical, PathBuf::from("/"));
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

        let (result, trace) = trace_acquisition(None, || acquire_root(&root));
        let (_root, canonical) = result.expect("acquire filesystem root symlink");

        assert_eq!(canonical, PathBuf::from("/"));
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
}
