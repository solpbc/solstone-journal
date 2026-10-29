// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Sole retained stage/intent namespace observer. Raw bytes are evidence only;
//! the owning domain decodes intent semantics. Sequential walks exclude no creators.

use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::journal_root::{JournalEntryKind, JournalRoot};
use crate::media::{MediaDirectory, MediaEntry, MediaIdentity, RootCarrierObservation};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryBudget {
    DirectoryEntries,
    Directories,
    TotalEntries,
    IntentBytes,
    Obligations,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryObservationFailure {
    Io,
    Changed,
    /// New directories appeared without replacing any previously listed entry.
    /// A caller may repeat the whole census; this is never complete evidence.
    DirectoryAdded,
    NotDirectory,
    Budget(RecoveryBudget),
}

#[derive(Debug)]
pub struct RecoveryObservationError {
    pub failure: RecoveryObservationFailure,
    pub relative: PathBuf,
    pub source: Option<io::Error>,
}

impl fmt::Display for RecoveryObservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "recovery observation {:?} at {}",
            self.failure,
            self.relative.display()
        )
    }
}

impl std::error::Error for RecoveryObservationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|error| error as _)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntentRead {
    UnknownEntry,
    NotPlainFile,
    Oversized,
    Complete {
        operation_id: String,
        bytes: Vec<u8>,
    },
}

/// Raw namespace evidence. No variant grants IO or settlement authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeObligation {
    Stage {
        relative: PathBuf,
        entry: MediaEntry,
    },
    Intent {
        relative: PathBuf,
        entry: MediaEntry,
        contents: IntentRead,
    },
}

#[derive(Debug)]
pub struct NativeObligations {
    entries: Vec<NativeObligation>,
}

/// Available evidence from an incomplete sole walk. It is never a census.
pub struct IncompleteRecoveryObservation {
    root: Arc<JournalRoot>,
    entries: Vec<NativeObligation>,
    encountered: Option<(PathBuf, MediaEntry)>,
    error: RecoveryObservationError,
}

impl fmt::Debug for IncompleteRecoveryObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IncompleteRecoveryObservation")
            .field("entries", &self.entries)
            .field("encountered", &self.encountered)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl IncompleteRecoveryObservation {
    pub fn root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn entries(&self) -> &[NativeObligation] {
        &self.entries
    }
    pub fn encountered(&self) -> Option<(&Path, &MediaEntry)> {
        self.encountered
            .as_ref()
            .map(|(path, entry)| (path.as_path(), entry))
    }
    pub fn error(&self) -> &RecoveryObservationError {
        &self.error
    }
}

impl NativeObligations {
    pub fn entries(&self) -> &[NativeObligation] {
        &self.entries
    }
    pub fn into_entries(self) -> Vec<NativeObligation> {
        self.entries
    }
}

/// Complete evidence retaining a borrow of the actual live original owner.
/// Recognition excludes no creator and grants no settlement or cleanup authority.
pub struct CarrierObligations<'a> {
    _carrier: RootCarrierObservation<'a>,
    all: NativeObligations,
    owned_index: usize,
}

impl CarrierObligations<'_> {
    pub fn entries(&self) -> &[NativeObligation] {
        self.all.entries()
    }

    pub fn unresolved_entries(&self) -> impl Iterator<Item = &NativeObligation> {
        self.all
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| (index != self.owned_index).then_some(entry))
    }
}

/// Storage cap shared directly by the native reader and owning strict schema.
pub const MAX_INTENT_BYTES: usize = 1024 * 1024;

fn valid_operation(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Derive a plain storage filename. No path is supplied or followed.
pub fn intent_filename(operation_id: &str) -> Option<std::ffi::OsString> {
    valid_operation(operation_id).then(|| format!("{operation_id}.json").into())
}

fn operation_from_name(name: &OsStr) -> Option<&str> {
    name.to_str()?
        .strip_suffix(".json")
        .filter(|id| valid_operation(id))
}

#[derive(Clone, Copy)]
struct Limits {
    entries: usize,
    directories: usize,
    total_entries: usize,
    intent_bytes: usize,
    obligations: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: 65_536,
            directories: 131_072,
            total_entries: 4_194_304,
            intent_bytes: 16_777_216,
            obligations: 4_096,
        }
    }
}

impl Limits {
    fn maximum(self, budget: RecoveryBudget) -> usize {
        match budget {
            RecoveryBudget::DirectoryEntries => self.entries,
            RecoveryBudget::Directories => self.directories,
            RecoveryBudget::TotalEntries => self.total_entries,
            RecoveryBudget::IntentBytes => self.intent_bytes,
            RecoveryBudget::Obligations => self.obligations,
        }
    }
}

#[derive(Default)]
struct Budget {
    directories: usize,
    entries: usize,
    intent_bytes: usize,
    obligations: usize,
}

fn error(failure: RecoveryObservationFailure, relative: &Path) -> RecoveryObservationError {
    RecoveryObservationError {
        failure,
        relative: relative.into(),
        source: None,
    }
}

fn io_error(relative: &Path, source: io::Error) -> RecoveryObservationError {
    RecoveryObservationError {
        failure: RecoveryObservationFailure::Io,
        relative: relative.into(),
        source: Some(source),
    }
}

fn charge(
    used: &mut usize,
    amount: usize,
    maximum: usize,
    budget: RecoveryBudget,
    relative: &Path,
) -> Result<(), RecoveryObservationError> {
    *used = used
        .checked_add(amount)
        .filter(|value| *value <= maximum)
        .ok_or_else(|| error(RecoveryObservationFailure::Budget(budget), relative))?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Role {
    Root,
    Chronicle(u8),
    Health,
    Offload,
    Intents,
}

/// Required children each role enters by name.
fn frontiers(role: Role) -> &'static [(&'static str, Role)] {
    match role {
        Role::Root => &[("chronicle", Role::Chronicle(0)), ("health", Role::Health)],
        Role::Health => &[("offload", Role::Offload)],
        Role::Offload => &[("restore-intents", Role::Intents)],
        Role::Chronicle(_) | Role::Intents => &[],
    }
}

/// What a directory's final listing must still match. A live journal keeps
/// writing while the walk runs, so the size and times of ordinary entries are
/// not evidence: only stages, intents and the identity of each entered child.
#[derive(PartialEq)]
enum Relisted<'a> {
    Evidence(&'a MediaEntry),
    Entered(&'a OsStr, JournalEntryKind, MediaIdentity),
}

impl<'a> Relisted<'a> {
    fn name(&self) -> &'a OsStr {
        match self {
            Self::Evidence(entry) => entry.name.as_os_str(),
            Self::Entered(name, _, _) => name,
        }
    }
}

fn relisted(entries: &[MediaEntry], role: Role) -> Vec<Relisted<'_>> {
    entries
        .iter()
        .filter_map(|entry| {
            if matches!(role, Role::Intents) || is_stage_namespace_name(&entry.name) {
                Some(Relisted::Evidence(entry))
            } else if (matches!(role, Role::Chronicle(0..=1))
                && entry.kind != JournalEntryKind::RegularFile)
                || frontiers(role)
                    .iter()
                    .any(|(name, _)| entry.name == OsStr::new(name))
            {
                Some(Relisted::Entered(&entry.name, entry.kind, entry.identity))
            } else {
                None
            }
        })
        .collect()
}

fn listing_failure(
    before: &[MediaEntry],
    after: &[MediaEntry],
    role: Role,
) -> Option<RecoveryObservationFailure> {
    let before = relisted(before, role);
    let after = relisted(after, role);
    if before == after {
        return None;
    }
    let before: std::collections::BTreeMap<_, _> =
        before.iter().map(|entry| (entry.name(), entry)).collect();
    let after: std::collections::BTreeMap<_, _> =
        after.iter().map(|entry| (entry.name(), entry)).collect();
    let unchanged = before
        .iter()
        .all(|(name, entry)| after.get(name) == Some(entry));
    let only_added_directories = after
        .iter()
        .filter(|(name, _)| !before.contains_key(*name))
        .all(|(_, entry)| matches!(entry, Relisted::Entered(_, JournalEntryKind::Directory, _)));
    Some(
        if unchanged && after.len() > before.len() && only_added_directories {
            RecoveryObservationFailure::DirectoryAdded
        } else {
            RecoveryObservationFailure::Changed
        },
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Checkpoint {
    Listed,
    Read,
    Missing,
    BeforeFinalList,
    FinishedDirectory,
}

struct Reader<F> {
    limits: Limits,
    budget: Budget,
    obligations: Vec<NativeObligation>,
    encountered: Option<(PathBuf, MediaEntry)>,
    checkpoint: F,
}

/// Census stage names in root/chronicle/day/day-child and health/offload,
/// and all entries in health/offload/restore-intents.
/// No directory or file is created, rewritten, removed, followed or repaired.
pub fn observe_obligations(
    root: &Arc<JournalRoot>,
) -> Result<NativeObligations, RecoveryObservationError> {
    observe_with(root, Limits::default(), |_, _| {})
}

/// Classify the reserved stage namespace; this grants no mutation authority.
pub fn is_stage_namespace_name(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(".tmp-offload-"))
}

#[cfg(test)]
mod namespace_name_tests {
    use super::is_stage_namespace_name;
    use std::ffi::OsStr;

    #[test]
    fn classification_preserves_exact_prefix_behavior() {
        for (name, reserved) in [
            (".tmp-offload-", true),
            (".tmp-offload-anything", true),
            (".tmp-offload", false),
            (".removing_.tmp-offload-x", false),
            ("segment.tmp-offload-x", false),
            ("070000_17", false),
        ] {
            assert_eq!(is_stage_namespace_name(OsStr::new(name)), reserved);
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_remains_unclassified() {
        use std::os::unix::ffi::OsStrExt;
        assert!(!is_stage_namespace_name(OsStr::from_bytes(
            b".tmp-offload-\xff"
        )));
    }
}

/// Fresh complete census recognizing only this borrowed live empty root carrier.
/// Every other stage/intent remains unresolved; errors never yield partial success.
pub fn observe_with_carrier<'a>(
    root: &Arc<JournalRoot>,
    carrier: RootCarrierObservation<'a>,
) -> Result<CarrierObligations<'a>, RecoveryObservationError> {
    observe_carrier_with(root, carrier, |_, _| {})
}

fn observe_carrier_with<'a>(
    root: &Arc<JournalRoot>,
    carrier: RootCarrierObservation<'a>,
    checkpoint: impl FnMut(Checkpoint, &Path),
) -> Result<CarrierObligations<'a>, RecoveryObservationError> {
    let before = carrier
        .validate(root)
        .map_err(|source| io_error(Path::new(""), source))?;
    let all = match observe_retained_with(root, Limits::default(), checkpoint) {
        Ok(all) => all,
        Err(mut failure) => {
            if failure.error.failure == RecoveryObservationFailure::DirectoryAdded {
                let after = carrier
                    .validate(root)
                    .map_err(|source| io_error(Path::new(""), source))?;
                let foreign = failure.entries.iter().any(|entry| {
                    !matches!(entry, NativeObligation::Stage { relative, entry }
                        if relative == Path::new(&before.name)
                            && entry.name == before.name
                            && entry.identity == before.identity
                            && entry.kind == JournalEntryKind::Directory)
                });
                if before != after || foreign {
                    failure.error.failure = RecoveryObservationFailure::Changed;
                }
            }
            return Err(failure.error);
        }
    };
    let after = carrier
        .validate(root)
        .map_err(|source| io_error(Path::new(""), source))?;
    if before != after {
        return Err(error(
            RecoveryObservationFailure::Changed,
            Path::new(&before.name),
        ));
    }
    let owned_index = matching_carrier(all.entries(), &before.name, before.identity)?;
    Ok(CarrierObligations {
        _carrier: carrier,
        all,
        owned_index,
    })
}

pub(crate) fn matching_carrier(
    entries: &[NativeObligation],
    name: &OsStr,
    identity: crate::media::MediaIdentity,
) -> Result<usize, RecoveryObservationError> {
    let mut matches = entries
        .iter()
        .enumerate()
        .filter_map(|(index, record)| match record {
            NativeObligation::Stage { relative, entry }
                if relative == Path::new(name)
                    && entry.name == name
                    && entry.identity == identity
                    && entry.kind == JournalEntryKind::Directory =>
            {
                Some(index)
            }
            _ => None,
        });
    let index = matches
        .next()
        .ok_or_else(|| error(RecoveryObservationFailure::Changed, Path::new(name)))?;
    if matches.next().is_some() {
        return Err(error(RecoveryObservationFailure::Changed, Path::new(name)));
    }
    Ok(index)
}

// Private instrumentation cannot return admission or grant mutation authority.
fn observe_with(
    root: &Arc<JournalRoot>,
    limits: Limits,
    checkpoint: impl FnMut(Checkpoint, &Path),
) -> Result<NativeObligations, RecoveryObservationError> {
    observe_retained_with(root, limits, checkpoint).map_err(|failure| failure.error)
}

pub(crate) fn observe_retained(
    root: &Arc<JournalRoot>,
) -> Result<NativeObligations, Box<IncompleteRecoveryObservation>> {
    observe_retained_with(root, Limits::default(), |_, _| {})
}

fn observe_retained_with(
    root: &Arc<JournalRoot>,
    limits: Limits,
    checkpoint: impl FnMut(Checkpoint, &Path),
) -> Result<NativeObligations, Box<IncompleteRecoveryObservation>> {
    let early = |error| {
        Box::new(IncompleteRecoveryObservation {
            root: Arc::clone(root),
            entries: Vec::new(),
            encountered: None,
            error,
        })
    };
    root.revalidate_canonical_binding()
        .map_err(|source| early(io_error(Path::new(""), io::Error::other(source))))?;
    let directory = MediaDirectory::open(Arc::clone(root), Path::new(""))
        .map_err(|source| early(io_error(Path::new(""), source)))?;
    let mut reader = Reader {
        limits,
        budget: Budget::default(),
        obligations: Vec::new(),
        encountered: None,
        checkpoint,
    };
    let result = reader
        .walk(&directory, Path::new(""), Role::Root)
        .and_then(|()| {
            root.revalidate_canonical_binding()
                .map_err(|source| io_error(Path::new(""), io::Error::other(source)))
        });
    if let Err(error) = result {
        return Err(Box::new(IncompleteRecoveryObservation {
            root: Arc::clone(root),
            entries: reader.obligations,
            encountered: reader.encountered,
            error,
        }));
    }
    Ok(NativeObligations {
        entries: reader.obligations,
    })
}

impl<F: FnMut(Checkpoint, &Path)> Reader<F> {
    fn entries(
        &mut self,
        directory: &MediaDirectory,
        relative: &Path,
        role: Role,
    ) -> Result<Vec<MediaEntry>, RecoveryObservationError> {
        // The census classifies every name itself, so a UTF-8 name the portable
        // rules refuse (old logs named with a colon) is not a refusal here. Only
        // stages and intents are evidence, as in `Relisted`, so an ordinary
        // entry's size and times moving between the listing's walks is not a
        // refusal either.
        let entries = directory
            .census_entries(
                self.limits.maximum(RecoveryBudget::DirectoryEntries),
                |entry| matches!(role, Role::Intents) || is_stage_namespace_name(&entry.name),
            )
            .map_err(|source| io_error(relative, source))?
            .ok_or_else(|| {
                error(
                    RecoveryObservationFailure::Budget(RecoveryBudget::DirectoryEntries),
                    relative,
                )
            })?;
        charge(
            &mut self.budget.entries,
            entries.len(),
            self.limits.maximum(RecoveryBudget::TotalEntries),
            RecoveryBudget::TotalEntries,
            relative,
        )?;
        Ok(entries)
    }

    fn add(
        &mut self,
        obligation: NativeObligation,
        relative: &Path,
    ) -> Result<(), RecoveryObservationError> {
        charge(
            &mut self.budget.obligations,
            1,
            self.limits.maximum(RecoveryBudget::Obligations),
            RecoveryBudget::Obligations,
            relative,
        )?;
        self.obligations.push(obligation);
        Ok(())
    }

    fn child(
        &mut self,
        directory: &MediaDirectory,
        entry: &MediaEntry,
        relative: &Path,
        role: Role,
    ) -> Result<(), RecoveryObservationError> {
        let relative = relative.join(&entry.name);
        self.encountered = Some((relative.clone(), entry.clone()));
        if entry.kind != JournalEntryKind::Directory {
            return Err(error(RecoveryObservationFailure::NotDirectory, &relative));
        }
        let child = directory
            .open_child(&entry.name)
            .map_err(|source| io_error(&relative, source))?
            .ok_or_else(|| error(RecoveryObservationFailure::Changed, &relative))?;
        if child.identity() != entry.identity {
            return Err(error(RecoveryObservationFailure::Changed, &relative));
        }
        self.walk(&child, &relative, role)?;
        self.encountered = None;
        Ok(())
    }

    fn frontier(
        &mut self,
        directory: &MediaDirectory,
        entries: &[MediaEntry],
        relative: &Path,
        name: &str,
        role: Role,
        missing: &mut Vec<String>,
    ) -> Result<(), RecoveryObservationError> {
        if let Some(entry) = entries.iter().find(|entry| entry.name == OsStr::new(name)) {
            return self.child(directory, entry, relative, role);
        }
        let child_relative = relative.join(name);
        if directory
            .open_child(OsStr::new(name))
            .map_err(|source| io_error(&child_relative, source))?
            .is_some()
        {
            return Err(error(
                RecoveryObservationFailure::DirectoryAdded,
                &child_relative,
            ));
        }
        missing.push(name.into());
        (self.checkpoint)(Checkpoint::Missing, &child_relative);
        Ok(())
    }

    fn intent(
        &mut self,
        directory: &MediaDirectory,
        entry: &MediaEntry,
        relative: &Path,
    ) -> Result<(), RecoveryObservationError> {
        let relative = relative.join(&entry.name);
        self.encountered = Some((relative.clone(), entry.clone()));
        let contents = match operation_from_name(&entry.name) {
            None => IntentRead::UnknownEntry,
            Some(_) if entry.kind != JournalEntryKind::RegularFile => IntentRead::NotPlainFile,
            Some(_) if entry.size > MAX_INTENT_BYTES as u64 => IntentRead::Oversized,
            Some(operation) => {
                charge(
                    &mut self.budget.intent_bytes,
                    entry.size as usize,
                    self.limits.maximum(RecoveryBudget::IntentBytes),
                    RecoveryBudget::IntentBytes,
                    &relative,
                )?;
                let read = directory
                    .read_file_bounded(&entry.name, MAX_INTENT_BYTES)
                    .map_err(|source| io_error(&relative, source))?
                    .ok_or_else(|| error(RecoveryObservationFailure::Changed, &relative))?;
                if !read.complete || read.entry != *entry {
                    return Err(error(RecoveryObservationFailure::Changed, &relative));
                }
                (self.checkpoint)(Checkpoint::Read, &relative);
                IntentRead::Complete {
                    operation_id: operation.into(),
                    bytes: read.bytes,
                }
            }
        };
        self.add(
            NativeObligation::Intent {
                relative: relative.clone(),
                entry: entry.clone(),
                contents,
            },
            &relative,
        )?;
        self.encountered = None;
        Ok(())
    }

    fn walk(
        &mut self,
        directory: &Arc<MediaDirectory>,
        relative: &Path,
        role: Role,
    ) -> Result<(), RecoveryObservationError> {
        charge(
            &mut self.budget.directories,
            1,
            self.limits.maximum(RecoveryBudget::Directories),
            RecoveryBudget::Directories,
            relative,
        )?;
        let entries = self.entries(directory, relative, role)?;
        (self.checkpoint)(Checkpoint::Listed, relative);
        let mut missing = Vec::new();
        for entry in &entries {
            if matches!(role, Role::Intents) {
                self.intent(directory, entry, relative)?;
            } else if is_stage_namespace_name(&entry.name) {
                let stage_relative = relative.join(&entry.name);
                self.encountered = Some((stage_relative.clone(), entry.clone()));
                self.add(
                    NativeObligation::Stage {
                        relative: stage_relative.clone(),
                        entry: entry.clone(),
                    },
                    &stage_relative,
                )?;
                self.encountered = None;
            } else if let Role::Chronicle(depth @ 0..=1) = role {
                match entry.kind {
                    JournalEntryKind::Directory => {
                        self.child(directory, entry, relative, Role::Chronicle(depth + 1))?
                    }
                    JournalEntryKind::RegularFile => {}
                    _ => {
                        return Err(error(
                            RecoveryObservationFailure::NotDirectory,
                            &relative.join(&entry.name),
                        ));
                    }
                }
            }
        }
        for &(name, child_role) in frontiers(role) {
            self.frontier(
                directory,
                &entries,
                relative,
                name,
                child_role,
                &mut missing,
            )?;
        }
        (self.checkpoint)(Checkpoint::BeforeFinalList, relative);
        if let Some(failure) =
            listing_failure(&entries, &self.entries(directory, relative, role)?, role)
        {
            return Err(error(failure, relative));
        }
        for name in missing {
            let child_relative = relative.join(&name);
            if directory
                .open_child(OsStr::new(&name))
                .map_err(|source| io_error(&child_relative, source))?
                .is_some()
            {
                return Err(error(
                    RecoveryObservationFailure::DirectoryAdded,
                    &child_relative,
                ));
            }
        }
        directory
            .revalidate()
            .map_err(|source| io_error(relative, source))?;
        (self.checkpoint)(Checkpoint::FinishedDirectory, relative);
        Ok(())
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn production_limits_and_storage_grammar_match_the_native_contract() {
        let limits = Limits::default();
        assert_eq!(limits.entries, 65_536);
        assert_eq!(limits.directories, 131_072);
        assert_eq!(limits.total_entries, 4_194_304);
        assert_eq!(limits.intent_bytes, 16_777_216);
        assert_eq!(limits.obligations, 4_096);
        assert_eq!(MAX_INTENT_BYTES, 1_048_576);
        let operation = "0123456789abcdef0123456789abcdef";
        let filename = intent_filename(operation).unwrap();
        assert_eq!(operation_from_name(&filename), Some(operation));
        for invalid in [
            "",
            "..",
            "../escape",
            "0123456789abcdef0123456789abcdeF",
            "000000000000000000000000000000000",
        ] {
            assert!(intent_filename(invalid).is_none());
        }
    }

    #[test]
    fn actual_production_budget_limits_accept_exactly_and_refuse_one_more() {
        let limits = Limits::default();
        for budget in [
            RecoveryBudget::DirectoryEntries,
            RecoveryBudget::Directories,
            RecoveryBudget::TotalEntries,
            RecoveryBudget::IntentBytes,
            RecoveryBudget::Obligations,
        ] {
            let maximum = limits.maximum(budget);
            let mut used = 0;
            charge(&mut used, maximum, maximum, budget, Path::new("fixture")).unwrap();
            assert_eq!(used, maximum);
            assert_eq!(
                charge(&mut used, 1, maximum, budget, Path::new("fixture"))
                    .unwrap_err()
                    .failure,
                RecoveryObservationFailure::Budget(budget)
            );
        }
        let mut used = usize::MAX;
        assert!(
            charge(
                &mut used,
                1,
                usize::MAX,
                RecoveryBudget::TotalEntries,
                Path::new("")
            )
            .is_err()
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        temp: tempfile::TempDir,
        root: Arc<JournalRoot>,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
                Some(root) => tempfile::tempdir_in(root).unwrap(),
                None => tempfile::tempdir().unwrap(),
            };
            let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
            Self { temp, root }
        }
        fn path(&self, relative: &str) -> PathBuf {
            self.temp.path().join(relative)
        }
        fn directory(&self, relative: &str) {
            fs::create_dir_all(self.path(relative)).unwrap();
        }
        fn observed(&self) -> Result<NativeObligations, RecoveryObservationError> {
            observe_obligations(&self.root)
        }
        fn intents(&self) -> PathBuf {
            self.directory("health/offload/restore-intents");
            self.path("health/offload/restore-intents")
        }
    }

    #[test]
    fn incomplete_walk_retains_prior_stage_and_listed_intent_before_read_or_budget_failure() {
        let f = Fixture::new();
        f.directory(".tmp-offload-prior");
        let name = format!("{}.json", "a".repeat(32));
        let intent = f.intents().join(&name);
        fs::write(&intent, b"saved bytes").unwrap();
        let relative = Path::new("health/offload/restore-intents").join(&name);
        let before = observe_obligations(&f.root).unwrap().into_entries();
        let failure = observe_retained_with(&f.root, Limits::default(), |point, path| {
            if point == Checkpoint::Listed && path == Path::new("health/offload/restore-intents") {
                fs::remove_file(&intent).unwrap();
            }
        })
        .unwrap_err();
        assert_eq!(failure.entries(), &before[..1]);
        let (path, entry) = failure.encountered().unwrap();
        assert_eq!(path, relative);
        let NativeObligation::Intent {
            entry: expected, ..
        } = &before[1]
        else {
            panic!()
        };
        assert_eq!(entry, expected);
        assert_eq!(failure.error().relative, relative);
        assert_eq!(failure.error().failure, RecoveryObservationFailure::Changed);
        fs::write(&intent, b"saved bytes").unwrap();
        let failure = observe_retained_with(
            &f.root,
            Limits {
                intent_bytes: 0,
                ..Limits::default()
            },
            |_, _| {},
        )
        .unwrap_err();
        assert_eq!(failure.entries().len(), 1);
        assert_eq!(failure.encountered().unwrap().0, relative);
        assert_eq!(
            failure.error().failure,
            RecoveryObservationFailure::Budget(RecoveryBudget::IntentBytes)
        );
    }

    #[test]
    fn census_completes_while_a_live_journal_writes_into_listed_directories() {
        use crate::media::{MediaPrimitive, run_with_media_hook};
        let f = Fixture::new();
        f.directory("indexer");
        f.directory("chronicle/20260101/stream/.tmp-offload-unrelated");
        let indexer = f.path("indexer");
        // A file created inside `indexer/` moves the root entry's times between
        // the root listing's two walks, as a running journal's writers do.
        let (observed, fired) = run_with_media_hook(
            MediaPrimitive::AfterFirstEnumeration,
            move || fs::write(indexer.join("written"), b"live"),
            || f.observed(),
        );
        assert!(fired);
        let observed = observed.unwrap();
        assert_eq!(observed.entries().len(), 1);
        assert!(
            matches!(&observed.entries()[0], NativeObligation::Stage { relative, .. }
            if relative == Path::new("chronicle/20260101/stream/.tmp-offload-unrelated"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn borrowed_activity_recognition_keeps_all_evidence_and_unrelated_stage_and_intent() {
        use crate::activity::ActivityLease;
        use crate::media::RootActivityCarrier;
        let f = Fixture::new();
        let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
        let carrier = RootActivityCarrier::prepare(activity).unwrap();
        let raw = f.observed().unwrap();
        assert_eq!(raw.entries().len(), 1);
        let recognized = observe_with_carrier(&f.root, carrier.observation()).unwrap();
        assert_eq!(recognized.entries(), raw.entries());
        assert_eq!(recognized.unresolved_entries().count(), 0);
        let NativeObligation::Stage { entry, .. } = &raw.entries()[0] else {
            panic!("missing own stage")
        };
        assert!(
            matching_carrier(
                &[raw.entries()[0].clone(), raw.entries()[0].clone()],
                &entry.name,
                entry.identity
            )
            .is_err()
        );
        drop(recognized);
        f.directory("chronicle/20260101/stream/.tmp-offload-unrelated");
        let intent = f.intents().join(format!("{}.json", "a".repeat(32)));
        fs::write(&intent, b"raw intent remains a hold").unwrap();
        let recognized = observe_with_carrier(&f.root, carrier.observation()).unwrap();
        assert_eq!(recognized.entries().len(), 3);
        let unresolved: Vec<_> = recognized.unresolved_entries().collect();
        assert_eq!(unresolved.len(), 2);
        assert!(unresolved.iter().any(|entry| matches!(entry, NativeObligation::Stage { relative, .. } if relative == Path::new("chronicle/20260101/stream/.tmp-offload-unrelated"))));
        assert!(unresolved.iter().any(|entry| matches!(entry, NativeObligation::Intent { contents: IntentRead::Complete { bytes, .. }, .. } if bytes == b"raw intent remains a hold")));
        assert_eq!(f.observed().unwrap().entries(), recognized.entries());
        assert_eq!(fs::read(intent).unwrap(), b"raw intent remains a hold");
    }

    #[cfg(unix)]
    #[test]
    fn carrier_recognition_requires_original_root_arc_and_preserves_valid_owner() {
        use crate::activity::ActivityLease;
        use crate::media::RootActivityCarrier;
        let f = Fixture::new();
        let other = Fixture::new();
        let carrier =
            RootActivityCarrier::prepare(ActivityLease::try_acquire(f.root.clone()).unwrap())
                .unwrap();
        let reopened = Arc::new(JournalRoot::open(f.temp.path()).unwrap());
        assert_eq!(reopened.identity(), f.root.identity());
        assert!(observe_with_carrier(&other.root, carrier.observation()).is_err());
        assert!(observe_with_carrier(&reopened, carrier.observation()).is_err());
        assert_eq!(
            observe_with_carrier(&f.root, carrier.observation())
                .unwrap()
                .unresolved_entries()
                .count(),
            0
        );
        carrier.revalidate().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn additive_frontier_is_retryable_only_without_an_observed_foreign_obligation() {
        use crate::activity::ActivityLease;
        use crate::media::RootActivityCarrier;
        for foreign in [false, true] {
            let f = Fixture::new();
            f.directory("chronicle/20260101");
            let carrier = RootActivityCarrier::prepare(
                ActivityLease::try_acquire(Arc::clone(&f.root)).unwrap(),
            )
            .unwrap();
            if foreign {
                f.directory(".tmp-offload-foreign");
            }
            let mut hit = false;
            let result = observe_carrier_with(&f.root, carrier.observation(), |point, relative| {
                if point == Checkpoint::BeforeFinalList
                    && relative == Path::new("chronicle/20260101")
                {
                    f.directory("chronicle/20260101/new-stream");
                    hit = true;
                }
            });
            assert!(hit);
            assert_eq!(
                result.err().unwrap().failure,
                if foreign {
                    RecoveryObservationFailure::Changed
                } else {
                    RecoveryObservationFailure::DirectoryAdded
                }
            );
            let fresh = observe_with_carrier(&f.root, carrier.observation()).unwrap();
            assert_eq!(fresh.unresolved_entries().count(), usize::from(foreign));
            carrier.revalidate().unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn nonempty_missing_moved_and_full_identity_replaced_carrier_never_recognize() {
        use crate::activity::ActivityLease;
        use crate::media::RootActivityCarrier;
        for change in 0..4 {
            let f = Fixture::new();
            let carrier =
                RootActivityCarrier::prepare(ActivityLease::try_acquire(f.root.clone()).unwrap())
                    .unwrap();
            let path = carrier.diagnostic_path();
            let saved = f.path(".tmp-offload-saved-original");
            match change {
                0 => fs::write(path.join("payload"), b"sole content").unwrap(),
                1 => fs::remove_dir(&path).unwrap(),
                2 => fs::rename(&path, &saved).unwrap(),
                _ => {
                    fs::rename(&path, &saved).unwrap();
                    fs::create_dir(&path).unwrap();
                    fs::write(path.join("competitor"), b"preserve").unwrap();
                }
            }
            assert!(observe_with_carrier(&f.root, carrier.observation()).is_err());
            drop(carrier);
            match change {
                0 => assert_eq!(fs::read(path.join("payload")).unwrap(), b"sole content"),
                1 => assert!(!path.exists()),
                2 => assert!(saved.is_dir()),
                _ => {
                    assert!(saved.is_dir());
                    assert_eq!(fs::read(path.join("competitor")).unwrap(), b"preserve");
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn late_carrier_loss_rebind_and_nonempty_change_refuse_after_complete_raw_walk() {
        use crate::activity::ActivityLease;
        use crate::media::RootActivityCarrier;
        for change in 0..3 {
            let f = Fixture::new();
            let carrier =
                RootActivityCarrier::prepare(ActivityLease::try_acquire(f.root.clone()).unwrap())
                    .unwrap();
            let path = carrier.diagnostic_path();
            let saved = f.path(".tmp-offload-saved-original");
            let mut hit = false;
            let observed =
                observe_carrier_with(&f.root, carrier.observation(), |point, relative| {
                    if point == Checkpoint::FinishedDirectory && relative == Path::new("") {
                        match change {
                            0 => fs::remove_dir(&path).unwrap(),
                            1 => {
                                fs::rename(&path, &saved).unwrap();
                                fs::create_dir(&path).unwrap();
                                fs::write(path.join("competitor"), b"preserve").unwrap();
                            }
                            _ => fs::write(path.join("payload"), b"sole content").unwrap(),
                        }
                        hit = true;
                    }
                });
            assert!(hit);
            assert!(observed.is_err());
            match change {
                0 => assert!(!path.exists()),
                1 => {
                    assert!(saved.is_dir());
                    assert_eq!(fs::read(path.join("competitor")).unwrap(), b"preserve");
                }
                _ => assert_eq!(fs::read(path.join("payload")).unwrap(), b"sole content"),
            }
        }
    }

    #[test]
    fn ineligible_raw_intents_are_held_without_read_budget_and_eligible_bytes_are_charged() {
        let f = Fixture::new();
        let directory = f.intents();
        fs::write(
            directory.join("unknown.lock"),
            b"not a recognized intent filename",
        )
        .unwrap();
        fs::create_dir(directory.join(format!("{}.json", "a".repeat(32)))).unwrap();
        fs::File::create(directory.join(format!("{}.json", "b".repeat(32))))
            .unwrap()
            .set_len(MAX_INTENT_BYTES as u64 + 1)
            .unwrap();
        let limits = Limits {
            intent_bytes: 0,
            ..Limits::default()
        };
        let raw = observe_with(&f.root, limits, |_, _| {}).unwrap();
        assert_eq!(raw.entries().len(), 3);
        let mut dispositions = raw.entries().iter().filter_map(|record| match record {
            NativeObligation::Intent { contents, .. } => Some(contents),
            _ => None,
        });
        let values: Vec<_> = dispositions.by_ref().collect();
        assert!(values.contains(&&IntentRead::UnknownEntry));
        assert!(values.contains(&&IntentRead::NotPlainFile));
        assert!(values.contains(&&IntentRead::Oversized));
        assert_eq!(values.len(), 3);
        fs::write(
            directory.join(format!("{}.json", "c".repeat(32))),
            b"eligible nonempty bytes",
        )
        .unwrap();
        assert_eq!(
            observe_with(&f.root, limits, |_, _| {})
                .unwrap_err()
                .failure,
            RecoveryObservationFailure::Budget(RecoveryBudget::IntentBytes)
        );
    }

    #[test]
    fn complete_raw_payload_is_exact_at_16_mib_and_one_more_byte_refuses_all_coverage() {
        let f = Fixture::new();
        let directory = f.intents();
        for index in 0..16_u8 {
            fs::write(
                directory.join(format!("{index:032x}.json")),
                vec![index; 1_048_576],
            )
            .unwrap();
        }
        let observed = f.observed().unwrap();
        let mut payload = 0;
        for record in observed.entries() {
            let NativeObligation::Intent {
                entry,
                contents:
                    IntentRead::Complete {
                        operation_id,
                        bytes,
                    },
                ..
            } = record
            else {
                panic!("missing complete raw payload");
            };
            let index = u8::from_str_radix(operation_id, 16).unwrap();
            assert_eq!(entry.name, format!("{index:032x}.json").as_str());
            assert_eq!(bytes.len(), 1_048_576);
            assert!(bytes.iter().all(|byte| *byte == index));
            payload += bytes.len();
        }
        assert_eq!(observed.entries().len(), 16);
        assert_eq!(payload, 16_777_216);
        drop(observed);
        fs::write(directory.join(format!("{:032x}.json", 16)), b"x").unwrap();
        assert_eq!(
            f.observed().unwrap_err().failure,
            RecoveryObservationFailure::Budget(RecoveryBudget::IntentBytes)
        );
    }

    #[test]
    fn absent_empty_and_ordinary_media_are_complete_without_mutation() {
        let f = Fixture::new();
        assert!(f.observed().unwrap().entries().is_empty());
        assert_eq!(fs::read_dir(f.temp.path()).unwrap().count(), 0);
        f.directory("chronicle/20260101/_default/010000_001");
        f.directory("chronicle/20260101/020000_001");
        fs::write(f.path("chronicle/20260101/020000_001/.hidden.flac"), b"raw").unwrap();
        f.directory("health/offload/restore-intents");
        fs::write(
            f.path("health/offload/20260101.jsonl"),
            b"legacy history belongs to Catalog",
        )
        .unwrap();
        assert!(f.observed().unwrap().entries().is_empty());
        assert_eq!(
            fs::read(f.path("chronicle/20260101/020000_001/.hidden.flac")).unwrap(),
            b"raw"
        );
    }

    #[cfg(unix)]
    #[test]
    fn names_the_portable_rules_refuse_are_classified_not_refused() {
        let f = Fixture::new();
        f.directory("chronicle/20260101/health");
        f.directory("health");
        for file in [
            "chronicle/20260101/health/12:00:00_think.log",
            "chronicle/20260101/notes?.md",
            "chronicle/old*.txt",
            "health/run:1.log",
            "trailing.",
        ] {
            fs::write(f.path(file), b"old").unwrap();
        }
        assert!(f.observed().unwrap().entries().is_empty());

        f.directory("chronicle/20260101/health/.tmp-offload-a:b");
        let held = f.observed().unwrap();
        assert!(matches!(
            held.entries(),
            [NativeObligation::Stage { relative, .. }]
                if relative == Path::new("chronicle/20260101/health/.tmp-offload-a:b")
        ));

        f.directory("chronicle/20260101/stream:1");
        assert_eq!(
            f.observed().unwrap_err().failure,
            RecoveryObservationFailure::Io
        );
        fs::remove_dir(f.path("chronicle/20260101/stream:1")).unwrap();

        use std::os::unix::ffi::OsStrExt;
        let unreadable = std::ffi::OsStr::from_bytes(b"old\xff.log");
        match fs::write(f.path("chronicle/20260101/health").join(unreadable), b"old") {
            Ok(()) => assert_eq!(
                f.observed().unwrap_err().failure,
                RecoveryObservationFailure::Io
            ),
            // APFS refuses this fixture before the observer can see it. The
            // pure Unix classifier still checks the unrepresentable bytes.
            #[cfg(target_os = "macos")]
            Err(error) if error.raw_os_error() == Some(nix::libc::EILSEQ) => {}
            Err(error) => panic!("could not create the invalid-byte fixture: {error}"),
        }
    }

    #[test]
    fn every_stage_kind_and_segment_like_stream_remains_held() {
        let f = Fixture::new();
        for name in [
            ".tmp-offload-root",
            "chronicle/.tmp-offload-chronicle",
            "chronicle/20260101/.tmp-offload-bad-suffix",
            "chronicle/20260101/_default/.tmp-offload-named",
            "chronicle/20260101/000000_300/.tmp-offload-ambiguous",
        ] {
            f.directory(name);
        }
        fs::write(f.path("chronicle/20260101/.tmp-offload-file"), b"only raw").unwrap();
        fs::write(
            f.path("chronicle/20260101/000000_300/.tmp-offload-ambiguous/payload"),
            b"sole raw",
        )
        .unwrap();
        let observed = f.observed().unwrap();
        assert_eq!(observed.entries().len(), 6);
        assert!(
            observed
                .entries()
                .iter()
                .all(|entry| matches!(entry, NativeObligation::Stage { .. }))
        );
        drop(observed);
        assert_eq!(
            fs::read(f.path("chronicle/20260101/000000_300/.tmp-offload-ambiguous/payload"))
                .unwrap(),
            b"sole raw"
        );
        assert_eq!(
            fs::read(f.path("chronicle/20260101/.tmp-offload-file")).unwrap(),
            b"only raw"
        );
    }

    #[test]
    fn missing_frontier_creation_and_late_stage_append_refuse_empty_coverage() {
        for missing in [
            "chronicle",
            "health",
            "health/offload",
            "health/offload/restore-intents",
        ] {
            let f = Fixture::new();
            if let Some(parent) = Path::new(missing).parent() {
                f.directory(parent.to_str().unwrap());
            }
            let mut hit = false;
            let result = observe_with(&f.root, Limits::default(), |point, relative| {
                if point == Checkpoint::Missing && relative == Path::new(missing) {
                    fs::create_dir(f.path(missing)).unwrap();
                    hit = true;
                }
            });
            assert!(hit);
            assert!(result.is_err());
            assert!(f.path(missing).is_dir());
        }
        let f = Fixture::new();
        f.directory("chronicle/20260101/stream");
        let stage = f.path("chronicle/20260101/stream/.tmp-offload-late");
        let result = observe_with(&f.root, Limits::default(), |point, relative| {
            if point == Checkpoint::BeforeFinalList
                && relative == Path::new("chronicle/20260101/stream")
            {
                fs::create_dir(&stage).unwrap();
            }
        });
        assert!(result.is_err());
        assert!(stage.is_dir());
    }

    #[test]
    fn sequential_observations_do_not_exclude_a_later_deep_stage() {
        let f = Fixture::new();
        f.directory("chronicle/20260101/stream");
        let stage = f.path("chronicle/20260101/stream/.tmp-offload-after-final");
        let mut hit = false;
        let observed = observe_with(&f.root, Limits::default(), |point, relative| {
            if point == Checkpoint::FinishedDirectory && relative == Path::new("chronicle/20260101")
            {
                fs::create_dir(&stage).unwrap();
                hit = true;
            }
        })
        .unwrap();
        assert!(hit);
        assert!(observed.entries().is_empty());
        assert!(stage.is_dir());
        assert_eq!(f.observed().unwrap().entries().len(), 1);
    }

    #[test]
    fn ordinary_writes_during_the_walk_complete_but_a_late_stage_or_changed_child_refuses() {
        for late in ["none", "stage", "replaced", "new_child"] {
            let f = Fixture::new();
            f.directory("chronicle/20260101/stream");
            f.directory("health/offload/restore-intents");
            f.directory("indexer");
            fs::write(f.path("journal.log"), b"line").unwrap();
            let mut hit = false;
            let observed = observe_with(&f.root, Limits::default(), |point, relative| {
                if point != Checkpoint::BeforeFinalList {
                    return;
                }
                if relative == Path::new("chronicle") {
                    fs::write(f.path("chronicle/20260101/segment.json"), b"new").unwrap();
                }
                if relative == Path::new("chronicle/20260101") && late == "new_child" {
                    f.directory("chronicle/20260101/unwalked");
                    hit = true;
                }
                if relative == Path::new("") {
                    fs::write(f.path("indexer/index.db-wal"), b"live").unwrap();
                    fs::write(f.path("health/supervisor.json"), b"live").unwrap();
                    fs::write(f.path("journal.log"), b"line\nline").unwrap();
                    f.directory("mcp-endpoint");
                    match late {
                        "stage" => f.directory(".tmp-offload-late"),
                        "replaced" => {
                            fs::rename(f.path("health"), f.path("health.saved")).unwrap();
                            f.directory("health/offload/restore-intents");
                        }
                        _ => {}
                    }
                    hit = true;
                }
            });
            assert!(hit);
            match late {
                "none" => assert!(observed.unwrap().entries().is_empty()),
                "new_child" => assert_eq!(
                    observed.unwrap_err().failure,
                    RecoveryObservationFailure::DirectoryAdded
                ),
                _ => assert_eq!(
                    observed.unwrap_err().failure,
                    RecoveryObservationFailure::Changed
                ),
            }
        }
    }

    #[test]
    fn intent_replacement_growth_shrink_and_removal_refuse_complete_observation() {
        for change in 0..4 {
            let f = Fixture::new();
            let operation = "f".repeat(32);
            let bytes = b"raw intent bytes".to_vec();
            let path = f.intents().join(format!("{}.json", operation));
            fs::write(&path, bytes.clone()).unwrap();
            let mut hit = false;
            let observed = observe_with(&f.root, Limits::default(), |point, _| {
                if point == Checkpoint::Read {
                    match change {
                        0 => {
                            fs::rename(&path, path.with_extension("saved")).unwrap();
                            fs::write(&path, b"competitor").unwrap();
                        }
                        1 => {
                            use std::io::Write;
                            fs::OpenOptions::new()
                                .append(true)
                                .open(&path)
                                .unwrap()
                                .write_all(b" ")
                                .unwrap();
                        }
                        2 => fs::write(&path, b"short").unwrap(),
                        _ => fs::remove_file(&path).unwrap(),
                    }
                    hit = true;
                }
            });
            assert!(hit);
            assert!(observed.is_err());
            if change == 0 {
                assert_eq!(fs::read(&path).unwrap(), b"competitor");
            }
        }
    }

    #[test]
    fn actual_traversal_propagates_every_budget_and_native_read_failure() {
        let f = Fixture::new();
        let operation = "f".repeat(32);
        let bytes = b"raw intent bytes".to_vec();
        let path = f.intents().join(format!("{}.json", operation));
        fs::write(&path, bytes.clone()).unwrap();
        for budget in [
            RecoveryBudget::DirectoryEntries,
            RecoveryBudget::Directories,
            RecoveryBudget::TotalEntries,
            RecoveryBudget::IntentBytes,
            RecoveryBudget::Obligations,
        ] {
            let mut limits = Limits::default();
            match budget {
                RecoveryBudget::DirectoryEntries => limits.entries = 0,
                RecoveryBudget::Directories => limits.directories = 0,
                RecoveryBudget::TotalEntries => limits.total_entries = 0,
                RecoveryBudget::IntentBytes => limits.intent_bytes = 0,
                RecoveryBudget::Obligations => limits.obligations = 0,
            }
            assert_eq!(
                observe_with(&f.root, limits, |_, _| {})
                    .unwrap_err()
                    .failure,
                RecoveryObservationFailure::Budget(budget)
            );
        }
        for replacement in 0..3 {
            let f = Fixture::new();
            let path = f.intents().join(format!("{}.json", operation));
            fs::write(&path, bytes.clone()).unwrap();
            let mut hit = false;
            let observed = observe_with(&f.root, Limits::default(), |cut, relative| {
                if !hit
                    && cut == Checkpoint::Listed
                    && relative == Path::new("health/offload/restore-intents")
                {
                    if replacement == 2 {
                        fs::rename(&path, path.with_extension("saved")).unwrap();
                        fs::write(&path, bytes.clone()).unwrap();
                    } else {
                        fs::remove_file(&path).unwrap();
                        if replacement == 1 {
                            fs::create_dir(&path).unwrap();
                        }
                    }
                    hit = true;
                }
            });
            assert!(hit);
            assert!(observed.is_err());
            match replacement {
                0 => assert!(!path.exists()),
                1 => assert!(path.is_dir()),
                _ => assert_eq!(fs::read(&path).unwrap(), bytes.clone()),
            }
        }
    }

    #[test]
    fn occupied_required_directories_refuse_without_changing_contents() {
        for relative in [
            "chronicle",
            "health",
            "health/offload",
            "health/offload/restore-intents",
        ] {
            let f = Fixture::new();
            if let Some(parent) = Path::new(relative).parent() {
                f.directory(parent.to_str().unwrap());
            }
            fs::write(f.path(relative), b"occupied").unwrap();
            assert_eq!(
                f.observed().unwrap_err().failure,
                RecoveryObservationFailure::NotDirectory
            );
            assert_eq!(fs::read(f.path(relative)).unwrap(), b"occupied");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_stage_holds_but_symlink_traversal_is_uncertain_and_never_followed() {
        let f = Fixture::new();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("sentinel"), b"outside").unwrap();
        f.directory("chronicle/20260101");
        std::os::unix::fs::symlink(
            outside.path(),
            f.path("chronicle/20260101/.tmp-offload-link"),
        )
        .unwrap();
        assert_eq!(f.observed().unwrap().entries().len(), 1);
        std::os::unix::fs::symlink(outside.path(), f.path("chronicle/20260101/stream")).unwrap();
        assert!(f.observed().is_err());
        assert_eq!(
            fs::read(outside.path().join("sentinel")).unwrap(),
            b"outside"
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_and_root_replacement_preserve_competitors_and_refuse() {
        for root_swap in [false, true] {
            let f = Fixture::new();
            f.directory("chronicle/20260101/stream");
            let original = if root_swap {
                f.temp.path().to_path_buf()
            } else {
                f.path("chronicle/20260101")
            };
            let saved = original.with_extension("saved-census");
            let mut hit = false;
            let observed = observe_with(&f.root, Limits::default(), |point, relative| {
                if !hit
                    && point == Checkpoint::Listed
                    && relative == Path::new("chronicle/20260101/stream")
                {
                    fs::rename(&original, &saved).unwrap();
                    fs::create_dir(&original).unwrap();
                    fs::write(original.join("sentinel"), b"replacement").unwrap();
                    hit = true;
                }
            });
            assert!(hit);
            assert!(observed.is_err());
            assert_eq!(fs::read(original.join("sentinel")).unwrap(), b"replacement");
            if root_swap {
                fs::remove_dir_all(saved).unwrap();
            }
        }
    }
}
