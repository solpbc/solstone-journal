// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local whole-journal restore obligation.
//!
//! The record is runtime state for one retained target. It is not journal
//! content: backups omit it, and restore refuses to overwrite or delete it.
//! A missing file is ordinary startup. An unreadable, malformed, or
//! substituted file stays pending and is not cleared from this reader.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
#[cfg(not(unix))]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic::{JsonWriteOptions, serialize_json_bytes};
#[cfg(unix)]
use crate::flat_directory::FlatDirectory;
use crate::journal_root::{JournalRoot, JournalRootError};

/// Journal-relative path of the one whole-journal restore obligation.
pub const WHOLE_JOURNAL_RESTORE_FILE: &str = "health/whole-journal-restore.json";

/// Owner-facing refusal when a content command reaches a journal that is still restoring.
pub const WHOLE_JOURNAL_RESTORE_NOT_READY: &str = "this journal isn't ready to use.";

const RECORD_VERSION: u64 = 1;
const MAX_OBLIGATION_BYTES: u64 = 8192;
const OBLIGATION_LOCK: &str = "whole-journal-restore.lock";
const OBLIGATION_NAME: &str = "whole-journal-restore.json";

/// Whether ordinary owner use may proceed.
///
/// `Open` means the obligation file is absent. `Paused` is a transient hold
/// while recovery is pending or the record cannot be trusted. It is not Stop,
/// not cancellation, and not a closed root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WholeJournalRestoreAdmission {
    Open,
    Paused,
}

/// Closed result of the readiness reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WholeJournalRestoreReadiness {
    Absent,
    Pending(Box<WholeJournalRestore>),
    Unavailable,
}

/// Source identity bound to the local obligation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum WholeJournalRestoreSource {
    Hosted {
        account_id: String,
        instance_id: String,
        bucket: String,
        prefix: String,
    },
    Byo {
        repository: String,
        backend: String,
    },
}

/// One published obligation. `root` is the canonical journal path at publication.
/// `attempt` identifies the writer that may retire this record. `root_identity`
/// is the admitted journal's observation, not a path spelling.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WholeJournalRestore {
    pub v: u64,
    pub attempt: u64,
    pub root: String,
    pub root_identity: String,
    pub source: WholeJournalRestoreSource,
    pub snapshot_id: String,
    pub subtree: String,
}

/// Failure to publish or retire an obligation. The reader never returns this.
#[derive(Debug)]
pub enum WholeJournalRestoreWriteError {
    Invalid(&'static str),
    Root(String),
    Io(String),
}

impl std::fmt::Display for WholeJournalRestoreWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => {
                write!(
                    formatter,
                    "whole-journal restore obligation refused: {reason}"
                )
            }
            Self::Root(reason) => {
                write!(
                    formatter,
                    "whole-journal restore obligation root refused: {reason}"
                )
            }
            Self::Io(reason) => {
                write!(
                    formatter,
                    "whole-journal restore obligation publish refused: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for WholeJournalRestoreWriteError {}

/// Read the obligation. This function does not create, repair, or delete it.
pub fn read_whole_journal_restore(journal: &Path) -> WholeJournalRestoreReadiness {
    let bytes = match marker_bytes(journal) {
        Marker::Absent => return WholeJournalRestoreReadiness::Absent,
        Marker::Unavailable => return WholeJournalRestoreReadiness::Unavailable,
        Marker::Bytes(bytes) => bytes,
    };
    let root = match JournalRoot::open(journal) {
        Ok(root) => root,
        Err(error) => {
            log::warn!("whole-journal restore obligation root is unavailable: {error}");
            return WholeJournalRestoreReadiness::Unavailable;
        }
    };
    if let Err(error) = root.revalidate_canonical_binding() {
        log::warn!("whole-journal restore obligation root changed: {error}");
        return WholeJournalRestoreReadiness::Unavailable;
    }
    match interpret(&root, &bytes) {
        Ok(record) => WholeJournalRestoreReadiness::Pending(Box::new(record)),
        Err(reason) => {
            log::warn!("whole-journal restore obligation is not usable: {reason}");
            WholeJournalRestoreReadiness::Unavailable
        }
    }
}

/// `Open` only when the obligation is absent. Every other read is `Paused`.
pub fn whole_journal_restore_admission(journal: &Path) -> WholeJournalRestoreAdmission {
    match read_whole_journal_restore(journal) {
        WholeJournalRestoreReadiness::Absent => WholeJournalRestoreAdmission::Open,
        WholeJournalRestoreReadiness::Pending(_) | WholeJournalRestoreReadiness::Unavailable => {
            WholeJournalRestoreAdmission::Paused
        }
    }
}

/// Publish the obligation for the journal just opened. The stored root is that
/// journal's canonical path and admitted identity. An existing record, including
/// a malformed or substituted one, is left untouched.
pub fn publish_whole_journal_restore(
    journal: &Path,
    source: &WholeJournalRestoreSource,
    snapshot_id: &str,
    subtree: &str,
) -> Result<WholeJournalRestore, WholeJournalRestoreWriteError> {
    validate_source(source).map_err(WholeJournalRestoreWriteError::Invalid)?;
    validate_snapshot_id(snapshot_id).map_err(WholeJournalRestoreWriteError::Invalid)?;
    validate_subtree(subtree).map_err(WholeJournalRestoreWriteError::Invalid)?;
    let root = open_root(journal)?;
    require_absent(root.canonical_path())?;
    let record = bound_record(&root, next_attempt(), source.clone(), snapshot_id, subtree)?;
    let bytes = canonical_bytes(&record)?;
    let health = prepare_health_dir(&root, true)?;
    with_obligation_lock(&root, &health, || {
        root.revalidate_canonical_binding().map_err(root_error)?;
        require_absent(root.canonical_path())?;
        let checkpoint = || {
            revalidate_health(&root, &health)
                .map_err(|error| error.to_string())
                .and_then(|_| match marker_bytes(root.canonical_path()) {
                    Marker::Absent => Ok(()),
                    Marker::Bytes(_) | Marker::Unavailable => {
                        Err("obligation already exists".to_owned())
                    }
                })
        };
        publish_bound_record(&root, &health, &bytes, &checkpoint)?;
        bump_generation(journal);
        Ok(record)
    })
}

/// After the previous writer has drained, bind a new attempt to the same
/// selection. The previous attempt can no longer retire the record.
pub fn adopt_whole_journal_restore(
    journal: &Path,
    expected: &WholeJournalRestore,
) -> Result<WholeJournalRestore, WholeJournalRestoreWriteError> {
    let root = open_root(journal)?;
    require_same_binding(&root, expected)?;
    let health = prepare_health_dir(&root, false)?;
    let expected_bytes = canonical_bytes(expected)?;
    with_obligation_lock(&root, &health, || {
        root.revalidate_canonical_binding().map_err(root_error)?;
        require_same_binding(&root, expected)?;
        match marker_bytes(root.canonical_path()) {
            Marker::Bytes(bytes) if bytes == expected_bytes => {}
            Marker::Absent => {
                return Err(WholeJournalRestoreWriteError::Invalid(
                    "obligation disappeared before adoption",
                ));
            }
            Marker::Bytes(_) | Marker::Unavailable => {
                return Err(WholeJournalRestoreWriteError::Invalid(
                    "obligation changed before adoption",
                ));
            }
        }
        let record = bound_record(
            &root,
            next_attempt(),
            expected.source.clone(),
            &expected.snapshot_id,
            &expected.subtree,
        )?;
        let bytes = canonical_bytes(&record)?;
        let checkpoint = || {
            revalidate_health(&root, &health)
                .map_err(|error| error.to_string())
                .and_then(|_| match marker_bytes(root.canonical_path()) {
                    Marker::Bytes(current) if current == expected_bytes => Ok(()),
                    _ => Err("obligation changed before adoption".to_owned()),
                })
        };
        publish_bound_record(&root, &health, &bytes, &checkpoint)?;
        bump_generation(journal);
        Ok(record)
    })
}

/// Remove the obligation only when the current bytes are still this attempt.
/// A mismatch, a root change, or a substituted file leaves the record in place.
pub fn clear_whole_journal_restore(
    journal: &Path,
    expected: &WholeJournalRestore,
) -> Result<(), WholeJournalRestoreWriteError> {
    let root = Arc::new(open_root(journal)?);
    require_same_binding(&root, expected)?;
    match marker_bytes(root.canonical_path()) {
        Marker::Absent => return Ok(()),
        Marker::Unavailable => {
            return Err(WholeJournalRestoreWriteError::Invalid(
                "obligation is not a regular file",
            ));
        }
        Marker::Bytes(_) => {}
    }
    let health = prepare_health_dir(&root, false)?;
    let expected_bytes = canonical_bytes(expected)?;
    let removed = with_obligation_lock(&root, &health, || {
        root.revalidate_canonical_binding().map_err(root_error)?;
        require_same_binding(&root, expected)?;
        retire_matching_file(&root, &health, &expected_bytes)
    })?;
    if removed {
        bump_generation(journal);
    }
    Ok(())
}

/// Whether the on-disk bytes are still the canonical form of `expected`.
/// This function does not write.
pub fn whole_journal_restore_matches(journal: &Path, expected: &WholeJournalRestore) -> bool {
    let Ok(root) = JournalRoot::open(journal) else {
        return false;
    };
    if root.revalidate_canonical_binding().is_err() {
        return false;
    }
    if require_same_binding(&root, expected).is_err() {
        return false;
    }
    let Ok(expected_bytes) = canonical_bytes(expected) else {
        return false;
    };
    matches!(marker_bytes(root.canonical_path()), Marker::Bytes(bytes) if bytes == expected_bytes)
}

/// Process-local change count for this journal. It advances when an obligation
/// is published, adopted, or removed. It is not a persisted ready flag.
pub fn whole_journal_restore_generation(journal: &Path) -> u64 {
    let (generations, _) = target_generations();
    lock_map(generations)
        .get(&hold_key(journal))
        .copied()
        .unwrap_or(0)
}

/// Return once the generation is no longer `observed`. A later publisher,
/// adopter, or clearer wakes the waiter without a bus finish.
pub fn wait_whole_journal_restore_generation(journal: &Path, observed: u64) {
    let key = hold_key(journal);
    let (generations, wake) = target_generations();
    let mut generations = lock_map(generations);
    while generations.get(&key).copied().unwrap_or(0) == observed {
        generations = wake
            .wait(generations)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

fn held_roots() -> &'static Mutex<BTreeSet<PathBuf>> {
    static HELD: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
    &HELD
}

fn target_flights() -> &'static (Mutex<BTreeMap<PathBuf, usize>>, Condvar) {
    static FLIGHTS: (Mutex<BTreeMap<PathBuf, usize>>, Condvar) =
        (Mutex::new(BTreeMap::new()), Condvar::new());
    &FLIGHTS
}

fn lock_map<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn hold_key(journal: &Path) -> PathBuf {
    journal
        .canonicalize()
        .unwrap_or_else(|_| journal.to_path_buf())
}

/// Mark the target so a new publication that races the restore observes the hold
/// even before it re-reads the obligation file.
pub fn hold_target_for_restore(journal: &Path) {
    lock_map(held_roots()).insert(hold_key(journal));
}

/// Drop the in-process hold. This does not remove the obligation file.
pub fn release_target_for_restore(journal: &Path) {
    lock_map(held_roots()).remove(&hold_key(journal));
}

/// Whether this process is inside a restore drain or copy for `journal`.
pub fn target_held_for_restore(journal: &Path) -> bool {
    lock_map(held_roots()).contains(&hold_key(journal))
}

/// `true` when ordinary use and publication must not proceed.
///
/// Absent obligation and no in-process hold is ordinary startup. Pending,
/// unreadable, malformed, substituted, or held state blocks use.
pub fn whole_journal_restore_blocks(journal: &Path) -> bool {
    whole_journal_restore_admission(journal) != WholeJournalRestoreAdmission::Open
        || target_held_for_restore(journal)
}

/// One in-flight use of the target. Dropping it releases the count.
pub struct TargetUse {
    key: PathBuf,
    _root: Arc<JournalRoot>,
}

impl Drop for TargetUse {
    fn drop(&mut self) {
        let (flights, wake) = target_flights();
        let mut flights = lock_map(flights);
        if let Some(count) = flights.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                flights.remove(&self.key);
            }
        }
        wake.notify_all();
    }
}

/// Ordinary use was refused because recovery is pending or the target is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetUseRefused;

impl std::fmt::Display for TargetUseRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("whole-journal restore is pending")
    }
}

impl std::error::Error for TargetUseRefused {}

/// Admit one publication. The count is taken before the hold and obligation are
/// checked again, so a drain that starts during the check waits for this use
/// or observes the refusal.
pub fn begin_target_use(journal: &Path) -> Result<TargetUse, TargetUseRefused> {
    let root = Arc::new(JournalRoot::open(journal).map_err(|_| TargetUseRefused)?);
    begin_target_use_bound(root)
}

/// Admit use of the retained root, retaining its identity until the work joins.
pub fn begin_target_use_bound(root: Arc<JournalRoot>) -> Result<TargetUse, TargetUseRefused> {
    root.revalidate_canonical_binding()
        .map_err(|_| TargetUseRefused)?;
    let key = root.canonical_path().to_owned();
    {
        let (flights, _) = target_flights();
        *lock_map(flights).entry(key.clone()).or_insert(0) += 1;
    }
    let guard = TargetUse {
        key,
        _root: Arc::clone(&root),
    };
    if whole_journal_restore_blocks(root.canonical_path())
        || root.revalidate_canonical_binding().is_err()
    {
        drop(guard);
        return Err(TargetUseRefused);
    }
    Ok(guard)
}

/// Block until every `begin_target_use` guard for this journal has been dropped.
pub fn wait_for_target_use_to_end(journal: &Path) {
    let key = hold_key(journal);
    let (flights, wake) = target_flights();
    let mut flights = lock_map(flights);
    while flights.get(&key).copied().unwrap_or(0) > 0 {
        flights = wake
            .wait(flights)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

#[cfg(any(target_os = "android", test))]
pub(crate) fn restore_target_quiescent(journal: &Path) -> bool {
    target_held_for_restore(journal)
        && lock_map(&target_flights().0)
            .get(&hold_key(journal))
            .copied()
            .unwrap_or(0)
            == 0
}

fn interpret(root: &JournalRoot, bytes: &[u8]) -> Result<WholeJournalRestore, &'static str> {
    let record: WholeJournalRestore =
        serde_json::from_slice(bytes).map_err(|_| "obligation is malformed")?;
    if record.v != RECORD_VERSION {
        return Err("obligation version is not current");
    }
    if record.attempt == 0 {
        return Err("obligation attempt is missing");
    }
    let canonical = canonical_root(root).map_err(|_| "journal path is not unicode")?;
    if record.root != canonical {
        return Err("obligation is bound to a different journal");
    }
    if record.root_identity != root.identity().observation_id() {
        return Err("obligation is bound to a different journal identity");
    }
    validate_source(&record.source)?;
    validate_snapshot_id(&record.snapshot_id)?;
    validate_subtree(&record.subtree)?;
    Ok(record)
}

enum Marker {
    Absent,
    Unavailable,
    Bytes(Vec<u8>),
}

#[cfg(unix)]
fn marker_bytes(journal: &Path) -> Marker {
    use std::ffi::OsStr;

    use crate::flat_directory::{
        list_flat_directory, open_flat_directory_bound, read_observed_file_bounded,
    };

    if let Some(marker) = parent_marker(journal) {
        return marker;
    }
    let root = match open_root(journal) {
        Ok(root) => root,
        Err(_) => return Marker::Unavailable,
    };
    let health_name = OsStr::new("health");
    let health = match open_flat_directory_bound(&root, health_name, root.canonical_path()) {
        Ok(Some(health)) => health,
        Ok(None) if root.revalidate_canonical_binding().is_ok() => return Marker::Absent,
        _ => return Marker::Unavailable,
    };
    let result = read_observed_file_bounded(
        &health,
        OsStr::new(OBLIGATION_NAME),
        MAX_OBLIGATION_BYTES as usize,
    );
    // A failed claimed removal can retain the obligation under its claim name.
    // An absent original is not ready while that local recovery state remains.
    let retired = if matches!(result, Ok(None)) {
        match list_flat_directory(&health, 4096) {
            Ok(Some(entries)) => entries.iter().any(|entry| {
                entry
                    .name
                    .to_str()
                    .is_some_and(|name| name.starts_with("!solstone-claim-00000000-"))
            }),
            _ => return Marker::Unavailable,
        }
    } else {
        false
    };
    if root.revalidate_canonical_binding().is_err()
        || !matches!(
            open_flat_directory_bound(&root, health_name, root.canonical_path()),
            Ok(Some(current)) if current.identity() == health.identity()
        )
    {
        return Marker::Unavailable;
    }
    match result {
        Ok(Some(observed)) => Marker::Bytes(observed.bytes),
        Ok(None) if !retired => Marker::Absent,
        Ok(None) => Marker::Unavailable,
        Err(_) => Marker::Unavailable,
    }
}

#[cfg(not(unix))]
fn marker_bytes(journal: &Path) -> Marker {
    let health = journal.join("health");
    let file = journal.join(WHOLE_JOURNAL_RESTORE_FILE);
    if let Some(marker) = parent_marker(journal) {
        return marker;
    }
    if let Some(marker) = parent_marker(&health) {
        return marker;
    }
    match fs::symlink_metadata(&file) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Marker::Absent,
        Err(error) => {
            log::warn!("whole-journal restore obligation is unreadable: {error}");
            Marker::Unavailable
        }
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            log::warn!("whole-journal restore obligation is not a regular file");
            Marker::Unavailable
        }
        Ok(meta) if meta.len() > MAX_OBLIGATION_BYTES => {
            log::warn!("whole-journal restore obligation is too large");
            Marker::Unavailable
        }
        Ok(_) => match read_regular(&file) {
            Ok(bytes) if bytes.len() as u64 <= MAX_OBLIGATION_BYTES => Marker::Bytes(bytes),
            Ok(_) => {
                log::warn!("whole-journal restore obligation is too large");
                Marker::Unavailable
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Marker::Absent,
            Err(error) => {
                log::warn!("whole-journal restore obligation is unreadable: {error}");
                Marker::Unavailable
            }
        },
    }
}

fn parent_marker(path: &Path) -> Option<Marker> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(Marker::Absent),
        Err(error) => {
            log::warn!("whole-journal restore obligation parent is unreadable: {error}");
            Some(Marker::Unavailable)
        }
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            log::warn!("whole-journal restore obligation parent is substituted");
            Some(Marker::Unavailable)
        }
        Ok(_) => None,
    }
}

fn open_root(journal: &Path) -> Result<JournalRoot, WholeJournalRestoreWriteError> {
    let root = JournalRoot::open(journal).map_err(root_error)?;
    root.revalidate_canonical_binding().map_err(root_error)?;
    Ok(root)
}

fn require_same_binding(
    root: &JournalRoot,
    expected: &WholeJournalRestore,
) -> Result<(), WholeJournalRestoreWriteError> {
    if expected.root != canonical_root(root)?
        || expected.root_identity != root.identity().observation_id()
    {
        return Err(WholeJournalRestoreWriteError::Invalid(
            "obligation is not bound to this journal",
        ));
    }
    Ok(())
}

fn require_absent(journal: &Path) -> Result<(), WholeJournalRestoreWriteError> {
    match marker_bytes(journal) {
        Marker::Absent => Ok(()),
        Marker::Bytes(_) | Marker::Unavailable => Err(WholeJournalRestoreWriteError::Invalid(
            "obligation already exists",
        )),
    }
}

fn bound_record(
    root: &JournalRoot,
    attempt: u64,
    source: WholeJournalRestoreSource,
    snapshot_id: &str,
    subtree: &str,
) -> Result<WholeJournalRestore, WholeJournalRestoreWriteError> {
    Ok(WholeJournalRestore {
        v: RECORD_VERSION,
        attempt,
        root: canonical_root(root)?,
        root_identity: root.identity().observation_id(),
        source,
        snapshot_id: snapshot_id.to_owned(),
        subtree: subtree.to_owned(),
    })
}

#[cfg(unix)]
type HealthDirectory = FlatDirectory;
#[cfg(not(unix))]
type HealthDirectory = PathBuf;

#[cfg(unix)]
fn prepare_health_dir(
    root: &JournalRoot,
    create: bool,
) -> Result<HealthDirectory, WholeJournalRestoreWriteError> {
    use crate::flat_directory::{create_or_open_flat_directory_bound, open_flat_directory_bound};
    use std::ffi::OsStr;

    root.revalidate_canonical_binding().map_err(root_error)?;
    let health = if create {
        create_or_open_flat_directory_bound(
            root,
            OsStr::new("health"),
            0o755,
            root.canonical_path(),
        )
        .map_err(|error| WholeJournalRestoreWriteError::Io(error.to_string()))?
    } else {
        open_flat_directory_bound(root, OsStr::new("health"), root.canonical_path())
            .map_err(|error| WholeJournalRestoreWriteError::Io(error.to_string()))?
            .ok_or(WholeJournalRestoreWriteError::Invalid(
                "obligation is absent",
            ))?
    };
    revalidate_health(root, &health)?;
    Ok(health)
}

#[cfg(not(unix))]
fn prepare_health_dir(
    _root: &JournalRoot,
    _create: bool,
) -> Result<HealthDirectory, WholeJournalRestoreWriteError> {
    Err(WholeJournalRestoreWriteError::Invalid(
        "bound restore obligation writes are unavailable",
    ))
}

#[cfg(unix)]
fn revalidate_health(
    root: &JournalRoot,
    health: &HealthDirectory,
) -> Result<(), WholeJournalRestoreWriteError> {
    root.revalidate_canonical_binding().map_err(root_error)?;
    match crate::flat_directory::open_flat_directory_bound(
        root,
        std::ffi::OsStr::new("health"),
        root.canonical_path(),
    ) {
        Ok(Some(current)) if current.identity() == health.identity() => Ok(()),
        _ => Err(WholeJournalRestoreWriteError::Invalid(
            "health directory changed",
        )),
    }
}

#[cfg(not(unix))]
fn revalidate_health(
    _root: &JournalRoot,
    _health: &HealthDirectory,
) -> Result<(), WholeJournalRestoreWriteError> {
    Err(WholeJournalRestoreWriteError::Invalid(
        "bound restore obligation writes are unavailable",
    ))
}

fn with_obligation_lock<T>(
    root: &JournalRoot,
    health: &HealthDirectory,
    body: impl FnOnce() -> Result<T, WholeJournalRestoreWriteError>,
) -> Result<T, WholeJournalRestoreWriteError> {
    #[cfg(unix)]
    {
        let lock = crate::locking::acquire_existing_parent_lock_bound_checked(
            health,
            std::ffi::OsStr::new(OBLIGATION_LOCK),
            crate::locking::DEFAULT_LOCK_TIMEOUT,
            crate::locking::DEFAULT_LOCK_POLL_INTERVAL,
            &|| revalidate_health(root, health).map_err(|error| error.to_string()),
        )
        .map_err(|error| WholeJournalRestoreWriteError::Io(error.to_string()))?;
        let result = body();
        drop(lock);
        result
    }
    #[cfg(not(unix))]
    {
        let _ = (root, health, body);
        Err(WholeJournalRestoreWriteError::Invalid(
            "bound restore obligation writes are unavailable",
        ))
    }
}

#[cfg(unix)]
fn publish_bound_record(
    root: &JournalRoot,
    health: &HealthDirectory,
    bytes: &[u8],
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), WholeJournalRestoreWriteError> {
    use crate::atomic::{BoundAtomicOutcome, atomic_replace_bound_checked};
    let outcome = atomic_replace_bound_checked(
        health,
        std::ffi::OsStr::new(OBLIGATION_NAME),
        bytes,
        0o644,
        checkpoint,
    )
    .map_err(|error| WholeJournalRestoreWriteError::Io(error.to_string()))?;
    revalidate_health(root, health)?;
    match outcome {
        BoundAtomicOutcome::Published { .. } => Ok(()),
        other => Err(WholeJournalRestoreWriteError::Io(format!(
            "obligation publication is uncertain: {other:?}"
        ))),
    }
}

#[cfg(not(unix))]
fn publish_bound_record(
    _root: &JournalRoot,
    _health: &HealthDirectory,
    _bytes: &[u8],
    _checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), WholeJournalRestoreWriteError> {
    Err(WholeJournalRestoreWriteError::Invalid(
        "bound restore obligation writes are unavailable",
    ))
}

fn next_attempt() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let tick = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    let mixed = tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ nanos
        ^ u64::from(std::process::id()).rotate_left(17);
    if mixed == 0 { tick | 1 } else { mixed }
}

fn target_generations() -> &'static (Mutex<BTreeMap<PathBuf, u64>>, Condvar) {
    static GENERATIONS: (Mutex<BTreeMap<PathBuf, u64>>, Condvar) =
        (Mutex::new(BTreeMap::new()), Condvar::new());
    &GENERATIONS
}

fn bump_generation(journal: &Path) {
    let key = hold_key(journal);
    let (generations, wake) = target_generations();
    let mut generations = lock_map(generations);
    let next = generations.get(&key).copied().unwrap_or(0).wrapping_add(1);
    generations.insert(key, next);
    wake.notify_all();
}

#[cfg(not(unix))]
fn read_regular(path: &Path) -> io::Result<Vec<u8>> {
    let file = open_nofollow(path)?;
    let meta = file.metadata()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "whole-journal restore obligation is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_OBLIGATION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn retire_matching_file(
    root: &Arc<JournalRoot>,
    health: &HealthDirectory,
    expected: &[u8],
) -> Result<bool, WholeJournalRestoreWriteError> {
    #[cfg(not(target_os = "android"))]
    use crate::claim_remove::claim_and_remove_observed_bounded;
    use crate::errors::ClaimRemovalOutcome;
    use crate::flat_directory::read_observed_file_bounded;
    use crate::name_admission::ClaimName;
    use std::ffi::OsStr;

    revalidate_health(root, health)?;
    let Some(observed) = read_observed_file_bounded(
        health,
        OsStr::new(OBLIGATION_NAME),
        MAX_OBLIGATION_BYTES as usize,
    )
    .map_err(|error| WholeJournalRestoreWriteError::Io(error.to_string()))?
    else {
        return Ok(false);
    };
    if observed.bytes != expected {
        return Err(WholeJournalRestoreWriteError::Invalid(
            "obligation changed before retirement",
        ));
    }
    // The reserved pid-zero namespace distinguishes these recovery claims from
    // other application removals. A retained claim blocks cold-start readiness.
    let claim = ClaimName::parse(&format!("!solstone-claim-00000000-{:016x}", next_attempt()))
        .map_err(|_| WholeJournalRestoreWriteError::Invalid("retirement claim is invalid"))?;
    revalidate_health(root, health)?;
    #[cfg(not(target_os = "android"))]
    let outcome = claim_and_remove_observed_bounded(
        health,
        OsStr::new(OBLIGATION_NAME),
        &observed,
        &claim,
        MAX_OBLIGATION_BYTES as usize,
    )
    .map_err(|error| WholeJournalRestoreWriteError::Io(error.to_string()))?;
    #[cfg(target_os = "android")]
    let outcome = crate::claim_remove::claim_and_remove_android_restore_observed_bounded(
        root,
        health,
        &observed,
        &claim,
        MAX_OBLIGATION_BYTES as usize,
    )
    .map_err(|error| WholeJournalRestoreWriteError::Io(error.to_string()))?;
    revalidate_health(root, health)?;
    match outcome {
        ClaimRemovalOutcome::Removed => Ok(true),
        other => Err(WholeJournalRestoreWriteError::Io(format!(
            "obligation retirement is uncertain: {other:?}"
        ))),
    }
}

#[cfg(not(unix))]
fn retire_matching_file(
    _root: &Arc<JournalRoot>,
    _health: &HealthDirectory,
    _expected: &[u8],
) -> Result<bool, WholeJournalRestoreWriteError> {
    Err(WholeJournalRestoreWriteError::Invalid(
        "bound restore obligation writes are unavailable",
    ))
}

#[cfg(windows)]
fn open_nofollow(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_nofollow(path: &Path) -> io::Result<fs::File> {
    let _ = path;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "whole-journal restore lock is unavailable",
    ))
}

fn canonical_bytes(record: &WholeJournalRestore) -> Result<Vec<u8>, WholeJournalRestoreWriteError> {
    serialize_json_bytes(
        record,
        JsonWriteOptions {
            mode: None,
            indent: None,
            sort_keys: true,
        },
    )
    .map_err(|error| WholeJournalRestoreWriteError::Io(error.to_string()))
}

fn canonical_root(root: &JournalRoot) -> Result<String, WholeJournalRestoreWriteError> {
    root.canonical_path()
        .to_str()
        .map(str::to_owned)
        .ok_or(WholeJournalRestoreWriteError::Invalid(
            "journal path is not unicode",
        ))
}

fn validate_source(source: &WholeJournalRestoreSource) -> Result<(), &'static str> {
    match source {
        WholeJournalRestoreSource::Hosted {
            account_id,
            instance_id,
            bucket,
            prefix,
        } => {
            require_token(account_id, "account id is empty")?;
            require_token(instance_id, "instance id is empty")?;
            require_token(bucket, "bucket is empty")?;
            reject_controls(prefix, "prefix contains a control character")?;
        }
        WholeJournalRestoreSource::Byo {
            repository,
            backend,
        } => {
            require_token(repository, "repository is empty")?;
            require_token(backend, "backend is empty")?;
        }
    }
    Ok(())
}

fn validate_snapshot_id(snapshot_id: &str) -> Result<(), &'static str> {
    if snapshot_id.len() == 64
        && snapshot_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("snapshot id must be 64 lowercase hex characters")
    }
}

fn validate_subtree(subtree: &str) -> Result<(), &'static str> {
    require_token(subtree, "subtree is empty")
}

fn require_token(value: &str, empty: &'static str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err(empty);
    }
    reject_controls(value, "field contains a control character")
}

fn reject_controls(value: &str, reason: &'static str) -> Result<(), &'static str> {
    if value
        .bytes()
        .any(|byte| matches!(byte, b'\n' | b'\r' | b'\0'))
    {
        Err(reason)
    } else {
        Ok(())
    }
}

fn root_error(error: JournalRootError) -> WholeJournalRestoreWriteError {
    WholeJournalRestoreWriteError::Root(error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const SNAPSHOT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn source() -> WholeJournalRestoreSource {
        WholeJournalRestoreSource::Hosted {
            account_id: "account".into(),
            instance_id: "instance".into(),
            bucket: "bucket".into(),
            prefix: String::new(),
        }
    }

    fn journal() -> tempfile::TempDir {
        tempfile::tempdir().expect("journal")
    }

    #[test]
    fn absent_file_is_open_and_does_not_create_a_record() {
        let journal = journal();
        assert_eq!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        );
        assert_eq!(
            whole_journal_restore_admission(journal.path()),
            WholeJournalRestoreAdmission::Open
        );
        assert!(!journal.path().join(WHOLE_JOURNAL_RESTORE_FILE).exists());
        let missing = journal.path().join("missing-child");
        assert_eq!(
            read_whole_journal_restore(&missing),
            WholeJournalRestoreReadiness::Absent
        );
    }

    #[test]
    fn publish_binds_the_opened_root_and_clear_removes_only_that_record() {
        let journal = journal();
        let record = publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "/journal")
            .expect("publish");
        assert_eq!(
            record.root,
            journal.path().canonicalize().unwrap().to_str().unwrap()
        );
        match read_whole_journal_restore(journal.path()) {
            WholeJournalRestoreReadiness::Pending(read) => assert_eq!(*read, record),
            other => panic!("pending record, got {other:?}"),
        }
        assert_eq!(
            whole_journal_restore_admission(journal.path()),
            WholeJournalRestoreAdmission::Paused
        );
        assert!(whole_journal_restore_matches(journal.path(), &record));
        clear_whole_journal_restore(journal.path(), &record).expect("clear");
        assert_eq!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        );
        clear_whole_journal_restore(journal.path(), &record).expect("second clear");
    }

    #[test]
    fn malformed_unreadable_and_substituted_records_stay_paused() {
        let journal = journal();
        let path = journal.path().join(WHOLE_JOURNAL_RESTORE_FILE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{").unwrap();
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Unavailable
        ));
        assert!(
            publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "/journal").is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"{");
        fs::remove_file(&path).unwrap();

        let record = publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "/journal")
            .expect("publish");
        let mut substituted = record.clone();
        substituted.root = "/other/journal".into();
        let foreign = canonical_bytes(&substituted).unwrap();
        fs::write(&path, &foreign).unwrap();
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Unavailable
        ));
        let error = clear_whole_journal_restore(journal.path(), &record).unwrap_err();
        assert!(matches!(error, WholeJournalRestoreWriteError::Invalid(_)));
        assert_eq!(fs::read(&path).unwrap(), foreign);

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Unavailable
        ));
        assert!(path.is_dir());
    }

    #[test]
    fn whitespace_and_wrong_version_are_not_the_published_record() {
        let journal = journal();
        let path = journal.path().join(WHOLE_JOURNAL_RESTORE_FILE);
        let record = publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "/journal")
            .expect("publish");
        let pretty = serde_json::to_string_pretty(&record).unwrap();
        fs::write(&path, &pretty).unwrap();
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));
        assert!(!whole_journal_restore_matches(journal.path(), &record));
        assert!(clear_whole_journal_restore(journal.path(), &record).is_err());
        assert_eq!(fs::read(&path).unwrap(), pretty.as_bytes());

        fs::write(&path, br#"{"v":2,"root":"x","source":{"kind":"byo","repository":"r","backend":"s3"},"snapshot_id":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","subtree":"/journal"}"#).unwrap();
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Unavailable
        ));
    }

    #[test]
    fn publish_rejects_empty_identity_and_a_short_snapshot_before_writing() {
        let journal = journal();
        let mut empty = source();
        if let WholeJournalRestoreSource::Hosted { account_id, .. } = &mut empty {
            account_id.clear();
        }
        assert!(
            publish_whole_journal_restore(journal.path(), &empty, SNAPSHOT, "/journal").is_err()
        );
        assert!(
            publish_whole_journal_restore(journal.path(), &source(), "abcd", "/journal").is_err()
        );
        assert!(
            publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "a\nb").is_err()
        );
        assert!(!journal.path().join(WHOLE_JOURNAL_RESTORE_FILE).exists());
        let byo = WholeJournalRestoreSource::Byo {
            repository: "repo".into(),
            backend: "s3".into(),
        };
        let record =
            publish_whole_journal_restore(journal.path(), &byo, SNAPSHOT, "/journal").expect("byo");
        assert!(matches!(
            record.source,
            WholeJournalRestoreSource::Byo { .. }
        ));
    }

    #[test]
    fn absent_use_is_admitted_and_a_refused_use_leaves_no_flight() {
        let journal = journal();
        let guard = begin_target_use(journal.path()).expect("absent admits use");
        drop(guard);
        wait_for_target_use_to_end(journal.path());

        publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "/journal")
            .expect("publish");
        assert!(begin_target_use(journal.path()).is_err());
        assert!(whole_journal_restore_blocks(journal.path()));
        wait_for_target_use_to_end(journal.path());
    }

    #[test]
    fn hold_blocks_new_use_until_release_and_wait_returns_after_drop() {
        let journal = journal();
        let path = journal.path().to_path_buf();
        hold_target_for_restore(&path);
        assert!(target_held_for_restore(&path));
        assert!(begin_target_use(&path).is_err());
        wait_for_target_use_to_end(&path);

        release_target_for_restore(&path);
        assert!(!target_held_for_restore(&path));
        let guard = begin_target_use(&path).expect("release admits use");
        let waiting = path.clone();
        let thread = std::thread::spawn(move || wait_for_target_use_to_end(&waiting));
        drop(guard);
        thread.join().expect("wait returns after the use drops");
    }

    #[test]
    fn native_retirement_requires_a_held_target_and_completed_uses() {
        let journal = journal();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        assert!(!restore_target_quiescent(journal.path()));
        let admitted = begin_target_use_bound(root).expect("ordinary accepted use");
        hold_target_for_restore(journal.path());
        assert!(!restore_target_quiescent(journal.path()));
        assert!(begin_target_use(journal.path()).is_err());
        drop(admitted);
        assert!(restore_target_quiescent(journal.path()));
        release_target_for_restore(journal.path());
        assert!(!restore_target_quiescent(journal.path()));
    }

    #[test]
    fn publish_does_not_overwrite_and_a_stale_attempt_cannot_clear() {
        let journal = journal();
        let first = publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "/journal")
            .expect("publish");
        assert_ne!(first.attempt, 0);
        assert!(
            publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "/journal").is_err()
        );
        assert!(whole_journal_restore_matches(journal.path(), &first));

        let adopted = adopt_whole_journal_restore(journal.path(), &first).expect("adopt");
        assert_ne!(adopted.attempt, first.attempt);
        assert_eq!(adopted.snapshot_id, first.snapshot_id);
        assert_eq!(adopted.subtree, first.subtree);
        assert_eq!(adopted.root_identity, first.root_identity);
        assert!(clear_whole_journal_restore(journal.path(), &first).is_err());
        assert!(whole_journal_restore_matches(journal.path(), &adopted));
        clear_whole_journal_restore(journal.path(), &adopted).expect("clear adopted");
        assert_eq!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        );
        assert!(adopt_whole_journal_restore(journal.path(), &adopted).is_err());
    }

    #[test]
    fn generation_advances_on_publish_adopt_and_clear() {
        let journal = journal();
        let path = journal.path().to_path_buf();
        let observed = whole_journal_restore_generation(&path);
        let waiting = path.clone();
        let thread =
            std::thread::spawn(move || wait_whole_journal_restore_generation(&waiting, observed));
        let first =
            publish_whole_journal_restore(&path, &source(), SNAPSHOT, "/journal").expect("publish");
        thread.join().expect("publish wakes the generation");
        let published = whole_journal_restore_generation(&path);
        assert_ne!(published, observed);

        let adopted = adopt_whole_journal_restore(&path, &first).expect("adopt");
        let adopted_generation = whole_journal_restore_generation(&path);
        assert_ne!(adopted_generation, published);
        clear_whole_journal_restore(&path, &adopted).expect("clear");
        assert_ne!(whole_journal_restore_generation(&path), adopted_generation);
    }

    #[test]
    fn forged_root_identity_stays_unavailable() {
        let journal = journal();
        let record = publish_whole_journal_restore(journal.path(), &source(), SNAPSHOT, "/journal")
            .expect("publish");
        let mut forged = record.clone();
        forged.root_identity = "native-root-v1:unix:0000000000000000:0000000000000000".into();
        let bytes = canonical_bytes(&forged).unwrap();
        let path = journal.path().join(WHOLE_JOURNAL_RESTORE_FILE);
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Unavailable
        ));
        assert!(clear_whole_journal_restore(journal.path(), &record).is_err());
        assert!(clear_whole_journal_restore(journal.path(), &forged).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn a_parent_replaced_during_the_read_never_publishes_open() {
        use crate::flat_directory::{FlatDirectoryTestPrimitive, run_with_flat_directory_hook};

        for pending in [false, true] {
            let primary = journal();
            let outside = journal();
            let health = primary.path().join("health");
            if pending {
                publish_whole_journal_restore(primary.path(), &source(), SNAPSHOT, "/journal")
                    .expect("publish");
            } else {
                fs::create_dir(&health).expect("health");
            }
            let moved = primary.path().join("retained-health");
            let swap_health = health.clone();
            let outside_path = outside.path().to_owned();
            let (readiness, fired) = run_with_flat_directory_hook(
                FlatDirectoryTestPrimitive::BeforeObservedFileOpen,
                move || {
                    fs::rename(&swap_health, &moved).expect("retain health");
                    std::os::unix::fs::symlink(&outside_path, &swap_health)
                        .expect("replace parent");
                },
                || read_whole_journal_restore(primary.path()),
            );
            assert!(fired);
            assert!(matches!(
                readiness,
                WholeJournalRestoreReadiness::Unavailable
            ));
            assert!(health.symlink_metadata().unwrap().file_type().is_symlink());
            assert!(!outside.path().join("whole-journal-restore.json").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_parent_substitution_are_not_absent() {
        let primary = journal();
        let path = primary.path().join(WHOLE_JOURNAL_RESTORE_FILE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/no/such/obligation", &path).unwrap();
        assert!(matches!(
            read_whole_journal_restore(primary.path()),
            WholeJournalRestoreReadiness::Unavailable
        ));
        assert_eq!(
            whole_journal_restore_admission(primary.path()),
            WholeJournalRestoreAdmission::Paused
        );
        let other = journal();
        let record = publish_whole_journal_restore(other.path(), &source(), SNAPSHOT, "/journal")
            .expect("other");
        assert!(clear_whole_journal_restore(primary.path(), &record).is_err());
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(
            publish_whole_journal_restore(primary.path(), &source(), SNAPSHOT, "/journal").is_err()
        );
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
        fs::remove_file(&path).unwrap();
        let owned = publish_whole_journal_restore(primary.path(), &source(), SNAPSHOT, "/journal")
            .expect("owned");
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/no/such/obligation", &path).unwrap();
        assert!(clear_whole_journal_restore(primary.path(), &owned).is_err());
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());

        let substituted = journal();
        let health = substituted.path().join("health");
        std::os::unix::fs::symlink("/no/such/health", &health).unwrap();
        assert!(matches!(
            read_whole_journal_restore(substituted.path()),
            WholeJournalRestoreReadiness::Unavailable
        ));
        assert!(
            publish_whole_journal_restore(substituted.path(), &source(), SNAPSHOT, "/journal")
                .is_err()
        );
        assert!(health.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn publication_parent_swap_never_writes_the_replacement_directory() {
        use crate::atomic::{BoundPublicationPrimitive, run_with_bound_publication_barrier};
        let primary = journal();
        let outside = journal();
        let health = primary.path().join("health");
        let retained = primary.path().join("retained-health");
        let swap_health = health.clone();
        let swap_retained = retained.clone();
        let outside_path = outside.path().to_owned();
        let (result, fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::AuthorizedRename,
            1,
            move || {
                fs::rename(&swap_health, &swap_retained).unwrap();
                std::os::unix::fs::symlink(outside_path, &swap_health).unwrap();
            },
            || publish_whole_journal_restore(primary.path(), &source(), SNAPSHOT, "/journal"),
        );
        assert!(fired);
        assert!(result.is_err());
        assert!(!outside.path().join(OBLIGATION_NAME).exists());
        assert!(retained.join(OBLIGATION_NAME).is_file());
        assert_eq!(
            whole_journal_restore_admission(primary.path()),
            WholeJournalRestoreAdmission::Paused
        );
    }

    #[test]
    fn retirement_preserves_a_replacement_after_its_initial_observation() {
        use crate::claim_remove::{ClaimRemovalPrimitive, run_with_claim_removal_barrier};
        let primary = journal();
        let record =
            publish_whole_journal_restore(primary.path(), &source(), SNAPSHOT, "/journal").unwrap();
        let path = primary.path().join(WHOLE_JOURNAL_RESTORE_FILE);
        let swap_path = path.clone();
        let (result, fired) = run_with_claim_removal_barrier(
            ClaimRemovalPrimitive::BeforeClaim,
            1,
            move || {
                fs::remove_file(&swap_path).unwrap();
                fs::write(&swap_path, b"replacement").unwrap();
            },
            || clear_whole_journal_restore(primary.path(), &record),
        );
        assert!(fired);
        assert!(result.is_err());
        assert_eq!(fs::read(path).unwrap(), b"replacement");
        assert_eq!(
            whole_journal_restore_admission(primary.path()),
            WholeJournalRestoreAdmission::Paused
        );
    }

    #[test]
    fn a_failed_unlink_keeps_cold_start_paused_under_the_retained_claim() {
        use crate::claim_remove::{ClaimRemovalPrimitive, run_with_claim_removal_fault};
        let primary = journal();
        let record =
            publish_whole_journal_restore(primary.path(), &source(), SNAPSHOT, "/journal").unwrap();
        let (result, fired) = run_with_claim_removal_fault(
            ClaimRemovalPrimitive::Unlink,
            1,
            nix::errno::Errno::EIO as i32,
            || clear_whole_journal_restore(primary.path(), &record),
        );
        assert!(fired);
        assert!(result.is_err());
        assert!(!primary.path().join(WHOLE_JOURNAL_RESTORE_FILE).exists());
        assert_eq!(
            whole_journal_restore_admission(primary.path()),
            WholeJournalRestoreAdmission::Paused
        );
        assert!(begin_target_use(primary.path()).is_err());
    }
}
