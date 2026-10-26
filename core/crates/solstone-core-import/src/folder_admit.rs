// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Copy a selected folder into an unpublished attempt, then rename it into `imports/`.

use std::cell::Cell;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, FileTimes};
#[cfg(not(unix))]
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration as LockDuration, SystemTime};

use chrono::{Duration, NaiveDateTime};
use serde_json::{Value, json};

use crate::snapshot;
use crate::staging::{SourceLocation, classify_source_location};
use crate::timestamp::{Timestamp, validate_timestamp};
use crate::{IMPORT_ID_PROBE_LIMIT, ImportError, hash_source};

/// Bytes requested for one reusable copy buffer.
pub const FOLDER_COPY_BUFFER_BYTES: usize = 65_536;

/// Largest buffer a folder copy may allocate.
pub const FOLDER_COPY_AGGREGATE_BUFFER_BYTES: usize = 65_536;

/// Open file and directory handles a folder copy may hold at once.
pub const FOLDER_COPY_MAX_DESCRIPTORS: usize = 8;

const ATTEMPT_ADAPTER: &str = "folder-copy";
const ATTEMPT_JSON: &[u8] = br#"{"adapter":"folder-copy"}"#;
const DIRECTORY_MODE: u32 = 0o700;

/// Attempt directories currently inside `admit_folder_bounded`. Not a recovery lease.
static LIVE_ATTEMPTS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// A published folder copy. It has no force flag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedFolder {
    pub import_id: Timestamp,
    pub legacy_hash: String,
    pub snapshot: String,
    pub file_count: u64,
    pub byte_count: u64,
}

impl std::fmt::Display for AdmittedFolder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Folder ready to preview.")
    }
}

/// Which folder sources may be imported again while the previous copy is kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReimportSource {
    ObsidianVault,
    GeminiDirectory,
    PdfDirectory,
    SyncObsidian,
    AudioArchive,
    SyncPlaud,
    SyncAudio,
}

/// The re-import answer for one [`ReimportSource`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReimportPolicy {
    Permitted,
    Refused,
}

/// Why a folder copy did not publish.
#[derive(Debug)]
pub enum FolderAdmitError {
    BadAttemptId,
    InsideImports,
    SpecialEntry,
    Escape,
    Duplicate,
    TimestampRequired,
    Cancelled,
    StorageFull,
    Truncated,
    Denied,
    Disappeared,
    DescriptorCap,
    BufferCap,
    ReleaseRefused,
    Publication(String),
}

impl std::fmt::Display for FolderAdmitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::BadAttemptId => "attempt id is not one folder component",
            Self::InsideImports | Self::Denied | Self::DescriptorCap | Self::BufferCap => {
                "The folder could not be read."
            }
            Self::SpecialEntry => "import sources must contain only regular files and folders",
            Self::Escape => "That entry is outside the selected folder.",
            Self::Duplicate => "That folder contains two entries with the same name.",
            Self::TimestampRequired => "A file in this folder has no modified time.",
            Self::Cancelled => "Folder selection cancelled.",
            Self::StorageFull => "Copy stopped because storage is full.",
            Self::Truncated => "Copy stopped because the folder changed during the copy.",
            Self::Disappeared => "The selected folder is no longer available.",
            Self::ReleaseRefused => "that folder attempt cannot be released",
            Self::Publication(message) => message,
        })
    }
}

impl std::error::Error for FolderAdmitError {}

/// Limits passed to the copy. Production admission uses the public constants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CopyLimits {
    pub buffer_bytes: usize,
    pub aggregate_buffer_bytes: usize,
    pub max_descriptors: usize,
}

impl CopyLimits {
    fn production() -> Self {
        Self {
            buffer_bytes: FOLDER_COPY_BUFFER_BYTES,
            aggregate_buffer_bytes: FOLDER_COPY_AGGREGATE_BUFFER_BYTES,
            max_descriptors: FOLDER_COPY_MAX_DESCRIPTORS,
        }
    }
}

/// Allocate one reusable buffer, or refuse when it would exceed the aggregate cap.
pub fn reserve_buffer(requested: usize, aggregate: usize) -> Result<Vec<u8>, FolderAdmitError> {
    if requested > aggregate {
        return Err(FolderAdmitError::BufferCap);
    }
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(requested)
        .map_err(|_| FolderAdmitError::BufferCap)?;
    buffer.resize(requested, 0);
    Ok(buffer)
}

/// Copy `source` into `imports/<timestamp>` under `attempt_id`.
///
/// A copy that fails before custody is written removes that partial attempt.
/// Publication is one rename onto a free id. An occupied id is left in place.
pub fn admit_folder(
    journal: &Path,
    source: &Path,
    attempt_id: &str,
    source_timestamp: &Timestamp,
) -> Result<AdmittedFolder, FolderAdmitError> {
    let cancel = AtomicBool::new(false);
    admit_folder_bounded(
        journal,
        source,
        attempt_id,
        source_timestamp,
        CopyLimits::production(),
        &cancel,
    )
}

pub(crate) fn admit_folder_bounded(
    journal: &Path,
    source: &Path,
    attempt_id: &str,
    source_timestamp: &Timestamp,
    limits: CopyLimits,
    cancel: &AtomicBool,
) -> Result<AdmittedFolder, FolderAdmitError> {
    if !one_component(attempt_id) {
        return Err(FolderAdmitError::BadAttemptId);
    }
    checkpoint(cancel)?;
    let imports = checked_staging_root(journal)?;
    let resolved_source = source.canonicalize().map_err(classify_io)?;
    if imports.starts_with(&resolved_source) {
        return Err(FolderAdmitError::InsideImports);
    }
    let ownership = folder_ownership(journal, true)?;
    #[cfg(not(unix))]
    let _ownership = ownership;
    let live = LiveAttempt::enter(journal, attempt_id);
    if !live.owned {
        return Err(FolderAdmitError::Denied);
    }
    let _live = live;
    match classify_source_location(journal, source) {
        Ok(SourceLocation::AlreadyInImports { .. }) => return Err(FolderAdmitError::InsideImports),
        Ok(SourceLocation::External { .. }) => {}
        Err(ImportError::SourceMissing { .. }) => return Err(FolderAdmitError::Disappeared),
        Err(_) => return Err(FolderAdmitError::Denied),
    }
    let root_meta = fs::symlink_metadata(source).map_err(classify_io)?;
    if root_meta.file_type().is_symlink() || !(root_meta.is_file() || root_meta.is_dir()) {
        return Err(FolderAdmitError::SpecialEntry);
    }
    if !root_meta.is_dir() {
        return Err(FolderAdmitError::Denied);
    }
    checkpoint(cancel)?;
    let mut buffer = reserve_buffer(limits.buffer_bytes, limits.aggregate_buffer_bytes)?;
    let attempt_dir = attempt_directory(journal, attempt_id)?;
    if fs::symlink_metadata(&attempt_dir).is_ok() {
        return Err(FolderAdmitError::Denied);
    }
    let attempts_root = attempt_dir
        .parent()
        .ok_or(FolderAdmitError::Denied)?
        .to_path_buf();
    #[cfg(not(unix))]
    solstone_core_journal_io::create_directory_with_mode(&attempts_root, DIRECTORY_MODE)
        .map_err(|error| classify_text(&error.to_string()))?;
    #[cfg(unix)]
    let attempts = {
        let imports =
            solstone_core_journal_io::FlatDirectory::open(&ownership.root, Path::new("imports"))
                .map_err(|_| FolderAdmitError::Denied)?;
        solstone_core_journal_io::create_or_open_flat_directory_bound(
            &imports,
            OsStr::new("folder-attempts"),
            DIRECTORY_MODE,
            attempts_root.parent().ok_or(FolderAdmitError::Denied)?,
        )
        .map_err(|_| FolderAdmitError::Denied)?
    };
    let claim = claim_path(journal, attempt_id)?;
    let token = mint_claim_token()?;
    #[cfg(unix)]
    let claim_file = attempts
        .create_file(claim.file_name().ok_or(FolderAdmitError::Denied)?, 0o600)
        .map_err(classify_flat);
    #[cfg(not(unix))]
    let claim_file = create_private(&claim);
    let mut claim_file = claim_file.map_err(|error| match error {
        FolderAdmitError::Duplicate => FolderAdmitError::Denied,
        other => other,
    })?;
    let mut cleanup = AttemptCleanup {
        attempt_dir: None,
        claim: Some(claim.clone()),
    };
    claim_file
        .write_all(token.as_bytes())
        .map_err(classify_io)?;
    claim_file.sync_all().map_err(classify_io)?;
    drop(claim_file);
    #[cfg(not(unix))]
    {
        solstone_core_journal_io::create_directory_with_mode(&attempt_dir, DIRECTORY_MODE)
            .map_err(|error| classify_text(&error.to_string()))?;
        cleanup.attempt_dir = Some(attempt_dir.clone());
        write_private(&attempt_dir.join("attempt.json"), ATTEMPT_JSON)?;
    }
    #[cfg(unix)]
    {
        let attempt = attempts
            .create_child(OsStr::new(attempt_id), DIRECTORY_MODE)
            .map_err(classify_flat)?;
        cleanup.attempt_dir = Some(attempt_dir.clone());
        solstone_core_journal_io::write_bytes_exclusive_bound(
            &attempt,
            OsStr::new("attempt.json"),
            ATTEMPT_JSON,
            0o600,
        )
        .map_err(|error| classify_text(&error.to_string()))?;
        drop(attempts);
    }
    let tree = attempt_dir.join("tree");
    let budget = DescriptorBudget {
        // The retained journal root and kernel lock remain live throughout copying.
        live: Cell::new(if cfg!(unix) { 2 } else { 1 }),
        limit: limits.max_descriptors,
    };
    let mut counts = CopyCounts { files: 0, bytes: 0 };
    copy_directory(source, &tree, &budget, &mut buffer, cancel, &mut counts)?;
    drop(buffer);
    checkpoint(cancel)?;
    let legacy_hash = hash_source(&tree)
        .map_err(|_| FolderAdmitError::Denied)?
        .into_inner();
    checkpoint(cancel)?;
    let snapshot = snapshot::fingerprint(&tree).map_err(classify_io)?;
    let custody = json!({
        "adapter": ATTEMPT_ADAPTER,
        "legacy_hash": legacy_hash,
        "snapshot": snapshot,
        "file_count": counts.files,
        "byte_count": counts.bytes,
    });
    // Custody and the no-replace rename are one admitted handoff. Stop before
    // entering it; a later Stop does not undo a handoff that already committed.
    checkpoint(cancel)?;
    #[cfg(not(unix))]
    solstone_core_journal_io::write_bytes_exclusive(
        attempt_dir.join("folder-custody.json"),
        &serde_json::to_vec(&custody).map_err(|_| FolderAdmitError::Denied)?,
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|_| FolderAdmitError::Denied)?;
    #[cfg(unix)]
    {
        ownership
            .root
            .revalidate_canonical_binding()
            .map_err(|_| FolderAdmitError::Denied)?;
        let attempt = solstone_core_journal_io::FlatDirectory::open_normal_relative(
            &ownership.root,
            &PathBuf::from("imports/folder-attempts").join(attempt_id),
        )
        .map_err(|_| FolderAdmitError::Denied)?;
        solstone_core_journal_io::write_bytes_exclusive_bound(
            &attempt,
            OsStr::new("folder-custody.json"),
            &serde_json::to_vec(&custody).map_err(|_| FolderAdmitError::Denied)?,
            0o600,
        )
        .map_err(|error| classify_text(&error.to_string()))?;
    }
    cleanup.attempt_dir = None;
    cleanup.claim = None;
    #[cfg(not(unix))]
    let import_id = publish_attempt(journal, &attempt_dir, source_timestamp)?;
    #[cfg(unix)]
    let import_id = publish_attempt_bound(&ownership.root, attempt_id, source_timestamp)?;
    remove_regular_file(&claim);
    Ok(AdmittedFolder {
        import_id,
        legacy_hash,
        snapshot,
        file_count: counts.files,
        byte_count: counts.bytes,
    })
}

fn checkpoint(cancel: &AtomicBool) -> Result<(), FolderAdmitError> {
    if cancel.load(Ordering::Relaxed) || !crate::work_continues() {
        Err(FolderAdmitError::Cancelled)
    } else {
        Ok(())
    }
}

fn checked_staging_root(journal: &Path) -> Result<PathBuf, FolderAdmitError> {
    let imports = solstone_core_journal_io::contained_path(journal, "imports")
        .map_err(|_| FolderAdmitError::Denied)?;
    // Check each existing ancestor before create_dir_all can traverse it.
    for path in [
        journal.join("imports"),
        journal.join("imports/folder-attempts"),
    ] {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(FolderAdmitError::Denied),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(classify_io(error)),
        }
    }
    Ok(imports)
}

#[cfg(unix)]
struct FolderOwnership {
    root: solstone_core_journal_io::JournalRoot,
    _lock: solstone_core_journal_io::BoundParentLock,
}

#[cfg(unix)]
fn folder_ownership(journal: &Path, create: bool) -> Result<FolderOwnership, FolderAdmitError> {
    use solstone_core_journal_io::{
        FlatDirectory, JournalRoot, acquire_existing_parent_lock_bound_checked,
        create_or_open_flat_directory_bound,
    };
    checked_staging_root(journal)?;
    let root = JournalRoot::open(journal).map_err(|_| FolderAdmitError::Denied)?;
    root.revalidate_canonical_binding()
        .map_err(|_| FolderAdmitError::Denied)?;
    let imports = if create {
        create_or_open_flat_directory_bound(
            &root,
            OsStr::new("imports"),
            DIRECTORY_MODE,
            root.canonical_path(),
        )
    } else {
        FlatDirectory::open(&root, Path::new("imports"))
    }
    .map_err(|_| FolderAdmitError::ReleaseRefused)?;
    let lock = acquire_existing_parent_lock_bound_checked(
        &imports,
        OsStr::new("folder-attempts.lock"),
        LockDuration::ZERO,
        LockDuration::ZERO,
        &|| {
            root.revalidate_canonical_binding()
                .map_err(|_| "folder staging changed".to_owned())
        },
    )
    .map_err(|_| FolderAdmitError::ReleaseRefused)?;
    Ok(FolderOwnership { root, _lock: lock })
}

/// Serialize this adapter's copying and cleanup with the existing kernel lock.
/// This is source ownership, independent of the caller's recovery target use.
#[cfg(not(unix))]
fn folder_ownership(
    journal: &Path,
    create: bool,
) -> Result<solstone_core_journal_io::FileLock, FolderAdmitError> {
    let imports = checked_staging_root(journal)?;
    if create {
        solstone_core_journal_io::create_directory_with_mode(&imports, DIRECTORY_MODE)
            .map_err(|_| FolderAdmitError::Denied)?;
    } else if !imports.is_dir() {
        return Err(FolderAdmitError::ReleaseRefused);
    }
    solstone_core_journal_io::hold_lock_checked(
        imports.join("folder-attempts"),
        solstone_core_journal_io::LockOptions {
            timeout: LockDuration::ZERO,
            ..solstone_core_journal_io::LockOptions::default()
        },
        &|| {
            checked_staging_root(journal)
                .map(|_| ())
                .map_err(|_| "folder staging changed".to_owned())
        },
    )
    .map_err(|_| FolderAdmitError::ReleaseRefused)
}

/// Delete one partial folder-copy attempt that this caller still holds.
///
/// The claim token must match `imports/folder-attempts/<attempt_id>.claim`.
/// Custody, a live copy, and every other path are refused and left in place.
pub fn release_folder_attempt(
    journal: &Path,
    attempt_id: &str,
    token: &str,
) -> Result<(), FolderAdmitError> {
    let _ownership = folder_ownership(journal, false)?;
    let Some(attempt_dir) = releasable_attempt(journal, attempt_id)? else {
        return Err(FolderAdmitError::ReleaseRefused);
    };
    if custody_exists(&attempt_dir) || !folder_copy_marker(&attempt_dir) {
        return Err(FolderAdmitError::ReleaseRefused);
    }
    let claim = claim_path(journal, attempt_id).map_err(|_| FolderAdmitError::ReleaseRefused)?;
    if read_regular_file(&claim).as_deref() != Some(token.as_bytes()) {
        return Err(FolderAdmitError::ReleaseRefused);
    }
    remove_attempt_dir(&attempt_dir)?;
    remove_regular_file(&claim);
    Ok(())
}

/// Delete one abandoned partial attempt after a restart.
///
/// There is no token. A live id, custody, or a marker without its claim file
/// is refused. An admitted import id is not deleted.
pub fn release_abandoned_folder_attempt(
    journal: &Path,
    attempt_id: &str,
) -> Result<(), FolderAdmitError> {
    let _ownership = folder_ownership(journal, false)?;
    let Some(attempt_dir) = releasable_attempt(journal, attempt_id)? else {
        return Err(FolderAdmitError::ReleaseRefused);
    };
    if custody_exists(&attempt_dir) || !folder_copy_marker(&attempt_dir) {
        return Err(FolderAdmitError::ReleaseRefused);
    }
    let claim = claim_path(journal, attempt_id).map_err(|_| FolderAdmitError::ReleaseRefused)?;
    if read_regular_file(&claim).is_none() {
        return Err(FolderAdmitError::ReleaseRefused);
    }
    remove_attempt_dir(&attempt_dir)?;
    remove_regular_file(&claim);
    Ok(())
}

/// Re-check a copied tree against the custody snapshot.
pub fn folder_preview_current(tree: &Path, custody_snapshot: &str) -> Result<(), String> {
    snapshot::verify(tree, custody_snapshot)
}

/// Whether importing this source again is allowed. This does not set force or import.
#[must_use]
pub fn reimport_policy(source: ReimportSource) -> ReimportPolicy {
    match source {
        ReimportSource::ObsidianVault
        | ReimportSource::GeminiDirectory
        | ReimportSource::PdfDirectory
        | ReimportSource::SyncObsidian => ReimportPolicy::Permitted,
        ReimportSource::AudioArchive | ReimportSource::SyncPlaud | ReimportSource::SyncAudio => {
            ReimportPolicy::Refused
        }
    }
}

struct CopyCounts {
    files: u64,
    bytes: u64,
}

struct DescriptorBudget {
    live: Cell<usize>,
    limit: usize,
}

struct DescriptorTicket<'a> {
    budget: &'a DescriptorBudget,
}

impl DescriptorBudget {
    fn take(&self) -> Result<DescriptorTicket<'_>, FolderAdmitError> {
        if self.live.get() >= self.limit {
            return Err(FolderAdmitError::DescriptorCap);
        }
        self.live.set(self.live.get() + 1);
        Ok(DescriptorTicket { budget: self })
    }
}

impl Drop for DescriptorTicket<'_> {
    fn drop(&mut self) {
        self.budget.live.set(self.budget.live.get() - 1);
    }
}

struct AttemptCleanup {
    attempt_dir: Option<PathBuf>,
    claim: Option<PathBuf>,
}

impl Drop for AttemptCleanup {
    fn drop(&mut self) {
        let attempt_dir = self.attempt_dir.take();
        let claim = self.claim.take();
        if let Some(dir) = &attempt_dir
            && custody_exists(dir)
        {
            return;
        }
        if let Some(dir) = attempt_dir
            && let Ok(meta) = fs::symlink_metadata(&dir)
            && meta.is_dir()
            && !meta.file_type().is_symlink()
        {
            let _ = fs::remove_dir_all(dir);
        }
        if let Some(claim) = claim {
            remove_regular_file(&claim);
        }
    }
}

struct LiveAttempt {
    key: String,
    owned: bool,
}

impl LiveAttempt {
    fn enter(journal: &Path, attempt_id: &str) -> Self {
        let key = live_key(journal, attempt_id);
        let owned = live_set().insert(key.clone());
        Self { key, owned }
    }

    fn is_live(journal: &Path, attempt_id: &str) -> bool {
        live_set().contains(&live_key(journal, attempt_id))
    }
}

impl Drop for LiveAttempt {
    fn drop(&mut self) {
        if self.owned {
            live_set().remove(&self.key);
        }
    }
}

fn live_set() -> std::sync::MutexGuard<'static, HashSet<String>> {
    LIVE_ATTEMPTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn live_key(journal: &Path, attempt_id: &str) -> String {
    journal
        .join("imports")
        .join("folder-attempts")
        .join(attempt_id)
        .to_string_lossy()
        .into_owned()
}

fn mint_claim_token() -> Result<String, FolderAdmitError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| FolderAdmitError::Denied)?;
    Ok(hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

#[cfg(unix)]
fn copy_directory(
    source: &Path,
    destination: &Path,
    budget: &DescriptorBudget,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    counts: &mut CopyCounts,
) -> Result<(), FolderAdmitError> {
    use solstone_core_journal_io::{
        FlatDirectory, FlatDirectoryEntry, JournalEntryKind, JournalRoot, list_flat_directory,
    };
    fn denied<T>(_: T) -> FolderAdmitError {
        FolderAdmitError::Denied
    }
    let _source_ticket = budget.take()?;
    let source_root = JournalRoot::open_no_follow(source).map_err(denied)?;
    let parent = destination.parent().ok_or(FolderAdmitError::Denied)?;
    let parent_root = JournalRoot::open_no_follow(parent).map_err(denied)?;
    let parent_directory = FlatDirectory::from_root(&parent_root).map_err(classify_flat)?;
    let created = parent_directory
        .create_child(
            destination.file_name().ok_or(FolderAdmitError::Denied)?,
            DIRECTORY_MODE,
        )
        .map_err(classify_flat)?;
    // Root acquisition temporarily retains three descriptors. Release these
    // two parent handles before opening it, retaining the created identity.
    drop(parent_directory);
    drop(parent_root);
    let _destination_ticket = budget.take()?;
    let destination_root = JournalRoot::open_no_follow(destination).map_err(denied)?;
    if destination_root.identity() != created.identity() {
        return Err(FolderAdmitError::Denied);
    }
    drop(created);
    // Directory handles are reopened from retained roots and dropped before
    // descending. Directory depth does not retain one descriptor per ancestor.
    let mut work: Vec<(PathBuf, Option<FlatDirectoryEntry>, bool)> =
        vec![(PathBuf::new(), None, false)];
    while let Some((relative, expected, finish_directory)) = work.pop() {
        checkpoint(cancel)?;
        let _source_parent = budget.take()?;
        let source_directory =
            FlatDirectory::open_normal_relative(&source_root, &relative).map_err(classify_flat)?;
        if let Some(expected) = &expected
            && !source_directory.matches_entry(expected)
        {
            return Err(FolderAdmitError::Truncated);
        }
        let _destination_parent = budget.take()?;
        let destination_directory =
            FlatDirectory::open_normal_relative(&destination_root, &relative)
                .map_err(classify_flat)?;
        if finish_directory {
            let modified =
                require_modified(source_directory.metadata().map_err(classify_io)?.modified())?;
            destination_directory
                .set_modified(modified)
                .map_err(classify_io)?;
            continue;
        }
        let entries = {
            let _listing = budget.take()?;
            list_flat_directory(&source_directory, usize::MAX)
                .map_err(denied)?
                .ok_or(FolderAdmitError::Denied)?
        };
        work.push((relative.clone(), expected, true));
        for entry in entries {
            checkpoint(cancel)?;
            let name = entry_name(&entry.name)?;
            match entry.kind {
                JournalEntryKind::Directory => {
                    destination_directory
                        .create_child(&entry.name, DIRECTORY_MODE)
                        .map_err(classify_flat)?;
                    work.push((relative.join(name), Some(entry), false));
                }
                JournalEntryKind::RegularFile => {
                    let _input_ticket = budget.take()?;
                    let mut input = source_directory
                        .open_regular_file(&entry)
                        .map_err(classify_flat)?;
                    let modified =
                        require_modified(input.metadata().map_err(classify_io)?.modified())?;
                    let _output_ticket = budget.take()?;
                    let mut output = destination_directory
                        .create_file(&entry.name, 0o600)
                        .map_err(classify_flat)?;
                    let mut copied = 0_u64;
                    loop {
                        checkpoint(cancel)?;
                        let read = input.read(buffer).map_err(classify_io)?;
                        checkpoint(cancel)?;
                        if read == 0 {
                            break;
                        }
                        output.write_all(&buffer[..read]).map_err(classify_io)?;
                        copied += u64::try_from(read).map_err(|_| FolderAdmitError::Denied)?;
                        if copied > entry.size {
                            return Err(FolderAdmitError::Truncated);
                        }
                    }
                    if copied != entry.size {
                        return Err(FolderAdmitError::Truncated);
                    }
                    source_directory
                        .verify_regular_file(&input, &entry)
                        .map_err(|_| FolderAdmitError::Truncated)?;
                    output
                        .set_times(FileTimes::new().set_modified(modified))
                        .map_err(classify_io)?;
                    counts.files += 1;
                    counts.bytes += copied;
                }
                _ => return Err(FolderAdmitError::SpecialEntry),
            }
        }
    }
    source_root.revalidate_canonical_binding().map_err(denied)?;
    destination_root
        .revalidate_canonical_binding()
        .map_err(denied)?;
    Ok(())
}

#[cfg(not(unix))]
fn copy_directory(
    source: &Path,
    destination: &Path,
    budget: &DescriptorBudget,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    counts: &mut CopyCounts,
) -> Result<(), FolderAdmitError> {
    checkpoint(cancel)?;
    let meta = fs::symlink_metadata(source).map_err(classify_io)?;
    if meta.file_type().is_symlink() || !(meta.is_file() || meta.is_dir()) {
        return Err(FolderAdmitError::SpecialEntry);
    }
    if meta.is_file() {
        return copy_file(source, destination, &meta, budget, buffer, cancel, counts);
    }
    let modified = require_modified(meta.modified())?;
    solstone_core_journal_io::create_directory_with_mode(destination, DIRECTORY_MODE)
        .map_err(|error| classify_text(&error.to_string()))?;
    let names = read_names(source, budget)?;
    for name in names {
        let child_name = entry_name(&name)?;
        let child_destination = destination.join(child_name);
        if fs::symlink_metadata(&child_destination).is_ok() {
            return Err(FolderAdmitError::Duplicate);
        }
        copy_directory(
            &source.join(child_name),
            &child_destination,
            budget,
            buffer,
            cancel,
            counts,
        )?;
    }
    {
        let _ticket = budget.take()?;
        let directory = File::open(destination).map_err(classify_io)?;
        directory
            .set_times(FileTimes::new().set_modified(modified))
            .map_err(classify_io)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn copy_file(
    source: &Path,
    destination: &Path,
    meta: &fs::Metadata,
    budget: &DescriptorBudget,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    counts: &mut CopyCounts,
) -> Result<(), FolderAdmitError> {
    let modified = require_modified(meta.modified())?;
    let expected = meta.len();
    let _input = budget.take()?;
    let mut input = File::open(source).map_err(classify_io)?;
    let _output = budget.take()?;
    let mut output = create_private(destination)?;
    let mut copied = 0_u64;
    loop {
        checkpoint(cancel)?;
        let read = input.read(buffer).map_err(classify_io)?;
        if read == 0 {
            break;
        }
        checkpoint(cancel)?;
        output.write_all(&buffer[..read]).map_err(classify_io)?;
        copied += u64::try_from(read).map_err(|_| FolderAdmitError::Denied)?;
    }
    if copied != expected {
        return Err(FolderAdmitError::Truncated);
    }
    output
        .set_times(FileTimes::new().set_modified(modified))
        .map_err(classify_io)?;
    counts.files += 1;
    counts.bytes += copied;
    Ok(())
}

#[cfg(not(unix))]
fn read_names(
    path: &Path,
    budget: &DescriptorBudget,
) -> Result<Vec<std::ffi::OsString>, FolderAdmitError> {
    let _ticket = budget.take()?;
    let entries = fs::read_dir(path).map_err(classify_io)?;
    let mut names = Vec::new();
    for entry in entries {
        names.push(entry.map_err(classify_io)?.file_name());
    }
    names.sort();
    Ok(names)
}

fn entry_name(name: &std::ffi::OsStr) -> Result<&str, FolderAdmitError> {
    let text = name.to_str().ok_or(FolderAdmitError::Denied)?;
    if !one_component(text) {
        return Err(FolderAdmitError::Escape);
    }
    Ok(text)
}

fn require_modified(modified: io::Result<SystemTime>) -> Result<SystemTime, FolderAdmitError> {
    modified.map_err(|_| FolderAdmitError::TimestampRequired)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> Result<File, FolderAdmitError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(FolderAdmitError::Duplicate)
        }
        Err(error) => Err(classify_io(error)),
    }
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), FolderAdmitError> {
    let mut file = create_private(path)?;
    file.write_all(bytes).map_err(classify_io)?;
    Ok(())
}

#[cfg(unix)]
fn classify_flat(error: solstone_core_journal_io::FlatDirectoryError) -> FolderAdmitError {
    match error {
        solstone_core_journal_io::FlatDirectoryError::Io { source, .. } => classify_io(source),
        _ => FolderAdmitError::Denied,
    }
}

fn classify_io(error: io::Error) -> FolderAdmitError {
    let text = error.to_string();
    if error.kind() == io::ErrorKind::StorageFull
        || error.raw_os_error() == Some(28)
        || text.to_ascii_lowercase().contains("no space")
    {
        return FolderAdmitError::StorageFull;
    }
    match error.kind() {
        io::ErrorKind::WriteZero => FolderAdmitError::Truncated,
        io::ErrorKind::PermissionDenied => FolderAdmitError::Denied,
        io::ErrorKind::NotFound => FolderAdmitError::Disappeared,
        _ => FolderAdmitError::Denied,
    }
}

fn classify_text(text: &str) -> FolderAdmitError {
    if text.to_ascii_lowercase().contains("no space") {
        FolderAdmitError::StorageFull
    } else {
        FolderAdmitError::Denied
    }
}

#[cfg(unix)]
fn publish_attempt_bound(
    root: &solstone_core_journal_io::JournalRoot,
    attempt_id: &str,
    source_timestamp: &Timestamp,
) -> Result<Timestamp, FolderAdmitError> {
    use solstone_core_journal_io::FlatDirectory;
    let imports =
        FlatDirectory::open(root, Path::new("imports")).map_err(|_| FolderAdmitError::Denied)?;
    let attempts = FlatDirectory::open(root, Path::new("imports/folder-attempts"))
        .map_err(|_| FolderAdmitError::Denied)?;
    for offset in 0..IMPORT_ID_PROBE_LIMIT {
        let candidate = shift_timestamp(source_timestamp, offset)?;
        match solstone_core_journal_io::media::publish_directory_noreplace_bound(
            &attempts,
            OsStr::new(attempt_id),
            &imports,
            OsStr::new(candidate.as_str()),
        ) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(classify_io(error)),
        }
    }
    Err(FolderAdmitError::Publication(
        "no free import id".to_owned(),
    ))
}

#[cfg(not(unix))]
fn publish_attempt(
    journal: &Path,
    attempt_dir: &Path,
    source_timestamp: &Timestamp,
) -> Result<Timestamp, FolderAdmitError> {
    let imports = journal.join("imports");
    solstone_core_journal_io::create_directory_with_mode(&imports, DIRECTORY_MODE)
        .map_err(|error| classify_text(&error.to_string()))?;
    let from_name = PathBuf::from("folder-attempts").join(
        attempt_dir
            .file_name()
            .ok_or_else(|| FolderAdmitError::Publication("attempt name missing".to_owned()))?,
    );
    for offset in 0..IMPORT_ID_PROBE_LIMIT {
        let candidate = shift_timestamp(source_timestamp, offset)?;
        match solstone_core_journal_io::publish_directory_noreplace(
            &imports,
            from_name.as_os_str(),
            OsStr::new(candidate.as_str()),
        ) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                let text = error.to_string();
                if text.contains('/') || text.contains('\\') {
                    return Err(classify_io(error));
                }
                return Err(FolderAdmitError::Publication(text));
            }
        }
    }
    Err(FolderAdmitError::Publication(
        "no free import id".to_owned(),
    ))
}

fn shift_timestamp(source: &Timestamp, offset_seconds: u32) -> Result<Timestamp, FolderAdmitError> {
    let naive = NaiveDateTime::parse_from_str(source.as_str(), "%Y%m%d_%H%M%S").map_err(|_| {
        FolderAdmitError::Publication(format!("invalid import id {}", source.as_str()))
    })?;
    let shifted = naive + Duration::seconds(i64::from(offset_seconds));
    let raw = shifted.format("%Y%m%d_%H%M%S").to_string();
    validate_timestamp(&raw)
        .map_err(|_| FolderAdmitError::Publication(format!("invalid import id {raw}")))
}

fn claim_path(journal: &Path, attempt_id: &str) -> Result<PathBuf, FolderAdmitError> {
    let attempt_dir = attempt_directory(journal, attempt_id)?;
    let parent = attempt_dir.parent().ok_or(FolderAdmitError::Denied)?;
    Ok(parent.join(format!("{attempt_id}.claim")))
}

fn releasable_attempt(
    journal: &Path,
    attempt_id: &str,
) -> Result<Option<PathBuf>, FolderAdmitError> {
    if !one_component(attempt_id) || LiveAttempt::is_live(journal, attempt_id) {
        return Ok(None);
    }
    let attempt_dir =
        attempt_directory(journal, attempt_id).map_err(|_| FolderAdmitError::ReleaseRefused)?;
    match fs::symlink_metadata(&attempt_dir) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(Some(attempt_dir)),
        _ => Ok(None),
    }
}

fn custody_exists(attempt_dir: &Path) -> bool {
    fs::symlink_metadata(attempt_dir.join("folder-custody.json")).is_ok()
}

fn folder_copy_marker(attempt_dir: &Path) -> bool {
    let bytes = read_regular_file(&attempt_dir.join("attempt.json")).unwrap_or_default();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    is_folder_copy_attempt(&value)
}

fn read_regular_file(path: &Path) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return None;
    }
    fs::read(path).ok()
}

fn remove_regular_file(path: &Path) {
    let Some(meta) = fs::symlink_metadata(path).ok() else {
        return;
    };
    if meta.is_file() && !meta.file_type().is_symlink() {
        let _ = fs::remove_file(path);
    }
}

fn remove_attempt_dir(attempt_dir: &Path) -> Result<(), FolderAdmitError> {
    let meta = fs::symlink_metadata(attempt_dir).map_err(|_| FolderAdmitError::ReleaseRefused)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(FolderAdmitError::ReleaseRefused);
    }
    fs::remove_dir_all(attempt_dir).map_err(|_| FolderAdmitError::ReleaseRefused)
}

fn attempt_directory(journal: &Path, attempt_id: &str) -> Result<PathBuf, FolderAdmitError> {
    if !one_component(attempt_id) {
        return Err(FolderAdmitError::BadAttemptId);
    }
    Ok(journal
        .join("imports")
        .join("folder-attempts")
        .join(attempt_id))
}

fn one_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !Path::new(name).is_absolute()
}

fn is_folder_copy_attempt(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 1 && object.get("adapter").and_then(Value::as_str) == Some(ATTEMPT_ADAPTER)
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs::{self, FileTimes, OpenOptions};
    use std::io;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration as StdDuration, SystemTime};

    use super::*;

    const ORACLE_HASH: &str = "4584c2bd1e437650ec375ae4d773ccb5114465bb0bb6b243bd04b8897c824c50";

    fn stamp() -> Timestamp {
        validate_timestamp("20260616_120000").unwrap()
    }

    fn write_oracle(root: &Path) {
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.md"), b"aaaaaaaa").unwrap();
        fs::write(root.join("sub/b.md"), b"bbbbbbbbbb").unwrap();
        fs::write(root.join("sub/c.bin"), vec![7_u8; 256]).unwrap();
        let modified = SystemTime::UNIX_EPOCH + StdDuration::from_secs(1_700_000_000);
        for path in [
            root.join("a.md"),
            root.join("sub/b.md"),
            root.join("sub/c.bin"),
        ] {
            OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(modified))
                .unwrap();
        }
    }

    fn no_published_custody(journal: &Path) -> bool {
        let imports = journal.join("imports");
        if !imports.exists() {
            return true;
        }
        for entry in fs::read_dir(imports).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name() == "folder-attempts" {
                continue;
            }
            if entry.path().join("folder-custody.json").exists() {
                return false;
            }
        }
        true
    }

    #[test]
    fn reserve_buffer_refuses_a_request_past_the_aggregate() {
        assert!(reserve_buffer(65_537, FOLDER_COPY_AGGREGATE_BUFFER_BYTES).is_err());
        assert_eq!(
            reserve_buffer(FOLDER_COPY_BUFFER_BYTES, FOLDER_COPY_AGGREGATE_BUFFER_BYTES)
                .unwrap()
                .len(),
            FOLDER_COPY_BUFFER_BYTES
        );
    }

    #[cfg(unix)]
    #[test]
    fn reimport_policy_matches_the_source_table_and_admit_has_no_force_flag() {
        assert_eq!(
            reimport_policy(ReimportSource::ObsidianVault),
            ReimportPolicy::Permitted
        );
        assert_eq!(
            reimport_policy(ReimportSource::GeminiDirectory),
            ReimportPolicy::Permitted
        );
        assert_eq!(
            reimport_policy(ReimportSource::PdfDirectory),
            ReimportPolicy::Permitted
        );
        assert_eq!(
            reimport_policy(ReimportSource::SyncObsidian),
            ReimportPolicy::Permitted
        );
        assert_eq!(
            reimport_policy(ReimportSource::AudioArchive),
            ReimportPolicy::Refused
        );
        assert_eq!(
            reimport_policy(ReimportSource::SyncPlaud),
            ReimportPolicy::Refused
        );
        assert_eq!(
            reimport_policy(ReimportSource::SyncAudio),
            ReimportPolicy::Refused
        );
        let journal = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("a.md"), b"aaaaaaaa").unwrap();
        let admitted = admit_folder(journal.path(), source.path(), "once", &stamp()).unwrap();
        assert_eq!(admitted.to_string(), "Folder ready to preview.");
    }

    #[cfg(unix)]
    #[test]
    fn oracle_copy_keeps_the_legacy_hash_and_the_occupied_id() {
        let journal = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        write_oracle(source.path());
        assert_eq!(hash_source(source.path()).unwrap().as_str(), ORACLE_HASH);
        let occupied = journal.path().join("imports").join("20260616_120000");
        fs::create_dir_all(&occupied).unwrap();
        fs::write(occupied.join("marker"), b"keep").unwrap();
        let admitted = admit_folder(journal.path(), source.path(), "oracle", &stamp()).unwrap();
        assert_eq!(admitted.import_id.as_str(), "20260616_120001");
        assert_eq!(admitted.legacy_hash, ORACLE_HASH);
        assert_eq!(admitted.file_count, 3);
        assert_eq!(admitted.byte_count, 8 + 10 + 256);
        assert_eq!(fs::read(occupied.join("marker")).unwrap(), b"keep");
        let published = journal
            .path()
            .join("imports")
            .join(admitted.import_id.as_str());
        assert_eq!(
            fs::read(published.join("attempt.json")).unwrap(),
            ATTEMPT_JSON
        );
        let custody: serde_json::Map<String, Value> =
            serde_json::from_slice(&fs::read(published.join("folder-custody.json")).unwrap())
                .unwrap();
        assert_eq!(
            custody
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "adapter",
                "byte_count",
                "file_count",
                "legacy_hash",
                "snapshot"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        assert_eq!(
            custody.get("legacy_hash").and_then(Value::as_str),
            Some(ORACLE_HASH)
        );
        let previous = fs::read(published.join("tree").join("sub").join("c.bin")).unwrap();
        let mut changed = fs::read(source.path().join("sub").join("c.bin")).unwrap();
        changed[0] = 8;
        fs::write(source.path().join("sub").join("c.bin"), &changed).unwrap();
        let again = admit_folder(journal.path(), source.path(), "oracle-2", &stamp()).unwrap();
        assert_eq!(again.legacy_hash, ORACLE_HASH);
        assert_ne!(again.snapshot, admitted.snapshot);
        assert_eq!(
            fs::read(published.join("tree").join("sub").join("c.bin")).unwrap(),
            previous
        );
        let tree = journal
            .path()
            .join("imports")
            .join(again.import_id.as_str())
            .join("tree");
        folder_preview_current(&tree, &again.snapshot).unwrap();
        let mut published_bytes = fs::read(tree.join("a.md")).unwrap();
        published_bytes[0] = b'z';
        fs::write(tree.join("a.md"), &published_bytes).unwrap();
        let preview = folder_preview_current(&tree, &again.snapshot).unwrap_err();
        assert_eq!(
            preview,
            "the source changed after preview; preview it again before importing"
        );
        let modified = fs::metadata(published.join("tree").join("a.md"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            1_700_000_000
        );
    }

    fn plant_attempt(
        journal: &Path,
        attempt_id: &str,
        token: Option<&str>,
        custody: bool,
    ) -> PathBuf {
        let attempt = journal.join("imports/folder-attempts").join(attempt_id);
        fs::create_dir_all(attempt.join("tree")).unwrap();
        fs::write(attempt.join("tree/a.md"), b"partial").unwrap();
        fs::write(attempt.join("attempt.json"), ATTEMPT_JSON).unwrap();
        if custody {
            fs::write(
                attempt.join("folder-custody.json"),
                br#"{"adapter":"folder-copy","legacy_hash":"abc","snapshot":"def","file_count":1,"byte_count":7}"#,
            )
            .unwrap();
        }
        if let Some(token) = token {
            fs::write(
                journal
                    .join("imports/folder-attempts")
                    .join(format!("{attempt_id}.claim")),
                token,
            )
            .unwrap();
        }
        attempt
    }

    #[test]
    fn second_guard_does_not_clear_a_live_attempt() {
        let journal = tempfile::tempdir().unwrap();
        let token = "0123456789abcdef0123456789abcdef";
        let attempt = plant_attempt(journal.path(), "blocked", Some(token), false);
        let claim = journal.path().join("imports/folder-attempts/blocked.claim");
        let tree = fs::read(attempt.join("tree/a.md")).unwrap();
        let claim_bytes = fs::read(&claim).unwrap();
        let first = LiveAttempt::enter(journal.path(), "blocked");
        assert!(first.owned);
        let second = LiveAttempt::enter(journal.path(), "blocked");
        assert!(!second.owned);
        drop(second);
        assert!(LiveAttempt::is_live(journal.path(), "blocked"));
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("a.md"), b"aaaaaaaa").unwrap();
        let refused = admit_folder(journal.path(), source.path(), "blocked", &stamp()).unwrap_err();
        assert!(matches!(refused, FolderAdmitError::Denied));
        assert!(release_folder_attempt(journal.path(), "blocked", token).is_err());
        assert_eq!(fs::read(attempt.join("tree/a.md")).unwrap(), tree);
        assert_eq!(fs::read(&claim).unwrap(), claim_bytes);
        drop(first);
    }

    #[test]
    fn empty_import_id_and_sibling_marker_stay() {
        let journal = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("a.md"), b"aaaaaaaa").unwrap();
        let occupied = journal.path().join("imports").join(stamp().as_str());
        fs::create_dir_all(&occupied).unwrap();
        let sibling = journal.path().join("imports").join("sibling-marker");
        fs::write(&sibling, b"stay").unwrap();
        let admitted = admit_folder(journal.path(), source.path(), "next", &stamp()).unwrap();
        assert_ne!(admitted.import_id.as_str(), stamp().as_str());
        assert!(fs::read_dir(&occupied).unwrap().next().is_none());
        assert_eq!(fs::read(&sibling).unwrap(), b"stay");
        assert!(
            journal
                .path()
                .join("imports")
                .join(admitted.import_id.as_str())
                .join("tree/a.md")
                .is_file()
        );
    }

    #[test]
    fn symlink_failure_publishes_nothing_and_release_removes_only_an_attempt() {
        let journal = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("a.md"), b"aaaaaaaa").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("a.md", source.path().join("link")).unwrap();
            let error =
                admit_folder(journal.path(), source.path(), "linked", &stamp()).unwrap_err();
            assert!(matches!(error, FolderAdmitError::SpecialEntry));
            assert_eq!(
                error.to_string(),
                "import sources must contain only regular files and folders"
            );
            assert!(no_published_custody(journal.path()));
            assert!(
                !journal
                    .path()
                    .join("imports/folder-attempts/linked")
                    .exists()
            );
            assert!(
                !journal
                    .path()
                    .join("imports/folder-attempts/linked.claim")
                    .exists()
            );
            fs::remove_file(source.path().join("link")).unwrap();
        }

        let published = admit_folder(journal.path(), source.path(), "done", &stamp()).unwrap();
        let published_dir = journal
            .path()
            .join("imports")
            .join(published.import_id.as_str());
        let published_bytes = fs::read(published_dir.join("tree/a.md")).unwrap();
        assert_eq!(
            fs::read(published_dir.join("attempt.json")).unwrap(),
            ATTEMPT_JSON
        );
        assert!(
            !journal
                .path()
                .join("imports/folder-attempts/done.claim")
                .exists()
        );
        let release_published = release_folder_attempt(
            journal.path(),
            published.import_id.as_str(),
            "0123456789abcdef0123456789abcdef",
        );
        assert!(release_published.is_err());
        assert!(
            release_abandoned_folder_attempt(journal.path(), published.import_id.as_str()).is_err()
        );
        assert_eq!(
            fs::read(published_dir.join("tree/a.md")).unwrap(),
            published_bytes
        );

        let token = "0123456789abcdef0123456789abcdef";
        let sibling = journal.path().join("imports/folder-attempts/sibling.txt");
        fs::write(&sibling, b"stay").unwrap();
        let held = plant_attempt(journal.path(), "held", Some(token), true);
        let custody = fs::read(held.join("folder-custody.json")).unwrap();
        let refused = release_folder_attempt(journal.path(), "held", token).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "that folder attempt cannot be released"
        );
        assert!(!refused.to_string().contains(token));
        assert!(release_abandoned_folder_attempt(journal.path(), "held").is_err());
        assert_eq!(fs::read(held.join("folder-custody.json")).unwrap(), custody);
        assert_eq!(fs::read(&sibling).unwrap(), b"stay");

        let wrong = plant_attempt(journal.path(), "wrong", Some(token), false);
        let wrong_bytes = fs::read(wrong.join("tree/a.md")).unwrap();
        let mismatch = release_folder_attempt(journal.path(), "wrong", "wrong-token").unwrap_err();
        assert!(!mismatch.to_string().contains(token));
        assert_eq!(fs::read(wrong.join("tree/a.md")).unwrap(), wrong_bytes);
        let live = LiveAttempt::enter(journal.path(), "wrong");
        assert!(release_folder_attempt(journal.path(), "wrong", token).is_err());
        assert!(release_abandoned_folder_attempt(journal.path(), "wrong").is_err());
        assert_eq!(fs::read(wrong.join("tree/a.md")).unwrap(), wrong_bytes);
        drop(live);
        release_folder_attempt(journal.path(), "wrong", token).unwrap();
        assert!(!wrong.exists());
        assert!(
            !journal
                .path()
                .join("imports/folder-attempts/wrong.claim")
                .exists()
        );
        assert_eq!(fs::read(&sibling).unwrap(), b"stay");

        let restart = plant_attempt(journal.path(), "restart", Some(token), false);
        release_abandoned_folder_attempt(journal.path(), "restart").unwrap();
        assert!(!restart.exists());
        assert!(
            !journal
                .path()
                .join("imports/folder-attempts/restart.claim")
                .exists()
        );
        let marker_only = plant_attempt(journal.path(), "marker", None, false);
        assert!(release_abandoned_folder_attempt(journal.path(), "marker").is_err());
        assert_eq!(fs::read(marker_only.join("tree/a.md")).unwrap(), b"partial");

        for offset in 0..IMPORT_ID_PROBE_LIMIT {
            let id = shift_timestamp(&stamp(), offset).unwrap();
            let dir = journal.path().join("imports").join(id.as_str());
            fs::create_dir_all(&dir).unwrap();
            let marker = dir.join("marker");
            if !marker.exists() {
                fs::write(&marker, id.as_str()).unwrap();
            }
        }
        let blocked = admit_folder(journal.path(), source.path(), "blocked", &stamp()).unwrap_err();
        assert!(matches!(blocked, FolderAdmitError::Publication(_)));
        assert_eq!(blocked.to_string(), "no free import id");
        let blocked_dir = journal.path().join("imports/folder-attempts/blocked");
        assert_eq!(
            fs::read(blocked_dir.join("tree/a.md")).unwrap(),
            b"aaaaaaaa"
        );
        assert_eq!(
            fs::read(blocked_dir.join("attempt.json")).unwrap(),
            ATTEMPT_JSON
        );
        assert!(blocked_dir.join("folder-custody.json").is_file());
        let claim = fs::read(journal.path().join("imports/folder-attempts/blocked.claim")).unwrap();
        assert!(
            !fs::read(blocked_dir.join("attempt.json"))
                .unwrap()
                .windows(claim.len())
                .any(|window| window == claim)
        );
        assert!(
            !fs::read(blocked_dir.join("folder-custody.json"))
                .unwrap()
                .windows(claim.len())
                .any(|window| window == claim)
        );
        assert_eq!(
            fs::read(published_dir.join("tree/a.md")).unwrap(),
            published_bytes
        );
        for offset in 0..IMPORT_ID_PROBE_LIMIT {
            let id = shift_timestamp(&stamp(), offset).unwrap();
            let marker = journal
                .path()
                .join("imports")
                .join(id.as_str())
                .join("marker");
            if marker.is_file() {
                assert!(!fs::read(&marker).unwrap().is_empty());
            } else {
                assert!(
                    journal
                        .path()
                        .join("imports")
                        .join(id.as_str())
                        .join("tree/a.md")
                        .is_file()
                );
            }
        }
        assert_eq!(fs::read(&sibling).unwrap(), b"stay");
    }

    #[test]
    fn inside_imports_cancel_and_a_tight_descriptor_cap_publish_nothing() {
        let journal = tempfile::tempdir().unwrap();
        let inside = journal.path().join("imports/already");
        fs::create_dir_all(&inside).unwrap();
        fs::write(inside.join("a.md"), b"aaaaaaaa").unwrap();
        let inside_error = admit_folder(journal.path(), &inside, "inside", &stamp()).unwrap_err();
        assert!(matches!(inside_error, FolderAdmitError::InsideImports));
        assert_eq!(inside_error.to_string(), "The folder could not be read.");
        assert!(
            !journal
                .path()
                .join("imports/folder-attempts/inside")
                .exists()
        );

        let source = tempfile::tempdir().unwrap();
        fs::write(
            source.path().join("a.md"),
            vec![1_u8; FOLDER_COPY_BUFFER_BYTES + 3],
        )
        .unwrap();
        let cancel = AtomicBool::new(true);
        let cancelled = admit_folder_bounded(
            journal.path(),
            source.path(),
            "cancelled",
            &stamp(),
            CopyLimits::production(),
            &cancel,
        )
        .unwrap_err();
        assert!(matches!(cancelled, FolderAdmitError::Cancelled));
        assert_eq!(cancelled.to_string(), "Folder selection cancelled.");
        assert!(
            !journal
                .path()
                .join("imports/folder-attempts/cancelled")
                .exists()
        );

        let capped = admit_folder_bounded(
            journal.path(),
            source.path(),
            "capped",
            &stamp(),
            CopyLimits {
                buffer_bytes: FOLDER_COPY_BUFFER_BYTES,
                aggregate_buffer_bytes: FOLDER_COPY_AGGREGATE_BUFFER_BYTES,
                max_descriptors: 1,
            },
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(capped, FolderAdmitError::DescriptorCap));
        assert!(no_published_custody(journal.path()));
        assert!(
            !journal
                .path()
                .join("imports/folder-attempts/capped")
                .exists()
        );

        let copied = admit_folder(journal.path(), source.path(), "wide", &stamp()).unwrap();
        let bytes = fs::read(
            journal
                .path()
                .join("imports")
                .join(copied.import_id.as_str())
                .join("tree")
                .join("a.md"),
        )
        .unwrap();
        assert_eq!(bytes.len(), FOLDER_COPY_BUFFER_BYTES + 3);
        assert_eq!(bytes[FOLDER_COPY_BUFFER_BYTES], 1);
        assert_eq!(
            require_modified(Err(io::Error::other("missing")))
                .unwrap_err()
                .to_string(),
            "A file in this folder has no modified time."
        );
        assert_eq!(
            entry_name(std::ffi::OsStr::new(".."))
                .unwrap_err()
                .to_string(),
            "That entry is outside the selected folder."
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_imports_directory_is_path_free_and_unpublished() {
        let journal = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let prior = target.path().join("prior.txt");
        fs::write(&prior, b"keep").unwrap();
        std::os::unix::fs::symlink(target.path(), journal.path().join("imports")).unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("a.md"), b"aaaaaaaa").unwrap();
        let error =
            admit_folder(journal.path(), source.path(), "linked-imports", &stamp()).unwrap_err();
        let shown = error.to_string();
        assert_eq!(shown, "The folder could not be read.");
        assert!(!shown.contains(journal.path().to_str().unwrap()));
        assert_eq!(fs::read(&prior).unwrap(), b"keep");
        let mut names = fs::read_dir(target.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec![std::ffi::OsString::from("prior.txt")]);
    }

    #[test]
    fn ancestor_source_is_refused_before_creating_staging() {
        let selected = tempfile::tempdir().unwrap();
        fs::write(selected.path().join("0-note.md"), b"keep").unwrap();
        let journal = selected.path().join("journal");
        fs::create_dir(&journal).unwrap();
        // Bound the pre-fix fixture at its first file, before it can recurse
        // into the staging tree it has just created inside the source.
        let error = admit_folder_bounded(
            &journal,
            selected.path(),
            "ancestor",
            &stamp(),
            CopyLimits {
                max_descriptors: 1,
                ..CopyLimits::production()
            },
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(error, FolderAdmitError::InsideImports));
        assert!(!journal.join("imports").exists());
        assert_eq!(
            fs::read(selected.path().join("0-note.md")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn installed_stop_refuses_folder_admission_before_any_write() {
        let journal = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("note.md"), b"keep").unwrap();
        let stop = crate::WorkStop::new();
        stop.request();
        let _work = crate::WorkStopGuard::install(&stop);
        let error = admit_folder(journal.path(), source.path(), "stopped", &stamp()).unwrap_err();
        assert!(matches!(error, FolderAdmitError::Cancelled));
        assert!(!journal.path().join("imports").exists());
        assert_eq!(fs::read(source.path().join("note.md")).unwrap(), b"keep");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn unavailable_no_replace_keeps_custody_and_the_occupied_import() {
        let journal = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("note.md"), b"selected source").unwrap();
        let stamp = validate_timestamp("20260616_120000").unwrap();
        let prior = journal.path().join("imports").join(stamp.as_str());
        fs::create_dir_all(&prior).unwrap();
        fs::write(prior.join("prior"), b"preserve").unwrap();
        assert!(admit_folder(journal.path(), source.path(), "unsupported", &stamp).is_err());
        assert_eq!(fs::read(prior.join("prior")).unwrap(), b"preserve");
        let attempt = journal.path().join("imports/folder-attempts/unsupported");
        assert_eq!(
            fs::read(attempt.join("tree/note.md")).unwrap(),
            b"selected source"
        );
        assert!(attempt.join("folder-custody.json").is_file());
        assert!(release_abandoned_folder_attempt(journal.path(), "unsupported").is_err());
    }
}
