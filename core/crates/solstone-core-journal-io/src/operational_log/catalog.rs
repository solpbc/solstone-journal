// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only, descriptor-bound discovery of canonical `oplog--` leaves.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

#[cfg(test)]
use std::cell::{Cell, RefCell};

use chrono::NaiveDate;

use super::LeaseProbe;
use super::admission::{OPLOG_ADMISSION_MAX_BYTES, validate_oplog_admission};
use super::name::{OplogName, OplogNameClassification, classify_oplog_name};
use super::namespace::OplogDayHealth;
use super::reason::{NamedOpen, OplogFileIdentity};
#[cfg(unix)]
use super::unix as platform;
#[cfg(windows)]
use super::windows as platform;
use crate::errors::FlatDirectoryError;
#[cfg(unix)]
use crate::flat_directory::{list_flat_directory, open_flat_directory_bound};
use crate::journal_root::{JournalEntryKind, JournalRoot};
use crate::observation::FlatDirectoryEntry;
use crate::paths::is_day_key;
#[cfg(windows)]
use crate::windows_sync_dir::{list_windows_flat_directory, open_windows_flat_directory_bound};

const CHRONICLE_DIR: &str = "chronicle";
const HEALTH_DIR: &str = "health";

#[cfg(test)]
thread_local! {
    static FORCED_UNSTABLE_ATTEMPTS: Cell<usize> = const { Cell::new(0) };
    static CATALOG_ONCE_CALLS: Cell<usize> = const { Cell::new(0) };
    static AFTER_INITIAL_LIST: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static FORCE_UNSTABLE_AFTER_ENTRIES: Cell<usize> = const { Cell::new(0) };
    static LIVE_RETAINED_DESCRIPTORS: Cell<usize> = const { Cell::new(0) };
    static MAX_LIVE_RETAINED_DESCRIPTORS: Cell<usize> = const { Cell::new(0) };
    static CATALOG_ENTRY_OPEN_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// Maximum complete-census retries after an identity or enumeration race.
pub const OPLOG_CATALOG_CENSUS_ATTEMPTS: usize = 4;
/// Maximum reserved canonical candidates accepted in one day directory.
pub const OPLOG_CATALOG_MAX_CANDIDATES_PER_DAY: usize = 512;
/// Maximum direct directory entries inspected across one complete catalog pass.
pub const OPLOG_CATALOG_MAX_COUNTABLE_ENTRIES_PER_PASS: usize = 4096;

/// One validated canonical operational-log leaf.
#[derive(Clone, Debug)]
pub struct OplogCatalogEntry {
    day: String,
    leaf: OsString,
    name: OplogName,
    identity: OplogFileIdentity,
    size: u64,
    payload_offset: usize,
    modified: Option<std::time::SystemTime>,
}

impl OplogCatalogEntry {
    /// Local YYYYMMDD partition containing the leaf.
    pub fn day(&self) -> &str {
        &self.day
    }

    /// Canonical native leaf spelling.
    pub fn leaf(&self) -> &OsStr {
        &self.leaf
    }

    /// Parsed canonical coordinates.
    pub fn name(&self) -> &OplogName {
        &self.name
    }

    /// Stable platform identity captured while cataloguing.
    pub fn identity(&self) -> OplogFileIdentity {
        self.identity
    }

    /// Byte length observed while cataloguing.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Modification time from the admitted descriptor, when supported.
    pub fn modified(&self) -> Option<std::time::SystemTime> {
        self.modified
    }

    /// Number of admission-header bytes before text payload begins.
    pub fn payload_offset(&self) -> usize {
        self.payload_offset
    }
}

/// Complete ordered read-only view of requested local days.
#[derive(Debug, Default)]
pub struct OplogCatalogSnapshot {
    entries: Vec<OplogCatalogEntry>,
    descriptors: Vec<RetainedDescriptor>,
}

impl OplogCatalogSnapshot {
    /// Entries in deterministic day/leaf order.
    pub fn entries(&self) -> &[OplogCatalogEntry] {
        &self.entries
    }

    /// Consume this snapshot and transfer every admission-bound descriptor to
    /// the caller in the same deterministic order as [`Self::entries`].
    pub fn into_catalogued_entries(self) -> Vec<(OplogCatalogEntry, File)> {
        self.entries
            .into_iter()
            .zip(self.descriptors)
            .map(|(entry, descriptor)| (entry, descriptor.into_file()))
            .collect()
    }
}

/// A catalogued file held open from admission until a consuming caller adopts
/// it.  Keeping this private preserves `OplogCatalogEntry` as cloneable
/// metadata while making descriptor lifetime explicit in the snapshot.
#[derive(Debug)]
struct RetainedDescriptor {
    file: Option<File>,
}

impl RetainedDescriptor {
    fn new(file: File) -> Self {
        #[cfg(test)]
        LIVE_RETAINED_DESCRIPTORS.with(|live| {
            let value = live.get() + 1;
            live.set(value);
            MAX_LIVE_RETAINED_DESCRIPTORS.with(|maximum| maximum.set(maximum.get().max(value)));
        });
        Self { file: Some(file) }
    }

    fn into_file(mut self) -> File {
        let file = self.file.take().expect("retained descriptor holds a file");
        #[cfg(test)]
        LIVE_RETAINED_DESCRIPTORS.with(|live| live.set(live.get() - 1));
        file
    }
}

impl Drop for RetainedDescriptor {
    fn drop(&mut self) {
        if self.file.is_some() {
            #[cfg(test)]
            LIVE_RETAINED_DESCRIPTORS.with(|live| live.set(live.get() - 1));
        }
    }
}

/// Closed error from an all-or-nothing operational-log catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OplogCatalogError {
    kind: &'static str,
    day: Option<String>,
}

impl OplogCatalogError {
    fn new(kind: &'static str, day: Option<&str>) -> Self {
        Self {
            kind,
            day: day.map(str::to_owned),
        }
    }

    /// Stable failure kind, suitable for a support-bundle error field.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Affected local day, when the failure belongs to one partition.
    pub fn day(&self) -> Option<&str> {
        self.day.as_deref()
    }

    fn retryable(&self) -> bool {
        matches!(
            self.kind,
            "oplog_catalog_identity_changed" | "oplog_catalog_enumeration_changed"
        )
    }

    pub(crate) fn io_for_day(day: &str) -> Self {
        Self::new("oplog_catalog_io", Some(day))
    }

    pub(crate) fn kind_for_day(kind: &'static str, day: &str) -> Self {
        Self::new(kind, Some(day))
    }

    pub fn root() -> Self {
        Self::new("oplog_catalog_root", None)
    }

    pub(crate) fn identity_for_day(day: &str) -> Self {
        Self::new("oplog_catalog_identity_changed", Some(day))
    }
}

impl fmt::Display for OplogCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.day {
            Some(day) => write!(formatter, "{}_{}", self.kind, day),
            None => formatter.write_str(self.kind),
        }
    }
}

impl Error for OplogCatalogError {}

/// Build a complete catalog of canonical oplogs for the supplied local days.
pub fn catalog_oplogs(
    root: JournalRoot,
    days: &[NaiveDate],
) -> Result<OplogCatalogSnapshot, OplogCatalogError> {
    let mut days = days
        .iter()
        .map(|day| day.format("%Y%m%d").to_string())
        .collect::<Vec<_>>();
    days.sort();
    days.dedup();

    for attempt in 0..OPLOG_CATALOG_CENSUS_ATTEMPTS {
        match catalog_once(&root, &days) {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) if error.retryable() && attempt + 1 < OPLOG_CATALOG_CENSUS_ATTEMPTS => {}
            Err(error) if error.retryable() => {
                return Err(OplogCatalogError::new(
                    "oplog_catalog_unstable",
                    error.day(),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded catalog attempts either return or fail")
}

/// Fold a complete census without retaining one descriptor per historical log.
///
/// Each callback reads from the admitted identity, bounded at its observed byte
/// frontier. At most one candidate descriptor is live. Directory/admission safety,
/// the countable-entry budget and whole-census retries are shared with snapshots.
/// A failed attempt discards its accumulator. Callbacks must only update that
/// accumulator: external effects cannot be rolled back on a census retry.
pub fn fold_oplogs<T: Default>(
    root: JournalRoot,
    days: &[NaiveDate],
    read: impl Fn(&mut T, &OplogCatalogEntry, &mut dyn Read) -> std::io::Result<()>,
) -> Result<T, OplogCatalogError> {
    let mut days = days
        .iter()
        .map(|day| day.format("%Y%m%d").to_string())
        .collect::<Vec<_>>();
    days.sort();
    days.dedup();
    for attempt in 0..OPLOG_CATALOG_CENSUS_ATTEMPTS {
        let result = catalog_pass(
            &root,
            &days,
            None,
            T::default(),
            |value, entry, mut retained| {
                let file = retained
                    .file
                    .as_mut()
                    .expect("retained descriptor holds a file");
                file.seek(SeekFrom::Start(entry.payload_offset as u64))?;
                let mut frontier =
                    file.take(entry.size.saturating_sub(entry.payload_offset as u64));
                read(value, &entry, &mut frontier)
            },
        );
        match result {
            Ok(value) => return Ok(value),
            Err(error) if error.retryable() && attempt + 1 < OPLOG_CATALOG_CENSUS_ATTEMPTS => {}
            Err(error) if error.retryable() => {
                return Err(OplogCatalogError::new(
                    "oplog_catalog_unstable",
                    error.day(),
                ));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded census attempts return a value or an error")
}

/// Probe the writer lease through an admission-bound descriptor, never a name
/// lookup. The identity is only needed by Windows' `OpenFileById` probe.
pub fn probe_retained_oplog_lease(file: &File, identity: OplogFileIdentity) -> LeaseProbe {
    #[cfg(unix)]
    {
        let _ = identity;
        crate::lease::probe_file_lease(file)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;

        super::windows_liveness::classify_liveness_by_id(file.as_raw_handle(), identity)
    }
}

struct CataloguedDay {
    health: OplogDayHealth,
    listed: Vec<FlatDirectoryEntry>,
}

fn catalog_once(
    root: &JournalRoot,
    days: &[String],
) -> Result<OplogCatalogSnapshot, OplogCatalogError> {
    let mut entries = catalog_pass(
        root,
        days,
        Some(OPLOG_CATALOG_MAX_CANDIDATES_PER_DAY),
        Vec::new(),
        |entries, entry, retained| {
            entries.push((entry, retained));
            Ok(())
        },
    )?;
    entries.sort_by(|(left, _), (right, _)| {
        left.day.cmp(&right.day).then_with(|| {
            left.leaf
                .as_encoded_bytes()
                .cmp(right.leaf.as_encoded_bytes())
        })
    });
    let (entries, descriptors) = entries.into_iter().unzip();
    Ok(OplogCatalogSnapshot {
        entries,
        descriptors,
    })
}

fn catalog_pass<T>(
    root: &JournalRoot,
    days: &[String],
    candidate_limit: Option<usize>,
    mut value: T,
    mut accept: impl FnMut(&mut T, OplogCatalogEntry, RetainedDescriptor) -> std::io::Result<()>,
) -> Result<T, OplogCatalogError> {
    #[cfg(test)]
    {
        CATALOG_ONCE_CALLS.with(|calls| calls.set(calls.get() + 1));
        let forced = FORCED_UNSTABLE_ATTEMPTS.with(|remaining| {
            let value = remaining.get();
            remaining.set(value.saturating_sub(1));
            value > 0
        });
        if forced {
            return Err(OplogCatalogError::new(
                "oplog_catalog_identity_changed",
                None,
            ));
        }
    }
    let mut listed_days = Vec::new();
    let mut countable = 0_usize;

    for day in days {
        let day_root = JournalRoot::open(root.canonical_path())
            .map_err(|_| OplogCatalogError::new("oplog_catalog_root", Some(day)))?;
        if day_root.identity() != root.identity() {
            return Err(OplogCatalogError::new(
                "oplog_catalog_identity_changed",
                Some(day),
            ));
        }
        let Some(health) = open_existing_day_health_directory(day_root, day)? else {
            continue;
        };
        let remaining = OPLOG_CATALOG_MAX_COUNTABLE_ENTRIES_PER_PASS.saturating_sub(countable);
        let listed = list_health(&health, remaining)?
            .ok_or_else(|| OplogCatalogError::new("oplog_catalog_countable_limit", Some(day)))?;
        countable = countable.saturating_add(listed.len());
        if countable > OPLOG_CATALOG_MAX_COUNTABLE_ENTRIES_PER_PASS {
            return Err(OplogCatalogError::new(
                "oplog_catalog_countable_limit",
                Some(day),
            ));
        }

        #[cfg(test)]
        if let Some(action) = AFTER_INITIAL_LIST.with(|hook| hook.borrow_mut().take()) {
            action();
        }

        let candidates = listed
            .iter()
            .filter(|item| {
                matches!(
                    classify_oplog_name(&item.name),
                    OplogNameClassification::Candidate(_)
                )
            })
            .count();
        if candidate_limit.is_some_and(|limit| candidates > limit) {
            return Err(OplogCatalogError::new(
                "oplog_catalog_candidate_limit",
                Some(day),
            ));
        }
        for item in &listed {
            let classification = classify_oplog_name(&item.name);
            let OplogNameClassification::Candidate(parsed) = classification else {
                continue;
            };
            let name =
                parsed.map_err(|_| OplogCatalogError::new("oplog_catalog_malformed", Some(day)))?;
            if item.kind != JournalEntryKind::RegularFile {
                return Err(OplogCatalogError::new("oplog_catalog_unsafe", Some(day)));
            }
            let (entry, retained) = catalog_entry(&health, day, item, name)?;
            accept(&mut value, entry, retained)
                .map_err(|_| OplogCatalogError::new("oplog_catalog_read", Some(day)))?;
        }
        listed_days.push(CataloguedDay { health, listed });
    }

    #[cfg(test)]
    if FORCE_UNSTABLE_AFTER_ENTRIES.with(|remaining| {
        let value = remaining.get();
        remaining.set(value.saturating_sub(1));
        value > 0
    }) {
        return Err(OplogCatalogError::new(
            "oplog_catalog_identity_changed",
            None,
        ));
    }

    for catalogued in &listed_days {
        catalogued.health.revalidate_binding().map_err(|_| {
            OplogCatalogError::new(
                "oplog_catalog_identity_changed",
                Some(catalogued.health.day()),
            )
        })?;
        let observed = list_health(
            &catalogued.health,
            catalogued.listed.len().saturating_add(1),
        )?
        .ok_or_else(|| {
            OplogCatalogError::new(
                "oplog_catalog_enumeration_changed",
                Some(catalogued.health.day()),
            )
        })?;
        if observed != catalogued.listed {
            return Err(OplogCatalogError::new(
                "oplog_catalog_enumeration_changed",
                Some(catalogued.health.day()),
            ));
        }
    }
    root.revalidate_canonical_binding()
        .map_err(|_| OplogCatalogError::new("oplog_catalog_identity_changed", None))?;
    Ok(value)
}

fn catalog_entry(
    health: &OplogDayHealth,
    day: &str,
    item: &FlatDirectoryEntry,
    name: OplogName,
) -> Result<(OplogCatalogEntry, RetainedDescriptor), OplogCatalogError> {
    let opened = platform::open_named(health, &item.name)
        .map_err(|_| OplogCatalogError::new("oplog_catalog_io", Some(day)))?;
    let NamedOpen::Regular {
        file,
        identity,
        nlink,
    } = opened
    else {
        return Err(OplogCatalogError::new("oplog_catalog_unsafe", Some(day)));
    };
    #[cfg(test)]
    CATALOG_ENTRY_OPEN_CALLS.with(|calls| calls.set(calls.get() + 1));
    let mut retained = RetainedDescriptor::new(file);
    if nlink != 1 {
        return Err(OplogCatalogError::new("oplog_catalog_unsafe", Some(day)));
    }
    let mut bytes = Vec::with_capacity(OPLOG_ADMISSION_MAX_BYTES);
    let mut buffer = [0_u8; 256];
    while bytes.len() < OPLOG_ADMISSION_MAX_BYTES && !bytes.contains(&b'\n') {
        let wanted = (OPLOG_ADMISSION_MAX_BYTES - bytes.len()).min(buffer.len());
        let read = retained
            .file
            .as_mut()
            .expect("retained descriptor holds a file")
            .read(&mut buffer[..wanted])
            .map_err(|_| OplogCatalogError::new("oplog_catalog_io", Some(day)))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let admission = validate_oplog_admission(&item.name, &bytes)
        .map_err(|_| OplogCatalogError::new("oplog_catalog_admission", Some(day)))?;
    if admission.leaf() != &name
        || platform::identity_of(
            retained
                .file
                .as_ref()
                .expect("retained descriptor holds a file"),
        )
        .ok()
            != Some(identity)
    {
        return Err(OplogCatalogError::new(
            "oplog_catalog_identity_changed",
            Some(day),
        ));
    }
    if platform::nlink_of(
        retained
            .file
            .as_ref()
            .expect("retained descriptor holds a file"),
    )
    .ok()
        != Some(1)
    {
        return Err(OplogCatalogError::new("oplog_catalog_unsafe", Some(day)));
    }
    Ok((
        OplogCatalogEntry {
            day: day.to_owned(),
            leaf: item.name.clone(),
            name,
            identity,
            size: item.size,
            payload_offset: admission.header_len(),
            modified: retained
                .file
                .as_ref()
                .and_then(|file| file.metadata().ok())
                .and_then(|metadata| metadata.modified().ok()),
        },
        retained,
    ))
}

/// Open the existing chain only. This deliberately has no create branch.
fn open_existing_day_health_directory(
    root: JournalRoot,
    day: &str,
) -> Result<Option<OplogDayHealth>, OplogCatalogError> {
    if !is_day_key(day) {
        return Err(OplogCatalogError::new(
            "oplog_catalog_invalid_day",
            Some(day),
        ));
    }
    #[cfg(unix)]
    {
        let chronicle = match open_flat_directory_bound(
            &root,
            OsStr::new(CHRONICLE_DIR),
            root.canonical_path(),
        ) {
            Ok(Some(directory)) => directory,
            Ok(None) => return Ok(None),
            Err(error) => return Err(map_directory_error(error, day)),
        };
        let day_directory = match open_flat_directory_bound(
            &chronicle,
            OsStr::new(day),
            chronicle.diagnostic_path(),
        ) {
            Ok(Some(directory)) => directory,
            Ok(None) => return Ok(None),
            Err(error) => return Err(map_directory_error(error, day)),
        };
        let health = match open_flat_directory_bound(
            &day_directory,
            OsStr::new(HEALTH_DIR),
            day_directory.diagnostic_path(),
        ) {
            Ok(Some(directory)) => directory,
            Ok(None) => return Ok(None),
            Err(error) => return Err(map_directory_error(error, day)),
        };
        Ok(Some(OplogDayHealth {
            day: day.to_owned(),
            root,
            chronicle_identity: chronicle.identity(),
            day_identity: day_directory.identity(),
            health,
        }))
    }
    #[cfg(windows)]
    {
        let chronicle = match open_windows_flat_directory_bound(
            &root,
            OsStr::new(CHRONICLE_DIR),
            root.canonical_path(),
        ) {
            Ok(Some(directory)) => directory,
            Ok(None) => return Ok(None),
            Err(error) => return Err(map_directory_error(error, day)),
        };
        let day_directory = match open_windows_flat_directory_bound(
            &chronicle,
            OsStr::new(day),
            chronicle.diagnostic_path(),
        ) {
            Ok(Some(directory)) => directory,
            Ok(None) => return Ok(None),
            Err(error) => return Err(map_directory_error(error, day)),
        };
        let health = match open_windows_flat_directory_bound(
            &day_directory,
            OsStr::new(HEALTH_DIR),
            day_directory.diagnostic_path(),
        ) {
            Ok(Some(directory)) => directory,
            Ok(None) => return Ok(None),
            Err(error) => return Err(map_directory_error(error, day)),
        };
        let chronicle_identity = chronicle.identity();
        let day_identity = day_directory.identity();
        Ok(Some(OplogDayHealth {
            day: day.to_owned(),
            root,
            chronicle_identity: crate::journal_root::ObjectIdentity::from_volume_and_file_id(
                chronicle_identity.volume_serial(),
                chronicle_identity.file_id(),
            ),
            day_identity: crate::journal_root::ObjectIdentity::from_volume_and_file_id(
                day_identity.volume_serial(),
                day_identity.file_id(),
            ),
            health,
        }))
    }
}

#[cfg(unix)]
fn list_health(
    health: &OplogDayHealth,
    maximum: usize,
) -> Result<Option<Vec<FlatDirectoryEntry>>, OplogCatalogError> {
    list_flat_directory(health.health(), maximum)
        .map_err(|error| map_directory_error(error, health.day()))
}

#[cfg(windows)]
fn list_health(
    health: &OplogDayHealth,
    maximum: usize,
) -> Result<Option<Vec<FlatDirectoryEntry>>, OplogCatalogError> {
    list_windows_flat_directory(health.health(), maximum)
        .map_err(|error| map_directory_error(error, health.day()))
}

/// restic's glob for the runtime-log name shape in every day health directory,
/// relative to the journal. It matches exactly the leaves [`has_runtime_log_shape`]
/// admits, in exactly the directories [`visit_runtime_log_exclusions`] reads.
/// Restic tests every walked path against every exclude pattern in turn, so a
/// pattern per runtime log made each backup cost walked files times logs.
pub const RUNTIME_LOG_SHAPE: &str =
    "chronicle/[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]/health/oplog--*.log";

/// Stream ordinary canonical runtime log names into a private exclusion file.
/// Both native implementations retain directories and inspect selected leaves
/// without following links. Neither directory names nor argv grow in memory.
pub fn visit_runtime_log_paths(
    root: &JournalRoot,
    mut visit: impl FnMut(&str) -> std::io::Result<()>,
) -> std::io::Result<()> {
    visit_runtime_log_days(root, |_, _, logs| {
        logs.iter().try_for_each(|member| visit(member))
    })
}

/// Positive exclusion patterns, relative to the journal, that leave out exactly
/// what [`visit_runtime_log_paths`] selects. An ordinary journal gets the single
/// [`RUNTIME_LOG_SHAPE`]. When a day's health directory holds owner content of
/// the same shape (a malformed name, a directory or a link), that day lists its
/// logs exactly and every other day gets its own shape. No pattern is negated,
/// so every restic consumer reads them with the same OR semantics.
pub fn visit_runtime_log_exclusions(
    root: &JournalRoot,
    mut visit: impl FnMut(&str) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut shaped_days = Vec::new();
    let mut any_lookalike = false;
    visit_runtime_log_days(root, |day, owner_lookalike, logs| {
        if !owner_lookalike {
            shaped_days.push(day.to_owned());
            return Ok(());
        }
        any_lookalike = true;
        logs.iter().try_for_each(|member| visit(member))
    })?;
    if !any_lookalike {
        return visit(RUNTIME_LOG_SHAPE);
    }
    shaped_days
        .iter()
        .try_for_each(|day| visit(&format!("chronicle/{day}/health/oplog--*.log")))
}

/// The leaf shape restic's `oplog--*.log` matches, on the native name.
fn has_runtime_log_shape(name: &OsStr) -> bool {
    const SUFFIX: &[u8] = b".log";
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = name.as_bytes();
        bytes.len() >= PREFIX_LEN + SUFFIX.len()
            && bytes.starts_with(b"oplog--")
            && bytes.ends_with(SUFFIX)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide = name.encode_wide().collect::<Vec<_>>();
        let ascii = |units: &[u16], bytes: &[u8]| {
            units
                .iter()
                .zip(bytes)
                .all(|(unit, byte)| *unit == u16::from(*byte))
        };
        wide.len() >= PREFIX_LEN + SUFFIX.len()
            && ascii(&wide[..PREFIX_LEN], b"oplog--")
            && ascii(&wide[wide.len() - SUFFIX.len()..], SUFFIX)
    }
    #[cfg(not(any(unix, windows)))]
    {
        name.to_str().is_some_and(|text| {
            text.len() >= PREFIX_LEN + SUFFIX.len()
                && text.starts_with("oplog--")
                && text.as_bytes().ends_with(SUFFIX)
        })
    }
}
const PREFIX_LEN: usize = "oplog--".len();

/// Each day health directory's runtime logs, and whether a leaf of the same
/// shape there is owner content. Memory is one directory's selected names.
/// Links are never followed, and restic does not descend them either.
fn visit_runtime_log_days(
    root: &JournalRoot,
    mut visit: impl FnMut(&str, bool, &[String]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    #[cfg(unix)]
    use crate::flat_directory::{
        open_flat_directory_bound as open_directory, visit_flat_directory_matching as visit_entries,
    };
    #[cfg(windows)]
    use crate::windows_sync_dir::{
        open_windows_flat_directory_bound as open_directory,
        visit_windows_flat_directory_matching as visit_entries,
    };
    root.revalidate_canonical_binding()
        .map_err(std::io::Error::other)?;
    let Some(chronicle) = open_directory(root, OsStr::new(CHRONICLE_DIR), root.canonical_path())
        .map_err(std::io::Error::other)?
    else {
        return Ok(());
    };
    let mut logs = Vec::new();
    visit_entries(&chronicle, |name| name.to_str().is_some_and(is_day_key), |entry| {
        if entry.kind != JournalEntryKind::Directory { return Ok(()); }
        let day = entry.name.to_str().expect("selected ASCII day");
        let failure = |error| FlatDirectoryError::Io {
            operation: "select runtime copy logs", path: root.canonical_path().join("chronicle").join(day), source: error,
        };
        let day_root = JournalRoot::open(root.canonical_path()).map_err(|e| failure(std::io::Error::other(e)))?;
        if day_root.identity() != root.identity() { return Err(failure(std::io::Error::other("journal changed while selecting logs"))); }
        let Some(health) = open_existing_day_health_directory(day_root, day).map_err(|e| failure(std::io::Error::other(e)))? else { return Ok(()); };
        logs.clear();
        let mut owner_lookalike = false;
        visit_entries(health.health(), has_runtime_log_shape, |entry| {
            let runtime = entry.kind == JournalEntryKind::RegularFile
                && matches!(classify_oplog_name(&entry.name), OplogNameClassification::Candidate(Ok(parsed)) if parsed.format() == super::name::OplogFormat::Log);
            if runtime {
                logs.push(format!("chronicle/{day}/health/{}", entry.name.to_str().expect("canonical ASCII log")));
            } else {
                owner_lookalike = true;
            }
            Ok(())
        })?;
        health.revalidate_binding().map_err(|e| failure(std::io::Error::other(e)))?;
        visit(day, owner_lookalike, &logs).map_err(failure)
    }).map_err(std::io::Error::other)?;
    let current = open_directory(root, OsStr::new(CHRONICLE_DIR), root.canonical_path())
        .map_err(std::io::Error::other)?
        .ok_or_else(|| std::io::Error::other("chronicle disappeared while selecting logs"))?;
    if current.identity() != chronicle.identity() {
        return Err(std::io::Error::other(
            "chronicle changed while selecting logs",
        ));
    }
    root.revalidate_canonical_binding()
        .map_err(std::io::Error::other)
}

fn map_directory_error(error: FlatDirectoryError, day: &str) -> OplogCatalogError {
    let kind = match error {
        FlatDirectoryError::IdentityChanged { .. } => "oplog_catalog_identity_changed",
        FlatDirectoryError::EnumerationChanged { .. } => "oplog_catalog_enumeration_changed",
        FlatDirectoryError::Io { .. } => "oplog_catalog_io",
        FlatDirectoryError::InvalidRelativePath { .. }
        | FlatDirectoryError::InvalidName { .. }
        | FlatDirectoryError::NotDirectory { .. }
        | FlatDirectoryError::SymlinkRefused { .. }
        | FlatDirectoryError::NotRegular { .. }
        | FlatDirectoryError::SizeLimitExceeded { .. } => "oplog_catalog_unsafe",
    };
    OplogCatalogError::new(kind, Some(day))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::{FixedOffset, TimeZone};
    use tempfile::TempDir;

    use super::*;
    use crate::operational_log::{OplogFormat, create_oplog_at};

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 7).unwrap()
    }

    fn instant() -> chrono::DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
            .single()
            .unwrap()
    }

    fn snapshot(temporary: &TempDir) -> Result<OplogCatalogSnapshot, OplogCatalogError> {
        catalog_oplogs(JournalRoot::open(temporary.path()).unwrap(), &[day()])
    }

    fn health(temporary: &TempDir) -> std::path::PathBuf {
        temporary.path().join("chronicle/20260807/health")
    }

    fn create(temporary: &TempDir) -> String {
        let writer = create_oplog_at(
            JournalRoot::open(temporary.path()).unwrap(),
            "source",
            "run",
            OplogFormat::Log,
            instant(),
        )
        .unwrap();
        let leaf = writer.leaf_name().to_owned();
        drop(writer);
        leaf
    }

    fn with_forced_unstable<T>(attempts: usize, operation: impl FnOnce() -> T) -> (T, usize) {
        FORCED_UNSTABLE_ATTEMPTS.with(|cell| cell.set(attempts));
        CATALOG_ONCE_CALLS.with(|cell| cell.set(0));
        let result = operation();
        let calls = CATALOG_ONCE_CALLS.with(Cell::get);
        FORCED_UNSTABLE_ATTEMPTS.with(|cell| cell.set(0));
        (result, calls)
    }

    fn with_forced_unstable_after_entries<T>(attempts: usize, operation: impl FnOnce() -> T) -> T {
        FORCE_UNSTABLE_AFTER_ENTRIES.with(|cell| cell.set(attempts));
        let result = operation();
        FORCE_UNSTABLE_AFTER_ENTRIES.with(|cell| cell.set(0));
        result
    }

    fn with_after_initial_list<T>(
        action: impl FnOnce() + 'static,
        operation: impl FnOnce() -> T,
    ) -> T {
        AFTER_INITIAL_LIST.with(|hook| hook.replace(Some(Box::new(action))));
        let result = operation();
        AFTER_INITIAL_LIST.with(|hook| hook.replace(None));
        result
    }

    #[test]
    fn missing_components_are_empty_and_do_not_create_anything() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(snapshot(&temporary).unwrap().entries().is_empty());
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);

        fs::create_dir(temporary.path().join("chronicle")).unwrap();
        assert!(snapshot(&temporary).unwrap().entries().is_empty());
        assert_eq!(
            fs::read_dir(temporary.path().join("chronicle"))
                .unwrap()
                .count(),
            0
        );

        fs::create_dir_all(temporary.path().join("chronicle/20260807")).unwrap();
        assert!(snapshot(&temporary).unwrap().entries().is_empty());
        assert_eq!(
            fs::read_dir(temporary.path().join("chronicle/20260807"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn unrelated_names_are_ignored_but_reserved_malformed_and_unsafe_names_fail() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(health(&temporary)).unwrap();
        fs::write(health(&temporary).join("unrelated.log"), b"ignored").unwrap();
        assert!(snapshot(&temporary).unwrap().entries().is_empty());

        fs::write(health(&temporary).join("oplog--broken.log"), b"bad").unwrap();
        assert_eq!(
            snapshot(&temporary).unwrap_err().kind(),
            "oplog_catalog_malformed"
        );
        fs::remove_file(health(&temporary).join("oplog--broken.log")).unwrap();

        let leaf = create(&temporary);
        fs::remove_file(health(&temporary).join(&leaf)).unwrap();
        fs::create_dir(health(&temporary).join(&leaf)).unwrap();
        assert_eq!(
            snapshot(&temporary).unwrap_err().kind(),
            "oplog_catalog_unsafe"
        );
    }

    #[test]
    fn non_directory_or_link_ancestors_are_errors_not_absence() {
        let temporary = tempfile::tempdir().unwrap();
        fs::write(temporary.path().join("chronicle"), b"file").unwrap();
        assert_eq!(
            snapshot(&temporary).unwrap_err().kind(),
            "oplog_catalog_unsafe"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_reserved_leaf_is_refused() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let leaf = create(&temporary);
        let target = temporary.path().join("target");
        fs::write(&target, b"target").unwrap();
        fs::remove_file(health(&temporary).join(&leaf)).unwrap();
        symlink(&target, health(&temporary).join(leaf)).unwrap();
        assert_eq!(
            snapshot(&temporary).unwrap_err().kind(),
            "oplog_catalog_unsafe"
        );
    }

    #[test]
    fn unstable_census_retries_at_most_four_times() {
        let temporary = tempfile::tempdir().unwrap();
        let (eventual, calls) = with_forced_unstable(2, || snapshot(&temporary));
        assert!(eventual.unwrap().entries().is_empty());
        assert_eq!(calls, 3);

        let (exhausted, calls) =
            with_forced_unstable(OPLOG_CATALOG_CENSUS_ATTEMPTS, || snapshot(&temporary));
        assert_eq!(exhausted.unwrap_err().kind(), "oplog_catalog_unstable");
        assert_eq!(calls, OPLOG_CATALOG_CENSUS_ATTEMPTS);
    }

    #[test]
    fn unstable_attempts_drop_retained_descriptors_before_retrying() {
        let temporary = tempfile::tempdir().unwrap();
        for _ in 0..3 {
            create(&temporary);
        }
        LIVE_RETAINED_DESCRIPTORS.with(|count| count.set(0));
        MAX_LIVE_RETAINED_DESCRIPTORS.with(|count| count.set(0));
        let result = with_forced_unstable_after_entries(OPLOG_CATALOG_CENSUS_ATTEMPTS - 1, || {
            snapshot(&temporary)
        })
        .unwrap();
        assert_eq!(result.entries().len(), 3);
        assert_eq!(LIVE_RETAINED_DESCRIPTORS.with(Cell::get), 3);
        assert_eq!(MAX_LIVE_RETAINED_DESCRIPTORS.with(Cell::get), 3);
        drop(result);
        assert_eq!(LIVE_RETAINED_DESCRIPTORS.with(Cell::get), 0);
    }

    #[test]
    fn replacement_between_listing_and_open_retries_without_a_stale_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let leaf = create(&temporary);
        let path = health(&temporary).join(&leaf);
        let original = fs::read(&path).unwrap();
        let replacement = path.clone();
        let result = with_after_initial_list(
            move || {
                fs::remove_file(&replacement).unwrap();
                fs::write(&replacement, &original).unwrap();
            },
            || snapshot(&temporary),
        )
        .unwrap();
        assert_eq!(result.entries().len(), 1);
        assert_eq!(result.entries()[0].leaf(), OsStr::new(&leaf));
    }

    #[test]
    fn configured_entry_bounds_are_complete_or_error() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir_all(health(&temporary)).unwrap();
        for index in 0..OPLOG_CATALOG_MAX_COUNTABLE_ENTRIES_PER_PASS {
            fs::write(health(&temporary).join(format!("unrelated-{index}")), b"x").unwrap();
        }
        assert!(snapshot(&temporary).unwrap().entries().is_empty());
        fs::write(health(&temporary).join("one-too-many"), b"x").unwrap();
        assert_eq!(
            snapshot(&temporary).unwrap_err().kind(),
            "oplog_catalog_countable_limit"
        );
    }

    #[test]
    fn complete_fold_exceeds_snapshot_capacity_with_one_handle_and_no_retry_duplicates() {
        let temporary = TempDir::new().unwrap();
        let count = OPLOG_CATALOG_MAX_CANDIDATES_PER_DAY + 1;
        for _ in 0..count {
            create(&temporary);
        }
        LIVE_RETAINED_DESCRIPTORS.with(|value| value.set(0));
        MAX_LIVE_RETAINED_DESCRIPTORS.with(|value| value.set(0));
        let read = || {
            fold_oplogs(
                JournalRoot::open(temporary.path()).unwrap(),
                &[day()],
                |count: &mut usize, _, input| {
                    let mut text = String::new();
                    input.read_to_string(&mut text)?;
                    assert!(
                        text.is_empty(),
                        "admission header must not reach the reader"
                    );
                    *count += 1;
                    Ok(())
                },
            )
        };
        assert_eq!(with_forced_unstable_after_entries(2, read).unwrap(), count);
        assert_eq!(MAX_LIVE_RETAINED_DESCRIPTORS.with(Cell::get), 1);
        assert_eq!(LIVE_RETAINED_DESCRIPTORS.with(Cell::get), 0);
        std::fs::write(health(&temporary).join("oplog--invalid.log"), "invalid").unwrap();
        assert!(
            read().is_err(),
            "unsafe/incomplete census must not yield a partial count"
        );
        assert_eq!(LIVE_RETAINED_DESCRIPTORS.with(Cell::get), 0);
    }

    #[test]
    #[cfg(feature = "full-tests")]
    fn candidate_limit_accepts_exactly_512_and_opens_none_at_513() {
        #[cfg(unix)]
        {
            use nix::sys::resource::{Resource, getrlimit, setrlimit};

            let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE).unwrap();
            let required = (OPLOG_CATALOG_MAX_CANDIDATES_PER_DAY * 2) as u64;
            if soft < required {
                assert!(
                    hard >= required,
                    "catalog fixture needs {required} descriptors"
                );
                setrlimit(Resource::RLIMIT_NOFILE, required, hard).unwrap();
            }
        }
        let temporary = tempfile::tempdir().unwrap();
        for _ in 0..OPLOG_CATALOG_MAX_CANDIDATES_PER_DAY {
            let _ = create(&temporary);
        }
        CATALOG_ENTRY_OPEN_CALLS.with(|count| count.set(0));
        let at_limit = snapshot(&temporary).unwrap();
        assert_eq!(
            at_limit.entries().len(),
            OPLOG_CATALOG_MAX_CANDIDATES_PER_DAY
        );
        assert_eq!(
            CATALOG_ENTRY_OPEN_CALLS.with(Cell::get),
            OPLOG_CATALOG_MAX_CANDIDATES_PER_DAY
        );
        drop(at_limit);

        let _ = create(&temporary);
        CATALOG_ENTRY_OPEN_CALLS.with(|count| count.set(0));
        assert_eq!(
            snapshot(&temporary).unwrap_err().kind(),
            "oplog_catalog_candidate_limit"
        );
        assert_eq!(CATALOG_ENTRY_OPEN_CALLS.with(Cell::get), 0);
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn copy_log_selection_streams_regular_logs_and_keeps_owner_lookalikes() {
        let temporary = TempDir::new().unwrap();
        let directory = health(&temporary);
        fs::create_dir_all(&directory).unwrap();
        let log = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log";
        fs::write(directory.join(log), "runtime").unwrap();
        fs::write(
            directory.join(log.replace(".log", ".jsonl")),
            "owner history",
        )
        .unwrap();
        fs::write(directory.join("oplog--not-a-name.log"), "owner original").unwrap();
        fs::create_dir(directory.join(log.replace("8f03cabe", "8f03cabf"))).unwrap();
        // A capture stream named health may have more entries than the diagnostic catalog.
        for n in 0..4100 {
            fs::create_dir(directory.join(format!("{n:06}_60"))).unwrap();
        }
        let root = JournalRoot::open(temporary.path()).unwrap();
        let mut selected = Vec::new();
        visit_runtime_log_paths(&root, |member| {
            selected.push(member.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(selected, vec![format!("chronicle/20260807/health/{log}")]);
        assert_eq!(
            fs::read_to_string(directory.join("oplog--not-a-name.log")).unwrap(),
            "owner original"
        );
    }

    #[cfg(all(feature = "full-tests", unix))]
    #[test]
    fn copy_log_selection_never_selects_through_symlinked_ancestors_or_leaves() {
        use std::os::unix::fs::symlink;
        for relative in [
            "chronicle",
            "chronicle/20260807",
            "chronicle/20260807/health",
        ] {
            let temporary = TempDir::new().unwrap();
            let outside = TempDir::new().unwrap();
            let link = temporary.path().join(relative);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            fs::write(outside.path().join("owner.txt"), "untouched").unwrap();
            symlink(outside.path(), &link).unwrap();
            let root = JournalRoot::open(temporary.path()).unwrap();
            let mut selected = Vec::new();
            let _ = visit_runtime_log_paths(&root, |member| {
                selected.push(member.to_owned());
                Ok(())
            });
            assert!(selected.is_empty(), "{relative}");
            assert_eq!(
                fs::read_to_string(outside.path().join("owner.txt")).unwrap(),
                "untouched"
            );
        }
        let temporary = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let directory = health(&temporary);
        fs::create_dir_all(&directory).unwrap();
        let target = outside.path().join("owner.txt");
        fs::write(&target, "untouched").unwrap();
        let log = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log";
        symlink(&target, directory.join(log)).unwrap();
        let root = JournalRoot::open(temporary.path()).unwrap();
        let mut selected = Vec::new();
        visit_runtime_log_paths(&root, |member| {
            selected.push(member.to_owned());
            Ok(())
        })
        .unwrap();
        assert!(selected.is_empty());
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn copy_log_exclusions_are_one_shape_until_a_day_holds_an_owner_lookalike() {
        let log = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log";
        let temporary = TempDir::new().unwrap();
        for day in ["20260807", "20260808"] {
            let directory = temporary.path().join(format!("chronicle/{day}/health"));
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(log), "runtime").unwrap();
            fs::write(directory.join(log.replace(".log", ".jsonl")), "owner").unwrap();
        }
        // Outside a day, or below the health directory, the shape is not matched.
        for relative in [
            "chronicle/notes/health",
            "chronicle/20260807/health/120000_60",
        ] {
            let directory = temporary.path().join(relative);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("oplog--not-a-name.log"), "owner").unwrap();
        }
        let root = JournalRoot::open(temporary.path()).unwrap();
        let exclusions = |root: &JournalRoot| {
            let mut patterns = Vec::new();
            visit_runtime_log_exclusions(root, |pattern| {
                patterns.push(pattern.to_owned());
                Ok(())
            })
            .unwrap();
            patterns.sort();
            patterns
        };
        assert_eq!(exclusions(&root), vec![RUNTIME_LOG_SHAPE.to_owned()]);

        let lookalikes: [&dyn Fn(&std::path::Path); 3] = [
            &|directory| fs::write(directory.join("oplog--not-a-name.log"), "owner").unwrap(),
            &|directory| {
                fs::create_dir(directory.join(log.replace("8f03cabe", "8f03cabf"))).unwrap()
            },
            #[cfg(unix)]
            &|directory| {
                std::os::unix::fs::symlink(
                    "outside",
                    directory.join(log.replace("8f03cabe", "8f03cab0")),
                )
                .unwrap()
            },
            #[cfg(not(unix))]
            &|directory| fs::create_dir(directory.join("oplog--.log")).unwrap(),
        ];
        for lookalike in lookalikes {
            let temporary = TempDir::new().unwrap();
            for day in ["20260807", "20260808"] {
                let directory = temporary.path().join(format!("chronicle/{day}/health"));
                fs::create_dir_all(&directory).unwrap();
                fs::write(directory.join(log), "runtime").unwrap();
            }
            lookalike(&temporary.path().join("chronicle/20260808/health"));
            let root = JournalRoot::open(temporary.path()).unwrap();
            assert_eq!(
                exclusions(&root),
                vec![
                    "chronicle/20260807/health/oplog--*.log".to_owned(),
                    format!("chronicle/20260808/health/{log}"),
                ]
            );
        }
    }

    #[test]
    fn runtime_log_shape_admits_what_restics_glob_matches() {
        for (name, shaped) in [
            ("oplog--.log", true),
            ("oplog--not-a-name.log", true),
            ("oplog--x.jsonl", false),
            ("oplog-.log", false),
            ("oplog--log", false),
            ("Oplog--x.log", false),
            ("oplog--x.LOG", false),
            ("notes.log", false),
        ] {
            assert_eq!(has_runtime_log_shape(OsStr::new(name)), shaped, "{name}");
        }
    }
}
