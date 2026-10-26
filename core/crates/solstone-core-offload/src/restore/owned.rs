// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owning restore execution and borrowed, independent effect facts.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use solstone_core_backup::archive_readback::{OwnedReadOperation, ReadOperation, ReadbackFailure};
use solstone_core_journal_io::media::{
    Cleanup, Durability, MediaDirectory, PublicationReceipt, RemovalReceipt,
};
use solstone_core_journal_io::paths::{PathOrDay, Segment, iter_segments};
use solstone_core_journal_io::{JournalRoot, LockOptions, check_record_identities};
use solstone_core_retention::archive_catalog::{
    ArchiveWitness, LegacyRestoreBinding, RestoreDisposition, SegmentCatalog, read_complete_catalog,
};
use solstone_core_retention::mark_mutation::{
    BoundMutationOutcome, BoundMutationRefusal, RestoredMarkRefusal,
};

use super::{RestoreResult, base, refused_selection};
use crate::restore_intent_store::transaction::{
    DetachedTransaction, StagedRestore, StagingFailure,
};
use crate::restore_prepared::PreparedRestore;

pub(crate) mod read;
use read::{RestoreRead, check_recovery};
use solstone_core_backup_runtime::engine::QualifiedArchiveRecoveryBinding;

#[cfg(unix)]
pub enum IntentPublication<'a> {
    Create(
        &'a Result<
            solstone_core_journal_io::atomic::ExclusivePublication,
            solstone_core_journal_io::atomic::DetailedAtomicError,
        >,
    ),
    Update(
        &'a Result<
            solstone_core_journal_io::atomic::BoundAtomicOutcome,
            solstone_core_journal_io::atomic::DetailedAtomicError,
        >,
    ),
}

pub struct IntentWriteFacts<'a> {
    #[cfg(unix)]
    pub publication: Option<IntentPublication<'a>>,
    pub named_read_confirmed: bool,
    pub durability: [Option<Durability>; 4],
    pub failures: Vec<String>,
    #[cfg(not(unix))]
    pub(crate) lifetime: std::marker::PhantomData<&'a ()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerEffect {
    AlreadyRecorded,
    Appended { bytes: usize },
    Attempted { written: Option<usize> },
}

pub struct LedgerFacts {
    pub effect: Option<LedgerEffect>,
    /// File, offload directory, health directory, journal root.
    pub durability: [bool; 4],
    pub confirmed: bool,
    pub failures: Vec<String>,
}

pub struct RetirementFacts<'a> {
    pub removal: Option<&'a RemovalReceipt>,
    pub retained_stage: bool,
    pub ancestors: [Durability; 4],
    pub failures: Vec<String>,
    pub retired: bool,
}

pub struct StorePreparationFacts<'a> {
    pub attempted: &'a [&'static str],
    pub durability: [Durability; 4],
    pub confirmed: bool,
    pub failures: Vec<String>,
}

pub struct SettlementCleanupFacts<'a> {
    pub cleanup: Cleanup,
    pub parent_sync: Option<&'a std::io::Result<()>>,
}

pub struct NoEffectCleanupFacts<'a> {
    pub cleanup: Cleanup,
    pub parent_sync: Option<&'a std::io::Result<()>>,
}

#[derive(Debug)]
pub enum RestoreRegisterRefusal {
    Bound(BoundMutationRefusal),
    Restored(RestoredMarkRefusal),
}

pub struct TransactionFacts<'a> {
    pub publication: Option<&'a PublicationReceipt>,
    pub intent_writes: Vec<IntentWriteFacts<'a>>,
    #[cfg(unix)]
    pub marker: Option<
        &'a Result<
            solstone_core_journal_io::health_marker::BoundStreamMarkerPublication,
            solstone_core_journal_io::health_marker::BoundStreamMarkerError,
        >,
    >,
    #[cfg(unix)]
    pub marker_ancestor_sync: &'a [std::io::Result<()>],
    pub ledger: Option<LedgerFacts>,
    pub register: Option<&'a Result<BoundMutationOutcome, RestoreRegisterRefusal>>,
    pub no_effect_cleanup: Option<NoEffectCleanupFacts<'a>>,
    pub settlement_cleanup: Option<SettlementCleanupFacts<'a>>,
    pub retirement: Option<RetirementFacts<'a>>,
    pub operation_id: Option<&'a str>,
    pub store_exclusion_retained: bool,
    pub retained_media: bool,
    pub preparation: Option<StorePreparationFacts<'a>>,
    pub preparation_failures: Vec<String>,
    pub hold: Option<String>,
    /// A report of this process's receipts, never an execution capability.
    pub completed: bool,
}

pub struct StagingFacts<'a> {
    pub hold: String,
    pub retained_stage: bool,
    pub readback: Option<&'a ReadbackFailure>,
}

pub enum FileEvidence<'a> {
    NotAttempted,
    Staging(StagingFacts<'a>),
    Transaction(Box<TransactionFacts<'a>>),
    /// A refused historical review whose probe or binding write is still owned.
    LegacyReview(legacy::LegacyReviewFacts<'a>),
}

pub struct RestoreFileFacts<'a> {
    pub day: &'a str,
    pub stream: &'a str,
    pub segment: &'a str,
    /// Includes the exact destination basename/layout and historical binding.
    pub witness: &'a ArchiveWitness,
    pub disposition: RestoreDisposition,
    pub reason: Option<&'a str>,
    pub files_restored: u64,
    pub bytes_restored: u64,
    pub evidence: FileEvidence<'a>,
}

enum FileOwner {
    None,
    Staging(Box<StagingFailure>),
    Transaction(Box<DetachedTransaction>),
    Review(Box<legacy::ReviewRemnant>),
}

struct FileRecord {
    detail: usize,
    day: String,
    stream: String,
    segment: String,
    witness: ArchiveWitness,
    disposition: RestoreDisposition,
    reason: Option<&'static str>,
    files: u64,
    bytes: u64,
    owner: FileOwner,
}

impl FileRecord {
    fn accept(&mut self, outcome: DetachedTransaction) -> bool {
        let effects = outcome.effects();
        self.files = effects.files;
        self.bytes = effects.bytes;
        self.disposition = outcome.disposition();
        let stopped = !outcome.completed();
        self.reason = if stopped {
            Some("restore_held")
        } else {
            match self.disposition {
                RestoreDisposition::Conflict => Some("restore_conflict"),
                RestoreDisposition::Unattempted => Some("restore_not_published"),
                _ => None,
            }
        };
        self.owner = FileOwner::Transaction(Box::new(outcome));
        stopped
    }
}

#[must_use = "retain the original operation, every native effect and unresolved obligation"]
pub struct RestoreExecution<R> {
    report: RestoreResult,
    _operation: R,
    _binding: Option<QualifiedArchiveRecoveryBinding>,
    files: Vec<FileRecord>,
    legacy: Vec<legacy::LegacyEvidence>,
}

pub type RestoreRunOutcome<'clock> = RestoreExecution<ReadOperation<'clock>>;
/// Active execution remains worker-local; its consuming finish closes every normal return.
pub type OwnedRestoreRunOutcome = RestoreExecution<OwnedReadOperation>;

impl<R> RestoreExecution<R> {
    pub fn report(&self) -> &RestoreResult {
        &self.report
    }

    pub fn files(&self) -> impl ExactSizeIterator<Item = RestoreFileFacts<'_>> {
        self.files.iter().map(|file| RestoreFileFacts {
            day: &file.day,
            stream: &file.stream,
            segment: &file.segment,
            witness: &file.witness,
            disposition: file.disposition,
            reason: file.reason,
            files_restored: file.files,
            bytes_restored: file.bytes,
            evidence: match &file.owner {
                FileOwner::None => FileEvidence::NotAttempted,
                FileOwner::Staging(failure) => FileEvidence::Staging(failure.facts()),
                FileOwner::Transaction(outcome) => {
                    FileEvidence::Transaction(Box::new(outcome.facts()))
                }
                FileOwner::Review(review) => FileEvidence::LegacyReview(review.facts()),
            },
        })
    }

    /// Historical bindings this execution confirmed in the ledger before reading.
    pub fn legacy_bindings(&self) -> impl ExactSizeIterator<Item = &LegacyRestoreBinding> {
        self.legacy.iter().map(|evidence| &evidence.binding)
    }
}

pub fn restore_bound_day<'clock>(
    operation: ReadOperation<'clock>,
    day: &str,
    time: u64,
) -> RestoreRunOutcome<'clock> {
    run(operation, None, "day", Some(day), time)
}

pub fn restore_bound_all(operation: ReadOperation<'_>, time: u64) -> RestoreRunOutcome<'_> {
    run(operation, None, "all", None, time)
}

pub fn restore_owned_bound_day(
    operation: OwnedReadOperation,
    day: &str,
    time: u64,
) -> OwnedRestoreRunOutcome {
    run(operation, None, "day", Some(day), time)
}

pub fn restore_owned_bound_all(operation: OwnedReadOperation, time: u64) -> OwnedRestoreRunOutcome {
    run(operation, None, "all", None, time)
}

/// Restores bound files, then reviews each historical manifest in the selection
/// on the same read and approves exactly the candidate that review produced.
pub fn restore_owned_day(
    operation: OwnedReadOperation,
    day: &str,
    time: u64,
) -> OwnedRestoreRunOutcome {
    legacy::continue_with_legacy(run(operation, None, "day", Some(day), time), time)
}

pub fn restore_owned_all(operation: OwnedReadOperation, time: u64) -> OwnedRestoreRunOutcome {
    legacy::continue_with_legacy(run(operation, None, "all", None, time), time)
}

/// Retains the actual original recovery owner through execution and consuming finish.
pub fn restore_owned_bound_day_with_recovery(
    operation: OwnedReadOperation,
    binding: QualifiedArchiveRecoveryBinding,
    day: &str,
    time: u64,
) -> OwnedRestoreRunOutcome {
    run(operation, Some(binding), "day", Some(day), time)
}

pub fn restore_owned_bound_all_with_recovery(
    operation: OwnedReadOperation,
    binding: QualifiedArchiveRecoveryBinding,
    time: u64,
) -> OwnedRestoreRunOutcome {
    run(operation, Some(binding), "all", None, time)
}

fn metadata_allowance() -> Option<u64> {
    u64::try_from(solstone_core_retention::review_context::MAX_REGISTER_BYTES)
        .ok()?
        .checked_add(
            u64::try_from(solstone_core_journal_io::activity_obligations::MAX_INTENT_BYTES)
                .ok()?
                .checked_mul(4)?,
        )
}

fn run<'clock, R: RestoreRead<'clock>>(
    operation: R,
    binding: Option<QualifiedArchiveRecoveryBinding>,
    scope: &str,
    day: Option<&str>,
    time: u64,
) -> RestoreExecution<R> {
    if let Err(error) = check_recovery(&operation, binding.as_ref()) {
        return RestoreExecution {
            report: base("refused", Some("restore_held"), scope, day)
                .with_reason_detail(error.to_string()),
            _operation: operation,
            _binding: binding,
            files: Vec::new(),
            legacy: Vec::new(),
        };
    }
    let (mut report, mut files) = match select(operation.as_read().journal_root(), scope, day) {
        Ok(selected) => selected,
        Err(report) => {
            return RestoreExecution {
                report: *report,
                _operation: operation,
                _binding: binding,
                files: Vec::new(),
                legacy: Vec::new(),
            };
        }
    };
    if files.is_empty() {
        return RestoreExecution {
            report,
            _operation: operation,
            _binding: binding,
            files,
            legacy: Vec::new(),
        };
    }
    let Some(largest) = bound_maximum(&mut files) else {
        summarize(&mut report, &files);
        return RestoreExecution {
            report,
            _operation: operation,
            _binding: binding,
            files,
            legacy: Vec::new(),
        };
    };
    let required = metadata_allowance()
        .ok_or_else(|| "metadata allowance overflow".into())
        .and_then(|metadata| {
            operation
                .as_read()
                .offload_required_bytes(largest, metadata)
                .map_err(|error| error.to_string())
        });
    match required {
        Ok(required) => execute(operation, binding, report, files, time, required),
        Err(error) => {
            for file in &mut files {
                file.reason.get_or_insert("restore_held");
            }
            summarize(&mut report, &files);
            report.reason_detail = Some(error);
            RestoreExecution {
                report,
                _operation: operation,
                _binding: binding,
                files,
                legacy: Vec::new(),
            }
        }
    }
}

fn bound_maximum(files: &mut [FileRecord]) -> Option<u64> {
    files
        .iter_mut()
        .filter_map(|file| {
            if file.witness.repository_id.is_none() {
                file.reason = Some("archive_unbound");
                None
            } else {
                Some(file.witness.file.bytes)
            }
        })
        .max()
}

fn options() -> LockOptions {
    LockOptions {
        timeout: Duration::ZERO,
        poll_interval: Duration::ZERO,
        mode: Some(0o600),
    }
}

fn select(
    root: &Arc<JournalRoot>,
    scope: &str,
    day: Option<&str>,
) -> Result<(RestoreResult, Vec<FileRecord>), Box<RestoreResult>> {
    if day
        .is_some_and(|day| crate::ledger::ledger_path_for_day(root.canonical_path(), day).is_err())
    {
        return Err(Box::new(base("error", Some("ledger_degraded"), scope, day)));
    }
    let catalog = read_complete_catalog(root).map_err(|error| {
        Box::new(base("error", Some("ledger_degraded"), scope, day).with_reason_detail(error))
    })?;
    if catalog.degraded() {
        return Err(Box::new(base("error", Some("ledger_degraded"), scope, day)));
    }
    let segments: Vec<SegmentCatalog> = catalog
        .days
        .into_iter()
        .filter(|candidate| day.is_none_or(|day| candidate.day == day))
        .flat_map(|day| day.segments)
        .filter(|segment| !segment.active.is_empty())
        .collect();
    let mut files = Vec::new();
    for (detail, segment) in segments.iter().enumerate() {
        for witness in &segment.active {
            files.push(FileRecord {
                detail,
                day: segment.day.clone(),
                stream: segment.stream.clone(),
                segment: segment.segment.clone(),
                witness: witness.clone(),
                disposition: RestoreDisposition::Unattempted,
                reason: None,
                files: 0,
                bytes: 0,
                owner: FileOwner::None,
            });
        }
    }
    let report = refused_selection(scope, day, segments).map_err(|error| {
        Box::new(base("error", Some("failed"), scope, day).with_reason_detail(error.into()))
    })?;
    Ok((report, files))
}

/// Every segment of one day, refusing unrepresentable or duplicate identities.
pub(crate) fn discover(root: &Arc<JournalRoot>, day: &str) -> Result<Vec<Segment>, String> {
    let directory = MediaDirectory::open(Arc::clone(root), &Path::new("chronicle").join(day))
        .map_err(|error| error.to_string())?;
    let segments = iter_segments(root.canonical_path(), PathOrDay::Day(day))
        .map_err(|error| error.to_string())?;
    check_record_identities(&segments).map_err(|error| error.to_string())?;
    directory.revalidate().map_err(|error| error.to_string())?;
    root.revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    Ok(segments)
}

fn destination<'a>(
    file: &FileRecord,
    segments: &'a [Segment],
) -> Result<&'a Segment, &'static str> {
    let (Some(layout), Some(name)) = (file.witness.layout, file.witness.segment_dir.as_deref())
    else {
        return Err("archive_unbound");
    };
    if file.witness.repository_id.is_none()
        || file.witness.archive_path.is_none()
        || file.witness.journal_id.is_none()
        || file.witness.processing.is_none()
    {
        return Err("archive_unbound");
    }
    let mut matching = segments.iter().filter(|segment| {
        segment.locator_identity().is_ok_and(|identity| {
            identity.layout == layout
                && identity.name == name
                && identity.stream == file.stream
                && identity.key == file.segment
        })
    });
    let selected = matching.next().ok_or("segment_missing")?;
    if matching.next().is_some() {
        return Err("segment_identity");
    }
    Ok(selected)
}

fn summarize(report: &mut RestoreResult, files: &[FileRecord]) {
    report.files_restored = 0;
    report.bytes_restored = 0;
    report.segments_restored = 0;
    let mut complete = true;
    let mut attempted = false;
    report.reason = None;
    // Selection retains catalog order, grouping file records by detail.
    let mut remaining = files.iter().peekable();
    for (index, detail) in report.details.iter_mut().enumerate() {
        let mut detail_complete = true;
        detail.files_restored = 0;
        detail.bytes_restored = 0;
        detail.reason = None;
        while remaining.peek().is_some_and(|file| file.detail == index) {
            let Some(file) = remaining.next() else {
                break;
            };
            // Expected totals were checked before effects; each selected file
            // contributes at most its own declared bytes once.
            detail.files_restored += file.files;
            detail.bytes_restored += file.bytes;
            let done = matches!(&file.owner, FileOwner::Transaction(result) if result.completed())
                && matches!(
                    file.disposition,
                    RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
                );
            detail_complete &= done;
            attempted |= !matches!(file.owner, FileOwner::None);
            if detail.reason.is_none() && !done {
                detail.reason = Some(file.reason.unwrap_or("restore_held").into());
            }
            if !done {
                let hold = match &file.owner {
                    FileOwner::Staging(failure) => Some(failure.facts().hold),
                    FileOwner::Transaction(outcome) => outcome.facts().hold,
                    FileOwner::None | FileOwner::Review(_) => None,
                };
                if let Some(hold) = hold {
                    match &mut report.reason_detail {
                        Some(existing) => {
                            existing.push_str("; ");
                            existing.push_str(&hold);
                        }
                        None => report.reason_detail = Some(hold),
                    }
                }
            }
        }
        complete &= detail_complete;
        report.files_restored += detail.files_restored;
        report.bytes_restored += detail.bytes_restored;
        if detail_complete {
            report.segments_restored += 1;
        }
        detail.status = if detail_complete {
            "ok"
        } else if detail.files_restored > 0 {
            "degraded"
        } else {
            "refused"
        }
        .into();
        if report.reason.is_none() {
            report.reason = detail.reason.clone();
        }
    }
    report.status = if complete {
        if files.is_empty() { "no_op" } else { "ok" }
    } else if attempted {
        "degraded"
    } else {
        "refused"
    }
    .into();
    if files.is_empty() {
        report.reason = Some("nothing_to_restore".into());
    }
}

// The public entry computes this requirement through the original operation's
// retained configuration. It is repeated on the actual stage before any WAN.
fn execute<'clock, R: RestoreRead<'clock>>(
    mut operation: R,
    binding: Option<QualifiedArchiveRecoveryBinding>,
    mut report: RestoreResult,
    mut files: Vec<FileRecord>,
    time: u64,
    required_capacity: u64,
) -> RestoreExecution<R> {
    let root = Arc::clone(operation.as_read().journal_root());
    let mut discovered = BTreeMap::new();
    let mut stopped = false;
    for file in &mut files {
        if stopped {
            file.reason.get_or_insert("restore_held");
            continue;
        }
        // Known legacy refusal needs no destination discovery or stage.
        if file.witness.repository_id.is_none() {
            file.reason = Some("archive_unbound");
            continue;
        }
        let segments = discovered
            .entry(file.day.clone())
            .or_insert_with(|| discover(&root, &file.day));
        let segments = match segments {
            Ok(segments) => segments,
            Err(error) => {
                report.reason_detail.get_or_insert_with(|| error.clone());
                file.reason = Some("segment_identity");
                continue;
            }
        };
        let segment = match destination(file, segments) {
            Ok(segment) => segment,
            Err(reason) => {
                file.reason = Some(reason);
                continue;
            }
        };
        let destination = match PreparedRestore::admit(Arc::clone(&root), &file.day, segment) {
            Ok(destination) => destination,
            Err(error) => {
                report.reason_detail.get_or_insert(error.detail);
                file.reason = Some("destination_admission_failed");
                continue;
            }
        };
        match StagedRestore::read_with_recovery(
            destination,
            &mut operation,
            file.witness.clone(),
            required_capacity,
            binding.as_ref(),
        ) {
            Err(failure) => {
                file.reason = Some("restore_held");
                file.disposition = failure.disposition();
                file.owner = FileOwner::Staging(failure);
                stopped = true;
            }
            Ok(staged) => {
                let outcome = staged
                    .commit(time, options())
                    .retire_intent(options())
                    .into_owned();
                stopped = file.accept(outcome);
            }
        }
    }
    summarize(&mut report, &files);
    RestoreExecution {
        report,
        _operation: operation,
        _binding: binding,
        files,
        legacy: Vec::new(),
    }
}

#[cfg(all(test, feature = "full-tests", any(unix, windows)))]
pub(crate) mod tests;

mod finish;
pub mod legacy;
pub use finish::{
    HeldRestoreFinish, OwnedRestoreFinishHold, RestoreFinishRefusal, SettledRestoreReport,
};
