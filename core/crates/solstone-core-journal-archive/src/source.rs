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

use crate::entry::{DirectoryProof, EntryProof, FileProof};
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
    .union(OFlag::O_NOFOLLOW);
const REQUESTED_ROOT_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC);

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcquisitionPrimitive {
    RequestedRootOpen,
    AuthoritativeFstat,
    Canonicalize,
    FilesystemRootOpen,
    ComponentStat,
    ComponentOpen,
    ComponentFstat,
    FinalComponentStat,
    FinalComponentOpen,
    FinalComponentFstat,
    FinalRestat,
}

#[cfg(test)]
struct TraceState {
    recorded: Vec<AcquisitionPrimitive>,
    barrier: Option<(usize, Box<dyn FnOnce()>)>,
}

#[cfg(test)]
thread_local! {
    static ACQUISITION_TRACE: std::cell::RefCell<Option<TraceState>> = const {
        std::cell::RefCell::new(None)
    };
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
    ACQUISITION_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "acquisition trace is already active"
        );
        *trace.borrow_mut() = Some(TraceState {
            recorded: Vec::new(),
            barrier,
        });
    });
    let guard = TraceGuard;
    let result = operation();
    let recorded = ACQUISITION_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("acquisition trace remains active")
            .recorded
    });
    drop(guard);
    (result, recorded)
}

#[cfg(test)]
fn record(primitive: AcquisitionPrimitive) {
    let callback = ACQUISITION_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let state = trace.as_mut()?;
        state.recorded.push(primitive);
        (state.barrier.as_ref().map(|(position, _)| *position) == Some(state.recorded.len()))
            .then(|| state.barrier.take().expect("pending acquisition barrier").1)
    });
    if let Some(callback) = callback {
        callback();
    }
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

    /// Confirm every recorded directory and leaf identity for an entry.
    pub fn revalidate(&self, entry: &InventoryEntry) -> Result<(), ArchiveError> {
        let file = open_verified_file(&self.root, entry.member_name(), entry.proof())?.0;
        drop(file);
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
    if directory_proof(&after)? != before_proof {
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
    if file_proof(&after)? != before_proof {
        return Err(changed(Some(member)));
    }
    Ok(before_proof)
}

pub(crate) fn list_directory(
    directory: &impl AsFd,
    member: Option<&ArchiveMemberName>,
) -> Result<Vec<OsString>, ArchiveError> {
    let mut directory = nix::dir::Dir::openat(directory, ".", DIRECTORY_FLAGS, Mode::empty())
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
    let opened = open(root, REQUESTED_ROOT_FLAGS, Mode::empty()).map_err(|error| match error {
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
    #[cfg(test)]
    record(AcquisitionPrimitive::RequestedRootOpen);

    let stat = stat_fd(&opened, None, "stat acquired journal root")?;
    #[cfg(test)]
    record(AcquisitionPrimitive::AuthoritativeFstat);

    if !is_directory(&stat) {
        return Err(ArchiveError::InvalidJournal {
            root: root.to_path_buf(),
            reason: "journal root is not a directory",
        });
    }
    Ok((opened, directory_proof(&stat)?))
}

fn restat_canonical_root(
    parent: &impl AsFd,
    name: &OsStr,
    expected: DirectoryProof,
) -> Result<(), ArchiveError> {
    let stat = stat_entry(parent, name, None, "restat acquired journal root")
        .map_err(acquisition_error)?;
    #[cfg(test)]
    record(AcquisitionPrimitive::FinalRestat);

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
    let canonical = match fs::canonicalize(root) {
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
    #[cfg(test)]
    record(AcquisitionPrimitive::Canonicalize);

    let mut current = open("/", DIRECTORY_FLAGS, Mode::empty())
        .map_err(|error| acquisition_error(source_io("open filesystem root", None, error)))?;
    #[cfg(test)]
    record(AcquisitionPrimitive::FilesystemRootOpen);

    let components = canonical_components(&canonical, root)?;
    let (final_name, ancestors): (OsString, &[OsString]) = match components.split_last() {
        Some((last, ancestors)) => (last.clone(), ancestors),
        None => (OsString::from("."), &[]),
    };
    for component in ancestors {
        let before = stat_entry(&current, component, None, "stat canonical root component")
            .map_err(acquisition_error)?;
        #[cfg(test)]
        record(AcquisitionPrimitive::ComponentStat);
        if !is_directory(&before) {
            return Err(changed(None));
        }
        let opened = open_directory(&current, component, None, true)?;
        #[cfg(test)]
        record(AcquisitionPrimitive::ComponentOpen);
        let after = stat_fd(&opened, None, "stat opened canonical root component")
            .map_err(acquisition_error)?;
        #[cfg(test)]
        record(AcquisitionPrimitive::ComponentFstat);
        if directory_proof(&before)? != directory_proof(&after)? {
            return Err(changed(None));
        }
        current = opened;
    }

    let before_final = stat_entry(&current, &final_name, None, "stat canonical root component")
        .map_err(acquisition_error)?;
    #[cfg(test)]
    record(AcquisitionPrimitive::FinalComponentStat);
    if !is_directory(&before_final) || directory_proof(&before_final)? != expected {
        return Err(changed(None));
    }
    let opened_final = open_directory(&current, &final_name, None, true)?;
    #[cfg(test)]
    record(AcquisitionPrimitive::FinalComponentOpen);
    let after_final = stat_fd(&opened_final, None, "stat opened canonical root component")
        .map_err(acquisition_error)?;
    #[cfg(test)]
    record(AcquisitionPrimitive::FinalComponentFstat);
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
    if file_proof(&after)? != proof.file || file_proof(&after)? != file_proof(&before)? {
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
    let mut current = openat(root, ".", DIRECTORY_FLAGS, Mode::empty())
        .map_err(|error| source_io("open retained journal root", Some(member), error))?;
    for (name, expected) in proof.components.iter().zip(proof.directories.iter()) {
        let before = stat_entry(&current, name, Some(member), "stat inventoried directory")
            .map_err(|error| revalidation_error(error, member))?;
        if !is_directory(&before) || directory_proof(&before)? != *expected {
            return Err(changed(Some(member)));
        }
        let opened = open_directory(&current, name, Some(member), true)?;
        let after = stat_fd(&opened, Some(member), "stat opened inventoried directory")?;
        if directory_proof(&after)? != *expected
            || directory_proof(&after)? != directory_proof(&before)?
        {
            return Err(changed(Some(member)));
        }
        current = opened;
    }
    Ok(current)
}

fn stat_entry(
    parent: &impl AsFd,
    name: &OsStr,
    member: Option<&ArchiveMemberName>,
    operation: &'static str,
) -> Result<FileStat, ArchiveError> {
    fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|error| source_io(operation, member, error))
}

fn stat_fd(
    fd: &impl AsFd,
    member: Option<&ArchiveMemberName>,
    operation: &'static str,
) -> Result<FileStat, ArchiveError> {
    fstat(fd).map_err(|error| source_io(operation, member, error))
}

fn open_directory(
    parent: &impl AsFd,
    name: &OsStr,
    member: Option<&ArchiveMemberName>,
    changed_on_race: bool,
) -> Result<OwnedFd, ArchiveError> {
    openat(parent, name, DIRECTORY_FLAGS, Mode::empty()).map_err(|error| {
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
    openat(parent, name, FILE_FLAGS, Mode::empty()).map_err(|error| {
        if changed_on_race && is_race_error(error) {
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
    fn root_self_uses_final_segment_path_with_no_special_case() {
        let (result, trace) = trace_acquisition(None, || acquire_root(Path::new("/")));
        let (_root, canonical) = result.expect("acquire filesystem root");

        assert_eq!(canonical, PathBuf::from("/"));
        assert_eq!(
            trace
                .iter()
                .filter(|primitive| **primitive == AcquisitionPrimitive::FinalRestat)
                .count(),
            1
        );
        assert!(!trace.iter().any(|primitive| {
            matches!(
                primitive,
                AcquisitionPrimitive::ComponentStat
                    | AcquisitionPrimitive::ComponentOpen
                    | AcquisitionPrimitive::ComponentFstat
            )
        }));
        assert!(trace.ends_with(&[
            AcquisitionPrimitive::FinalComponentStat,
            AcquisitionPrimitive::FinalComponentOpen,
            AcquisitionPrimitive::FinalComponentFstat,
            AcquisitionPrimitive::FinalRestat,
        ]));
    }
}
