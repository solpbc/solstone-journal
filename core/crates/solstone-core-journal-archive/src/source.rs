// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};
use solstone_core_journal_io::{JournalRoot, JournalRootError};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DescendantPrimitive {
    ListDirectoryOpen,
    Metadata,
    DirectoryOpen,
    LeafOpen,
    Fstat,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static DESCENDANT_TRACE: std::cell::RefCell<Option<DescendantTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
thread_local! {
    static DESCENDANT_STAT_SUBSTITUTION: std::cell::RefCell<Option<DescendantStatSubstitution>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DescendantEvent {
    primitive: DescendantPrimitive,
    member: Option<String>,
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) struct DescendantFault {
    primitive: DescendantPrimitive,
    member: Option<String>,
    ordinal: usize,
    error: Errno,
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) struct DescendantBarrier {
    pub(crate) primitive: DescendantPrimitive,
    pub(crate) member: Option<String>,
    pub(crate) ordinal: usize,
    pub(crate) callback: Box<dyn FnOnce()>,
}

#[cfg(any(test, feature = "test-hooks"))]
struct DescendantTraceState {
    attempted: Vec<DescendantEvent>,
    successful: Vec<DescendantEvent>,
    fault: Option<DescendantFault>,
    #[cfg(test)]
    fault_consumed: bool,
    barrier: Option<DescendantBarrier>,
    barrier_fired: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug)]
pub(crate) struct DescendantTraceOutcome {
    #[cfg(test)]
    pub(crate) attempted: Vec<DescendantEvent>,
    #[cfg(test)]
    pub(crate) successful: Vec<DescendantEvent>,
    #[cfg(test)]
    pub(crate) fault_consumed: bool,
    pub(crate) barrier_fired: bool,
}

#[cfg(test)]
struct DescendantStatSubstitution {
    member: String,
    ordinal: usize,
    seen: usize,
    consumed: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
struct DescendantTraceGuard;

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for DescendantTraceGuard {
    fn drop(&mut self) {
        DESCENDANT_TRACE.with(|trace| {
            trace.borrow_mut().take();
        });
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn trace_descendants<T>(
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
            #[cfg(test)]
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
            #[cfg(test)]
            attempted: state.attempted,
            #[cfg(test)]
            successful: state.successful,
            #[cfg(test)]
            fault_consumed: state.fault_consumed,
            barrier_fired: state.barrier_fired,
        },
    )
}

#[cfg(any(test, feature = "test-hooks"))]
fn descendant_event(
    primitive: DescendantPrimitive,
    member: Option<&ArchiveMemberName>,
) -> DescendantEvent {
    DescendantEvent {
        primitive,
        member: member.map(|member| member.as_str().to_owned()),
    }
}

#[cfg(any(test, feature = "test-hooks"))]
fn matching_ordinal(events: &[DescendantEvent], candidate: &DescendantEvent) -> usize {
    events.iter().filter(|event| *event == candidate).count()
}

#[cfg(any(test, feature = "test-hooks"))]
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
            #[cfg(test)]
            {
                state.fault_consumed = true;
            }
            return Err(fault.error);
        }
        Ok(())
    })
}

#[cfg(any(test, feature = "test-hooks"))]
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
    #[cfg(not(any(test, feature = "test-hooks")))]
    let _ = (primitive, member);
    #[cfg(any(test, feature = "test-hooks"))]
    let event = descendant_event(primitive, member);
    #[cfg(any(test, feature = "test-hooks"))]
    attempt_descendant(&event)?;
    let result = operation();
    #[cfg(any(test, feature = "test-hooks"))]
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

/// A frozen, capability-rooted portable archive source.
pub struct ArchiveSource {
    root: JournalRoot,
    inventory: Inventory,
}

impl ArchiveSource {
    /// Acquire `root` once and immediately freeze its portable archive inventory.
    pub fn open(root: &Path) -> Result<Self, ArchiveError> {
        let retained_root = JournalRoot::open(root).map_err(map_root_error)?;
        let inventory = crate::inventory::build(&retained_root)?;
        Ok(Self {
            root: retained_root,
            inventory,
        })
    }

    /// Return the inventory frozen when this source was opened.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// Return the verified canonical path acquired when this source was opened.
    pub fn canonical_source(&self) -> &Path {
        self.root.canonical_path()
    }

    /// Re-open a frozen entry through the retained journal descriptor.
    pub fn open_file(&self, entry: &InventoryEntry) -> Result<OpenedInventoryFile, ArchiveError> {
        let (file, proof) = open_verified_file(&self.root, entry.member_name(), entry.proof())?;
        Ok(OpenedInventoryFile::new(File::from(file), proof.size))
    }

    /// Confirm every directory and regular-file identity in the frozen inventory.
    pub fn revalidate(&self) -> Result<(), ArchiveError> {
        self.root.revalidate().map_err(map_root_error)?;
        for proof in &self.inventory.directory_proofs {
            revalidate_directory(&self.root, proof)?;
        }
        for entry in &self.inventory.entries {
            revalidate_file(&self.root, entry)?;
        }
        Ok(())
    }
}

pub(crate) fn map_root_error(error: JournalRootError) -> ArchiveError {
    match error {
        JournalRootError::Invalid { root, reason, .. } => {
            ArchiveError::InvalidJournal { root, reason }
        }
        JournalRootError::Unsupported { root, reason, .. } => {
            ArchiveError::UnsupportedJournal { root, reason }
        }
        JournalRootError::Io {
            operation, source, ..
        } => ArchiveError::SourceIo {
            operation,
            member: None,
            source,
        },
        JournalRootError::Changed => ArchiveError::SourceChanged { member: None },
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
    JournalEntryKind::from_mode(mode)
}

fn open_verified_file(
    root: &impl AsFd,
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
    root: &impl AsFd,
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
    root: &impl AsFd,
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

fn revalidate_directory(root: &impl AsFd, proof: &DirectoryEntryProof) -> Result<(), ArchiveError> {
    let member = member_name(&proof.components)?;
    let directory =
        walk_verified_directories(root, &member, &proof.components, &proof.directories)?;
    drop(directory);
    Ok(())
}

fn revalidate_file(root: &impl AsFd, entry: &InventoryEntry) -> Result<(), ArchiveError> {
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
            && (is_race_error(error)
                || matches!(error, Errno::ENXIO | Errno::ENODEV | Errno::EOPNOTSUPP))
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
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

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

    const DESCENDANT_MEMBER: &str = "imports/import-1/source.bin";

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
            Case {
                primitive: DescendantPrimitive::LeafOpen,
                member: Some(DESCENDANT_MEMBER),
                error: Errno::EOPNOTSUPP,
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

    #[test]
    fn retained_root_inventory_reads_original_bytes_after_ancestor_rename() {
        let temporary = TempDir::new("rename-ancestor");
        let root = nested_journal(&temporary, b"source");
        let source = ArchiveSource::open(&root).expect("open source");
        let outer = temporary.path().join("outer");
        let moved = temporary.path().join("outer-moved");
        fs::rename(&outer, &moved).expect("rename opened ancestor");
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
    fn map_root_error_preserves_refusal_categories() {
        let root = PathBuf::from("/journal");
        match map_root_error(JournalRootError::Invalid {
            root: root.clone(),
            reason: "journal root does not exist",
            category: None,
        }) {
            ArchiveError::InvalidJournal {
                root: actual,
                reason,
            } => {
                assert_eq!(actual, root);
                assert_eq!(reason, "journal root does not exist");
            }
            other => panic!("unexpected mapping: {other:?}"),
        }

        let io_error = io::Error::from_raw_os_error(Errno::EIO as i32);
        match map_root_error(JournalRootError::Io {
            operation: "open journal root",
            path: root.clone(),
            source: io_error,
        }) {
            ArchiveError::SourceIo {
                operation,
                member,
                source,
            } => {
                assert_eq!(operation, "open journal root");
                assert!(member.is_none());
                assert_eq!(source.raw_os_error(), Some(Errno::EIO as i32));
            }
            other => panic!("unexpected mapping: {other:?}"),
        }

        match map_root_error(JournalRootError::Changed) {
            ArchiveError::SourceChanged { member: None } => {}
            other => panic!("unexpected mapping: {other:?}"),
        }

        match map_root_error(JournalRootError::Unsupported {
            root: root.clone(),
            reason: "no retained handle",
            category: None,
        }) {
            ArchiveError::UnsupportedJournal {
                root: actual,
                reason,
            } => {
                assert_eq!(actual, root);
                assert_eq!(reason, "no retained handle");
            }
            other => panic!("unexpected mapping: {other:?}"),
        }

        let result = ArchiveSource::open(Path::new("relative"));
        assert!(
            matches!(
                result,
                Err(ArchiveError::InvalidJournal {
                    reason: "journal root must be absolute",
                    ..
                })
            ),
            "relative open must not succeed as an empty inventory"
        );
    }
}
