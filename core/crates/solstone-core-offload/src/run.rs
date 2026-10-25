// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Archive raw media, record the pre-mark ledger witness, then mint an offload mark.

use serde_json::Value;

pub use current::{
    CurrentFileIdentity, CurrentOffloadAvailability, CurrentOffloadDecision, CurrentOffloadRefusal,
    CurrentOffloadRefusalReason, CurrentOffloadReport, EligibleOffloadFile,
    inspect_current_offload, inspect_owned_current_offload,
};

mod offer_creation;
pub use offer_creation::{
    OffloadCreation, OffloadCreationNative, OffloadCreationOwner, OffloadCreationRefusal,
    OffloadLedgerEffect, OffloadLedgerSync, OffloadLedgerWrite, OffloadPlanning,
    OffloadPlanningPhase, OffloadPlanningRefusal, OffloadPlanningReport, OffloadSourceCleanup,
    OffloadSourceCopyReport, OwnedOffloadCreation, OwnedOffloadCreationFinish,
    OwnedOffloadCreationFinishRefusal, OwnedOffloadCreationHold, OwnedOffloadCreationOwner,
    OwnedOffloadInputFailure, OwnedOffloadInputs, OwnedOffloadPlan, OwnedOffloadPlanFinish,
    OwnedOffloadPlanFinishRefusal, OwnedOffloadPlanHold, OwnedOffloadPlanInputFailure,
    OwnedOffloadPlanInputs, OwnedOffloadPlanOwner, SettledOffloadCreation,
    SettledOffloadCreationReport, SettledOffloadPlan, SettledOffloadPlanReport,
    create_offload_offers, create_owned_offload_offers, create_owned_planned_offload_offers,
    create_planned_offload_offers, prepare_owned_offload_inputs, prepare_owned_offload_plan,
};

#[cfg(all(test, unix, feature = "full-tests"))]
pub(crate) fn with_payload_capacity_fault<T>(run: impl FnOnce() -> T) -> (T, bool) {
    offer_creation::with_payload_capacity_fault(run)
}

#[cfg(all(test, unix, feature = "full-tests"))]
pub(crate) fn with_source_cleanup_fault<T>(index: usize, run: impl FnOnce() -> T) -> (T, bool) {
    offer_creation::with_source_cleanup_fault(index, run)
}

pub const OFFLOAD_STALL_BACKUP_NOT_READY: &str = "backup_not_ready";
pub const OFFLOAD_STALL_BACKUP_FAILING: &str = "backup_failing";
pub const OFFLOAD_STALL_VERIFICATION_MISSING: &str = "verification_missing";
pub const OFFLOAD_STALL_VERIFICATION_OVERDUE: &str = "verification_overdue";
pub const OFFLOAD_STALL_VERIFICATION_FAILED: &str = "verification_failed";
pub const OFFLOAD_STALL_LOCKED: &str = "locked";
pub const OFFLOAD_STALL_ARCHIVE_FAILED: &str = "archive_failed";
pub const OFFLOAD_STALL_CONFIRM_FAILED: &str = "confirm_failed";
pub const OFFLOAD_STALL_CONFIRM_TOOL_FAILED: &str = "confirm_tool_failed";
pub const OFFLOAD_STALL_SEGMENT_IDENTITY: &str = "segment_identity";
pub const OFFLOAD_STALL_UNEXPECTED_ERROR: &str = "unexpected_error";
pub const OFFLOAD_STALL_REASONS: [&str; 11] = [
    OFFLOAD_STALL_BACKUP_NOT_READY,
    OFFLOAD_STALL_BACKUP_FAILING,
    OFFLOAD_STALL_VERIFICATION_MISSING,
    OFFLOAD_STALL_VERIFICATION_OVERDUE,
    OFFLOAD_STALL_VERIFICATION_FAILED,
    OFFLOAD_STALL_LOCKED,
    OFFLOAD_STALL_ARCHIVE_FAILED,
    OFFLOAD_STALL_CONFIRM_FAILED,
    OFFLOAD_STALL_CONFIRM_TOOL_FAILED,
    OFFLOAD_STALL_SEGMENT_IDENTITY,
    OFFLOAD_STALL_UNEXPECTED_ERROR,
];
pub const OFFLOAD_OK_BUDGET_ALREADY_SATISFIED: &str = "budget_already_satisfied";
pub const OFFLOAD_OK_BUDGET_SATISFIED: &str = "budget_satisfied";
pub const OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED: &str = "markable_media_exhausted";
pub const VERIFICATION_INTEGRITY_REASONS: [&str; 3] =
    ["integrity_failed", "auth_failed", "repo_missing"];
pub const VERIFICATION_MAX_AGE_SECONDS: i64 = 14 * 86400;
pub const OFFLOAD_MAX_RUNTIME: &str = "7h";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffloadSegmentDetail {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub files: u64,
    pub bytes: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffloadResult {
    pub status: String,
    pub reason: Option<String>,
    pub files_marked: u64,
    pub bytes_marked: u64,
    pub files_already_marked: u64,
    pub bytes_already_marked: u64,
    pub ran_out_of_markable_media: bool,
    pub dry_run: bool,
    /// Extra identity detail when `reason` is `segment_identity`. The reason
    /// token itself stays `segment_identity`.
    pub reason_detail: Option<String>,
    pub details: Vec<OffloadSegmentDetail>,
    /// Best-effort failure while creating or appending the invocation audit oplog.
    pub audit_recording_failure: Option<String>,
    /// Failure while recording the backup ledger's `last_offload` state.
    pub recording_failure: Option<String>,
}

pub fn format_offload_result(result: &OffloadResult) -> String {
    let line = if result.dry_run {
        if result.status == "stalled" {
            format!(
                "backup offload: stalled reason={}{} dry_run=true",
                offload_reason_text(
                    result
                        .reason
                        .as_deref()
                        .unwrap_or(OFFLOAD_STALL_UNEXPECTED_ERROR)
                ),
                result
                    .reason_detail
                    .as_deref()
                    .map(|detail| format!(" detail={detail}"))
                    .unwrap_or_default()
            )
        } else if result.status == "ok" {
            format!(
                "backup offload: ok reason={} dry_run=true",
                result
                    .reason
                    .as_deref()
                    .map(offload_reason_text)
                    .unwrap_or_default()
            )
        } else {
            format!("backup offload: {} dry_run=true", result.status)
        }
    } else if result.status == "ok" {
        format!(
            "backup offload: ok reason={} files_marked={} bytes_marked={} files_already_marked={} bytes_already_marked={} bytes_released=0 ran_out_of_markable_media={}",
            result
                .reason
                .as_deref()
                .map(offload_reason_text)
                .unwrap_or_default(),
            result.files_marked,
            result.bytes_marked,
            result.files_already_marked,
            result.bytes_already_marked,
            result.ran_out_of_markable_media
        )
    } else if result.status == "skipped" {
        "backup offload: skipped".to_owned()
    } else {
        format!(
            "backup offload: stalled reason={}{} files_marked={} bytes_marked={} bytes_released=0 ran_out_of_markable_media={}",
            offload_reason_text(
                result
                    .reason
                    .as_deref()
                    .unwrap_or(OFFLOAD_STALL_UNEXPECTED_ERROR)
            ),
            result
                .reason_detail
                .as_deref()
                .map(|detail| format!(" detail={detail}"))
                .unwrap_or_default(),
            result.files_marked,
            result.bytes_marked,
            result.ran_out_of_markable_media
        )
    };
    let line = match result.recording_failure.as_deref() {
        Some(detail) => format!("{line} recording_failed={detail}"),
        None => line,
    };
    match result.audit_recording_failure.as_deref() {
        Some(detail) => format!("{line} audit_recording_failed={detail}"),
        None => line,
    }
}

/// Render existing offload presentation codes, including prelaunch tool errors.
/// Unknown codes remain visible for diagnostics.
fn offload_reason_text(reason: &str) -> String {
    match reason {
        OFFLOAD_STALL_BACKUP_NOT_READY => {
            "your backup isn't set up or hasn't completed a successful run yet".into()
        }
        OFFLOAD_STALL_BACKUP_FAILING => "your last backup didn't complete successfully".into(),
        OFFLOAD_STALL_VERIFICATION_MISSING => "no backup verification has completed yet".into(),
        OFFLOAD_STALL_VERIFICATION_OVERDUE => {
            "backup verification hasn't succeeded recently enough".into()
        }
        OFFLOAD_STALL_VERIFICATION_FAILED => {
            "the last backup verification found a problem with the backup's integrity".into()
        }
        // Same wording as `backup_failure_reason`'s "locked" arm in
        // solstone-core-backup-cli, so the two surfaces read alike.
        OFFLOAD_STALL_LOCKED => {
            "the backup at the destination is locked, usually by a run that's still going or one \
             that was interrupted"
                .into()
        }
        OFFLOAD_STALL_ARCHIVE_FAILED => "the archive step of the offload didn't complete".into(),
        OFFLOAD_STALL_CONFIRM_FAILED => {
            "the offload couldn't confirm the archived files actually reached the backup".into()
        }
        OFFLOAD_STALL_CONFIRM_TOOL_FAILED => {
            "the check that confirms an offload archive couldn't complete".into()
        }
        OFFLOAD_STALL_SEGMENT_IDENTITY => "one of your journal's recordings couldn't be verified \
             against its own identity record"
            .into(),
        OFFLOAD_STALL_UNEXPECTED_ERROR => "something unexpected interrupted the offload".into(),
        OFFLOAD_OK_BUDGET_ALREADY_SATISFIED => {
            "the offload budget was already met, so nothing needed to be marked for release".into()
        }
        // ⛔ Never "moved" or "released": offload only marks raw media for a
        // later owner-approved release step and never deletes it itself
        // The CLI line also reports `bytes_released=0`.
        OFFLOAD_OK_BUDGET_SATISFIED => {
            "enough media has been marked for release to meet the offload budget".into()
        }
        OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED => {
            "every eligible file was already checked; nothing more can be marked for release \
             right now"
                .into()
        }
        "restic_unavailable" => "the restic backup program isn't available".into(),
        "rclone_unavailable" => "the rclone program isn't available".into(),
        other => format!("stopped without a reason ({other})"),
    }
}

fn precondition(config: &serde_json::Map<String, Value>, now: i64) -> Option<&'static str> {
    if config.get("enabled") != Some(&Value::Bool(true)) {
        return Some(OFFLOAD_STALL_BACKUP_NOT_READY);
    }
    let Some(backup) = config.get("last_backup").and_then(Value::as_object) else {
        return Some(OFFLOAD_STALL_BACKUP_NOT_READY);
    };
    if backup.get("status") != Some(&Value::String("ok".into())) {
        return Some(OFFLOAD_STALL_BACKUP_FAILING);
    }
    if backup
        .get("snapshot_id")
        .and_then(Value::as_str)
        .is_none_or(|snapshot| snapshot.trim().is_empty())
    {
        return Some(OFFLOAD_STALL_BACKUP_NOT_READY);
    }
    let Some(verification) = config.get("last_verification").and_then(Value::as_object) else {
        return Some(OFFLOAD_STALL_VERIFICATION_MISSING);
    };
    if !matches!(
        verification.get("status").and_then(Value::as_str),
        Some("ok" | "skipped" | "error")
    ) {
        return Some(OFFLOAD_STALL_VERIFICATION_MISSING);
    }
    if verification.get("status") == Some(&Value::String("error".into()))
        && verification
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| VERIFICATION_INTEGRITY_REASONS.contains(&reason))
    {
        return Some(OFFLOAD_STALL_VERIFICATION_FAILED);
    }
    let Some(last_ok) = verification.get("last_ok_time").and_then(Value::as_i64) else {
        return Some(OFFLOAD_STALL_VERIFICATION_MISSING);
    };
    let Some(age) = now.checked_sub(last_ok) else {
        return Some(OFFLOAD_STALL_VERIFICATION_OVERDUE);
    };
    if age < 0 {
        return Some(OFFLOAD_STALL_VERIFICATION_MISSING);
    }
    if age > VERIFICATION_MAX_AGE_SECONDS {
        return Some(OFFLOAD_STALL_VERIFICATION_OVERDUE);
    }
    None
}

mod current {
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use chrono::{DateTime, FixedOffset, Utc};
    use serde_json::{Map, Value};
    use solstone_core_backup_runtime::{AdmittedArchive, ArchiveError, OwnedAdmittedArchive};
    use solstone_core_journal_io::journal_root::JournalRoot;
    use solstone_core_journal_io::locking::LockOptions;
    use solstone_core_journal_io::media::{MediaDirectory, MediaIdentity, MediaObservation};
    use solstone_core_journal_io::paths::SegmentLayout;
    use solstone_core_retention::Target;
    use solstone_core_retention::archive_proof::{ApprovalRevision, ArchiveFileProof};
    use solstone_core_retention::content::{ClosedHandlerSet, ContentName, JournalMedia};
    use solstone_core_retention::grouped::{DecisionPurpose, GroupVerdict, inspect_grouped};
    use solstone_core_retention::marks::{Mark, MarkId, MarkState, RemovalClass};
    use solstone_core_retention::policy::{Policy, policy_from_journal_config};
    use solstone_core_retention::review_context::load_at_root;
    use solstone_core_retention::scan::{CheckedSegment, scan_segment_bound};
    use solstone_core_segment::output_attempt::OutputClaims;

    use solstone_core_retention::archive_catalog::read_complete_catalog;

    #[derive(Clone, Copy)]
    pub(super) enum CurrentArchive<'borrow, 'clock> {
        Generic(&'borrow AdmittedArchive<'clock>),
        Owned(&'borrow OwnedAdmittedArchive),
    }
    impl CurrentArchive<'_, '_> {
        pub(super) fn journal_root(&self) -> &Arc<JournalRoot> {
            match self {
                Self::Generic(operation) => operation.journal_root(),
                Self::Owned(operation) => operation.journal_root(),
            }
        }
        pub(super) fn repository_id(&self) -> &str {
            match self {
                Self::Generic(operation) => operation.repository_id(),
                Self::Owned(operation) => operation.repository_id(),
            }
        }
        pub(super) fn current_time(&self) -> Result<i64, ArchiveError> {
            match self {
                Self::Generic(operation) => operation.current_time(),
                Self::Owned(operation) => operation.current_time(),
            }
        }
        pub(super) fn revalidate(&self) -> Result<(), ArchiveError> {
            match self {
                Self::Generic(operation) => operation.revalidate(),
                Self::Owned(operation) => operation.revalidate_local(),
            }
        }
    }

    pub(super) const MAX_CURRENT_ENTRIES: usize = 100_000;
    const MAX_CURRENT_CONFIG_BYTES: usize = 1024 * 1024;

    #[cfg(test)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(super) enum Checkpoint {
        BeforeHash,
        AfterGroup,
        BeforeFinish,
    }
    #[cfg(test)]
    type TestCheckpoint = Box<dyn FnMut(Checkpoint, &Path)>;
    #[cfg(test)]
    thread_local! {
        static CHECKPOINT: std::cell::RefCell<Option<TestCheckpoint>> = const { std::cell::RefCell::new(None) };
    }
    #[cfg(test)]
    pub(super) fn with_checkpoint<T>(
        hook: impl FnMut(Checkpoint, &Path) + 'static,
        run: impl FnOnce() -> T,
    ) -> T {
        CHECKPOINT.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(Box::new(hook));
        });
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                CHECKPOINT.with(|slot| {
                    slot.borrow_mut().take();
                });
            }
        }
        let _reset = Reset;
        run()
    }
    #[cfg(test)]
    fn checkpoint(phase: Checkpoint, path: &Path) {
        CHECKPOINT.with(|slot| {
            if let Some(hook) = slot.borrow_mut().as_mut() {
                hook(phase, path);
            }
        });
    }

    /// Whether the requested observation completed with known refusal decisions.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum CurrentOffloadAvailability {
        Available,
        Unavailable,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum CurrentOffloadRefusalReason {
        Busy,
        Kept { names: Vec<String> },
        Stale,
        Failed,
        Missing,
        Unbound,
        WrongClass,
        Unproven { names: Vec<String> },
        Incomplete,
        AdmissionChanged,
        ExecutionUnavailable,
        Prerequisite { reason: &'static str },
    }

    #[derive(Debug)]
    pub struct CurrentOffloadRefusal {
        mark_id: Option<MarkId>,
        reason: CurrentOffloadRefusalReason,
    }

    impl CurrentOffloadRefusal {
        pub fn mark_id(&self) -> Option<&MarkId> {
            self.mark_id.as_ref()
        }
        pub fn reason(&self) -> &CurrentOffloadRefusalReason {
            &self.reason
        }
    }

    /// Lossless native identity observed by this batch, without a public factory.
    #[derive(Debug, Eq, PartialEq)]
    pub struct CurrentFileIdentity(MediaIdentity);

    /// A current projection observation, never a release capability.
    #[derive(Debug)]
    pub struct EligibleOffloadFile {
        mark: Arc<Mark>,
        file: ArchiveFileProof,
        layout: SegmentLayout,
        revision: ApprovalRevision,
        identity: CurrentFileIdentity,
    }

    impl EligibleOffloadFile {
        fn file(&self) -> &ArchiveFileProof {
            &self.file
        }
        pub fn target(&self) -> &Target {
            &self.mark.target
        }
        pub fn layout(&self) -> SegmentLayout {
            self.layout
        }
        pub fn name(&self) -> &str {
            &self.file().name
        }
        pub fn bytes(&self) -> u64 {
            self.file().bytes
        }
        pub fn revision(&self) -> &ApprovalRevision {
            &self.revision
        }
        pub fn class(&self) -> RemovalClass {
            self.mark.class
        }
        pub fn identity(&self) -> &CurrentFileIdentity {
            &self.identity
        }
        pub fn witness_id(&self) -> &str {
            &self.file().witness_id
        }
        pub fn mark(&self) -> &Mark {
            &self.mark
        }
    }

    pub struct CurrentOffloadDecision {
        root: Arc<JournalRoot>,
        availability: CurrentOffloadAvailability,
        eligible: Vec<EligibleOffloadFile>,
        refusals: Vec<CurrentOffloadRefusal>,
    }

    impl CurrentOffloadDecision {
        pub fn availability(&self) -> CurrentOffloadAvailability {
            self.availability
        }
        pub fn eligible(&self) -> impl Iterator<Item = &EligibleOffloadFile> {
            self.eligible.iter()
        }
        pub fn refusals(&self) -> &[CurrentOffloadRefusal] {
            &self.refusals
        }
        pub fn root_observation(&self) -> String {
            self.root.identity().observation_id()
        }
        pub(super) fn into_report(self) -> CurrentOffloadReport {
            CurrentOffloadReport {
                root_observation: self.root.identity().observation_id(),
                availability: self.availability,
                eligible: self.eligible,
                refusals: self.refusals,
            }
        }
        fn refuse(&mut self, mark: Option<&Mark>, reason: CurrentOffloadRefusalReason) {
            self.refusals.push(CurrentOffloadRefusal {
                mark_id: mark.map(|mark| mark.id.clone()),
                reason,
            });
        }
        fn unavailable(&mut self, reason: CurrentOffloadRefusalReason) {
            self.availability = CurrentOffloadAvailability::Unavailable;
            self.eligible.clear();
            self.refuse(None, reason);
        }
    }

    /// Original batch data without a retained journal handle or P authority.
    pub struct CurrentOffloadReport {
        root_observation: String,
        availability: CurrentOffloadAvailability,
        eligible: Vec<EligibleOffloadFile>,
        refusals: Vec<CurrentOffloadRefusal>,
    }
    impl CurrentOffloadReport {
        pub fn root_observation(&self) -> &str {
            &self.root_observation
        }
        pub fn availability(&self) -> CurrentOffloadAvailability {
            self.availability
        }
        pub fn eligible(&self) -> impl Iterator<Item = &EligibleOffloadFile> {
            self.eligible.iter()
        }
        pub fn refusals(&self) -> &[CurrentOffloadRefusal] {
            &self.refusals
        }
    }

    #[derive(PartialEq)]
    pub(super) struct CurrentPolicyObservation {
        prerequisites: Map<String, Value>,
        retention: Option<Value>,
        preserve_all: Option<Value>,
    }

    struct CurrentGroupObservation {
        directory: Arc<MediaDirectory>,
        scan: CheckedSegment,
        marks: Vec<Arc<Mark>>,
        layout: SegmentLayout,
    }

    fn same_scan(before: &CheckedSegment, after: &CheckedSegment) -> bool {
        before.files().len() == after.files().len()
            && before
                .files()
                .iter()
                .zip(after.files())
                .all(|(before_file, after_file)| {
                    before_file.name == after_file.name
                        && before_file.size == after_file.size
                        && before_file.sidecar.record == after_file.sidecar.record
                        && before_file.sidecar.has_analysis_row
                            == after_file.sidecar.has_analysis_row
                        && before.bound_entry(&before_file.name)
                            == after.bound_entry(&after_file.name)
                })
    }

    impl CurrentPolicyObservation {
        pub(super) fn capture(
            root: &Arc<JournalRoot>,
            now: i64,
        ) -> Result<Self, CurrentOffloadRefusalReason> {
            let directory = MediaDirectory::open(Arc::clone(root), Path::new("config"))
                .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
            let read = directory
                .read_file_bounded(OsStr::new("journal.json"), MAX_CURRENT_CONFIG_BYTES)
                .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?
                .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
            if !read.complete {
                return Err(CurrentOffloadRefusalReason::Incomplete);
            }
            let config: Map<String, Value> = serde_json::from_slice(&read.bytes)
                .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
            let backup = config
                .get("backup")
                .and_then(Value::as_object)
                .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
            if let Some(reason) = super::precondition(backup, now) {
                return Err(CurrentOffloadRefusalReason::Prerequisite { reason });
            }
            if let Some(retention) = config.get("retention") {
                let retention = retention
                    .as_object()
                    .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
                if let Some(streams) = retention.get("per_stream") {
                    let streams = streams
                        .as_object()
                        .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
                    if streams.values().any(|stream| !stream.is_object()) {
                        return Err(CurrentOffloadRefusalReason::Incomplete);
                    }
                }
            }
            let preserve_all = match config.get("transcribe") {
                None => None,
                Some(value) => value
                    .as_object()
                    .ok_or(CurrentOffloadRefusalReason::Incomplete)?
                    .get("preserve_all")
                    .cloned(),
            };
            directory
                .revalidate()
                .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
            // The backup and verification records are checked above on every
            // capture, not pinned: a scheduled run that finishes while
            // offload plans writes a newer one, and a healthy newer record is
            // no reason to stop.
            Ok(Self {
                prerequisites: ["enabled"]
                    .into_iter()
                    .filter_map(|key| {
                        backup
                            .get(key)
                            .cloned()
                            .map(|value| (key.to_owned(), value))
                    })
                    .collect(),
                retention: config.get("retention").cloned(),
                preserve_all,
            })
        }

        pub(super) fn policy(&self) -> Policy {
            let mut config = Map::new();
            if let Some(retention) = &self.retention {
                config.insert("retention".into(), retention.clone());
            }
            if let Some(preserve_all) = &self.preserve_all {
                config.insert(
                    "transcribe".into(),
                    serde_json::json!({"preserve_all": preserve_all}),
                );
            }
            policy_from_journal_config(&config)
        }

        pub(super) fn revalidate(
            &self,
            operation: CurrentArchive<'_, '_>,
        ) -> Result<(), CurrentOffloadRefusalReason> {
            let now = operation
                .current_time()
                .map_err(|_| CurrentOffloadRefusalReason::AdmissionChanged)?;
            if Self::capture(operation.journal_root(), now)? != *self {
                return Err(CurrentOffloadRefusalReason::AdmissionChanged);
            }
            Ok(())
        }
    }

    fn relative_segment(target: &Target, layout: SegmentLayout) -> PathBuf {
        let day = Path::new("chronicle").join(&target.day);
        match layout {
            SegmentLayout::Direct => day.join(&target.dir),
            SegmentLayout::Named => day.join(&target.stream).join(&target.dir),
        }
    }

    fn check_mark(
        root: &JournalRoot,
        repository: &str,
        current: &solstone_core_retention::marks::Register,
        expected: &Mark,
    ) -> Result<(), CurrentOffloadRefusalReason> {
        if expected.class != RemovalClass::OffloadRawRelease {
            return Err(CurrentOffloadRefusalReason::WrongClass);
        }
        let actual = current
            .marks
            .get(&expected.id)
            .ok_or(CurrentOffloadRefusalReason::Missing)?;
        if actual != expected {
            return Err(CurrentOffloadRefusalReason::Stale);
        }
        if !matches!(actual.state, MarkState::Marked) {
            return Err(CurrentOffloadRefusalReason::Failed);
        }
        let offer = actual
            .offload
            .as_ref()
            .ok_or(CurrentOffloadRefusalReason::Unbound)?;
        if offer.validate().is_err()
            || offer.proof.journal_id != root.identity().observation_id()
            || offer.proof.repository_id != repository
        {
            return Err(CurrentOffloadRefusalReason::Unbound);
        }
        Ok(())
    }

    /// Inspect one requested batch using fresh bound proof and a per-call hash set.
    /// Busy groups earn no credit. Incomplete/global observations make the whole
    /// result unavailable, rather than representing unavailable P as zero.
    /// `at` supplies the owner's calendar offset and may not be in the admitted
    /// clock's future; prerequisites are checked against that actual clock.
    pub fn inspect_current_offload(
        operation: &mut AdmittedArchive<'_>,
        expected: &[Mark],
        at: DateTime<FixedOffset>,
    ) -> CurrentOffloadDecision {
        inspect(CurrentArchive::Generic(operation), expected, at)
    }

    pub fn inspect_owned_current_offload(
        operation: &mut OwnedAdmittedArchive,
        expected: &[Mark],
        at: DateTime<FixedOffset>,
    ) -> CurrentOffloadDecision {
        inspect(CurrentArchive::Owned(operation), expected, at)
    }

    pub(super) fn inspect(
        operation: CurrentArchive<'_, '_>,
        expected: &[Mark],
        at: DateTime<FixedOffset>,
    ) -> CurrentOffloadDecision {
        let root = Arc::clone(operation.journal_root());
        let mut decision = CurrentOffloadDecision {
            root: Arc::clone(&root),
            availability: CurrentOffloadAvailability::Available,
            eligible: Vec::new(),
            refusals: Vec::new(),
        };
        // Known prelaunch refusal cannot become actionable projected relief.
        if !cfg!(any(unix, windows)) {
            decision.unavailable(CurrentOffloadRefusalReason::ExecutionUnavailable);
            return decision;
        }
        let inspect = || -> Result<CurrentPolicyObservation, CurrentOffloadRefusalReason> {
            operation
                .revalidate()
                .map_err(|_| CurrentOffloadRefusalReason::AdmissionChanged)?;
            let now = operation
                .current_time()
                .map_err(|_| CurrentOffloadRefusalReason::AdmissionChanged)?;
            if at.timestamp() > now {
                return Err(CurrentOffloadRefusalReason::AdmissionChanged);
            }
            CurrentPolicyObservation::capture(&root, now)
        };
        let observation = match inspect() {
            Ok(observation) => observation,
            Err(reason) => {
                decision.unavailable(reason);
                return decision;
            }
        };
        let current = match load_at_root(&root) {
            Ok(current) => current,
            Err(_) => {
                decision.unavailable(CurrentOffloadRefusalReason::Incomplete);
                return decision;
            }
        };
        if read_complete_catalog(&root).is_err() {
            decision.unavailable(CurrentOffloadRefusalReason::Incomplete);
            return decision;
        }
        let mut ids = BTreeSet::new();
        let mut groups: BTreeMap<(String, String, String, SegmentLayout), Vec<Arc<Mark>>> =
            BTreeMap::new();
        for mark in expected {
            if !ids.insert(mark.id.clone()) {
                decision.unavailable(CurrentOffloadRefusalReason::Incomplete);
                return decision;
            }
            if let Err(reason) = check_mark(&root, operation.repository_id(), &current, mark) {
                decision.refuse(Some(mark), reason);
                continue;
            }
            if let Some(offer) = &mark.offload {
                let target = &mark.target;
                groups
                    .entry((
                        target.day.clone(),
                        target.stream.clone(),
                        target.dir.clone(),
                        offer.proof.layout,
                    ))
                    .or_default()
                    .push(Arc::new(mark.clone()));
            }
        }
        let policy = observation.policy();
        let mut observed_groups = Vec::new();
        for ((_, _, _, layout), marks) in groups {
            match inspect_group(
                operation,
                &observation,
                &policy,
                &root,
                &marks,
                layout,
                at,
                &mut decision,
            ) {
                Ok(Some(group)) => observed_groups.push(group),
                Ok(None) => {}
                Err(reason) => {
                    decision.unavailable(reason);
                    return decision;
                }
            }
        }
        #[cfg(test)]
        checkpoint(Checkpoint::BeforeFinish, root.canonical_path());
        if operation.revalidate().is_err() || observation.revalidate(operation).is_err() {
            decision.unavailable(CurrentOffloadRefusalReason::AdmissionChanged);
            return decision;
        }
        let final_register = load_at_root(&root);
        let final_catalog = read_complete_catalog(&root);
        let (Ok(final_register), Ok(final_catalog)) = (final_register, final_catalog) else {
            decision.unavailable(CurrentOffloadRefusalReason::Incomplete);
            return decision;
        };
        let prior = std::mem::take(&mut decision.eligible);
        for file in prior {
            let mark = &file.mark;
            let result = check_mark(&root, operation.repository_id(), &final_register, mark);
            if let Err(reason) = result {
                decision.refuse(Some(mark), reason);
                continue;
            }
            let Some(offer) = &mark.offload else {
                decision.unavailable(CurrentOffloadRefusalReason::Incomplete);
                return decision;
            };
            let proof = &offer.proof;
            if final_catalog.active_witness(proof, file.file()).is_none() {
                decision.refuse(Some(mark), CurrentOffloadRefusalReason::Unbound);
                continue;
            }
            decision.eligible.push(file);
        }
        for group in observed_groups {
            if let Err(reason) = recheck_group(operation, &observation, &root, group, &mut decision)
            {
                decision.unavailable(reason);
                return decision;
            }
        }
        if observation.revalidate(operation).is_err() {
            decision.unavailable(CurrentOffloadRefusalReason::AdmissionChanged);
        }
        decision
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "explicit current batch admission and observations"
    )]
    fn inspect_group(
        operation: CurrentArchive<'_, '_>,
        observation: &CurrentPolicyObservation,
        policy: &Policy,
        root: &Arc<JournalRoot>,
        marks: &[Arc<Mark>],
        layout: SegmentLayout,
        at: DateTime<FixedOffset>,
        decision: &mut CurrentOffloadDecision,
    ) -> Result<Option<CurrentGroupObservation>, CurrentOffloadRefusalReason> {
        let Some(first) = marks.first() else {
            return Ok(None);
        };
        let target = &first.target;
        let relative = relative_segment(target, layout);
        let path = root.canonical_path().join(&relative);
        let directory = MediaDirectory::open(Arc::clone(root), &relative)
            .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        let options = LockOptions {
            timeout: Duration::ZERO,
            ..LockOptions::default()
        };
        let claims = match OutputClaims::discover(&path, options) {
            Ok(claims) => claims,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                for mark in marks {
                    decision.refuse(Some(mark), CurrentOffloadRefusalReason::Busy);
                }
                return Ok(None);
            }
            Err(_) => return Err(CurrentOffloadRefusalReason::Incomplete),
        };
        if claims
            .segment_identity(&path)
            .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?
            != directory.object_identity()
        {
            return Err(CurrentOffloadRefusalReason::Incomplete);
        }
        let commit = match claims.commit(&path) {
            Ok(commit) => commit,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                for mark in marks {
                    decision.refuse(Some(mark), CurrentOffloadRefusalReason::Busy);
                }
                return Ok(None);
            }
            Err(_) => return Err(CurrentOffloadRefusalReason::Incomplete),
        };
        observation.revalidate(operation)?;
        let register = load_at_root(root).map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        let catalog =
            read_complete_catalog(root).map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        let scan = scan_segment_bound(
            &directory,
            &ClosedHandlerSet,
            &JournalMedia,
            MAX_CURRENT_ENTRIES,
        )
        .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        let grouped = inspect_grouped(
            &scan,
            target,
            layout,
            policy,
            at.date_naive(),
            at.with_timezone(&Utc),
            DecisionPurpose::OffloadProposal,
        );
        let mut hashes: BTreeMap<String, MediaObservation> = BTreeMap::new();
        let start = decision.eligible.len();
        for mark in marks {
            if let Err(reason) = check_mark(root, operation.repository_id(), &register, mark) {
                decision.refuse(Some(mark), reason);
                continue;
            }
            let Some(offer) = &mark.offload else {
                return Err(CurrentOffloadRefusalReason::Incomplete);
            };
            let proof = &offer.proof;
            for file in &proof.files {
                let current = grouped
                    .eligible()
                    .find(|candidate| candidate.name() == file.name);
                let Some(current) = current else {
                    let group = [grouped.empty(), grouped.ordinary()]
                        .into_iter()
                        .find(|group| group.members().contains(&file.name));
                    let reason = match group.map(|group| group.verdict()) {
                        Some(GroupVerdict::Kept(_)) => CurrentOffloadRefusalReason::Kept {
                            names: vec![file.name.clone()],
                        },
                        _ => CurrentOffloadRefusalReason::Unproven {
                            names: vec![file.name.clone()],
                        },
                    };
                    decision.refuse(Some(mark), reason);
                    continue;
                };
                if current.bytes() != file.bytes
                    || current.processing() != &file.processing
                    || catalog.active_witness(proof, file).is_none()
                {
                    decision.refuse(Some(mark), CurrentOffloadRefusalReason::Unbound);
                    continue;
                }
                if !hashes.contains_key(&file.name) {
                    let name = ContentName::new(&file.name)
                        .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
                    let entry = scan
                        .bound_entry(&name)
                        .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
                    let before = directory
                        .read_prefix(OsStr::new(&file.name), 0)
                        .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?
                        .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
                    if before.entry != *entry {
                        return Err(CurrentOffloadRefusalReason::Incomplete);
                    }
                    #[cfg(test)]
                    checkpoint(Checkpoint::BeforeHash, &path.join(&file.name));
                    let hashed = directory
                        .observe_file(OsStr::new(&file.name))
                        .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?
                        .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
                    let after = directory
                        .read_prefix(OsStr::new(&file.name), 0)
                        .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?
                        .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
                    if after.entry != *entry
                        || hashed.identity != entry.identity
                        || hashed.digest.size != entry.size
                    {
                        return Err(CurrentOffloadRefusalReason::Incomplete);
                    }
                    hashes.insert(file.name.clone(), hashed);
                }
                let hashed = hashes
                    .get(&file.name)
                    .ok_or(CurrentOffloadRefusalReason::Incomplete)?;
                if hashed.digest.size != file.bytes || hashed.digest.sha256 != file.sha256 {
                    decision.refuse(Some(mark), CurrentOffloadRefusalReason::Unbound);
                    continue;
                }
                decision.eligible.push(EligibleOffloadFile {
                    mark: Arc::clone(mark),
                    file: file.clone(),
                    layout,
                    revision: offer.revision.clone(),
                    identity: CurrentFileIdentity(hashed.identity),
                });
            }
        }
        let final_scan = scan_segment_bound(
            &directory,
            &ClosedHandlerSet,
            &JournalMedia,
            MAX_CURRENT_ENTRIES,
        )
        .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        if !same_scan(&scan, &final_scan) {
            return Err(CurrentOffloadRefusalReason::Incomplete);
        }
        for file in decision
            .eligible
            .get(start..)
            .ok_or(CurrentOffloadRefusalReason::Incomplete)?
        {
            let name =
                ContentName::new(file.name()).ok_or(CurrentOffloadRefusalReason::Incomplete)?;
            if scan.bound_entry(&name) != final_scan.bound_entry(&name) {
                return Err(CurrentOffloadRefusalReason::Incomplete);
            }
        }
        observation.revalidate(operation)?;
        commit
            .revalidate()
            .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        directory
            .revalidate()
            .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        #[cfg(test)]
        checkpoint(Checkpoint::AfterGroup, &path);
        if decision.eligible.len() == start {
            return Ok(None);
        }
        Ok(Some(CurrentGroupObservation {
            directory,
            scan,
            marks: marks.to_vec(),
            layout,
        }))
    }

    fn recheck_group(
        operation: CurrentArchive<'_, '_>,
        observation: &CurrentPolicyObservation,
        root: &Arc<JournalRoot>,
        group: CurrentGroupObservation,
        decision: &mut CurrentOffloadDecision,
    ) -> Result<(), CurrentOffloadRefusalReason> {
        let Some(first) = group.marks.first() else {
            return Err(CurrentOffloadRefusalReason::Incomplete);
        };
        let matches = |file: &EligibleOffloadFile| {
            file.target() == &first.target && file.layout() == group.layout
        };
        if !decision.eligible.iter().any(matches) {
            return Ok(());
        }
        group
            .directory
            .revalidate()
            .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        let path = root
            .canonical_path()
            .join(relative_segment(&first.target, group.layout));
        let options = LockOptions {
            timeout: Duration::ZERO,
            ..LockOptions::default()
        };
        let busy = |decision: &mut CurrentOffloadDecision| {
            decision.eligible.retain(|file| !matches(file));
            for mark in &group.marks {
                decision.refuse(Some(mark), CurrentOffloadRefusalReason::Busy);
            }
        };
        let claims = match OutputClaims::discover(&path, options) {
            Ok(claims) => claims,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                busy(decision);
                return Ok(());
            }
            Err(_) => return Err(CurrentOffloadRefusalReason::Incomplete),
        };
        if claims
            .segment_identity(&path)
            .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?
            != group.directory.object_identity()
        {
            return Err(CurrentOffloadRefusalReason::Incomplete);
        }
        let commit = match claims.commit(&path) {
            Ok(commit) => commit,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                busy(decision);
                return Ok(());
            }
            Err(_) => return Err(CurrentOffloadRefusalReason::Incomplete),
        };
        observation.revalidate(operation)?;
        let scan = scan_segment_bound(
            &group.directory,
            &ClosedHandlerSet,
            &JournalMedia,
            MAX_CURRENT_ENTRIES,
        )
        .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        if !same_scan(&group.scan, &scan) {
            return Err(CurrentOffloadRefusalReason::Incomplete);
        }
        let register = load_at_root(root).map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        let catalog =
            read_complete_catalog(root).map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        let prior = std::mem::take(&mut decision.eligible);
        for file in prior {
            if matches(&file) {
                if let Err(reason) =
                    check_mark(root, operation.repository_id(), &register, &file.mark)
                {
                    decision.refuse(Some(&file.mark), reason);
                    continue;
                }
                let Some(offer) = &file.mark.offload else {
                    return Err(CurrentOffloadRefusalReason::Incomplete);
                };
                if catalog.active_witness(&offer.proof, file.file()).is_none() {
                    decision.refuse(Some(&file.mark), CurrentOffloadRefusalReason::Unbound);
                    continue;
                }
            }
            decision.eligible.push(file);
        }
        observation.revalidate(operation)?;
        commit
            .revalidate()
            .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        group
            .directory
            .revalidate()
            .map_err(|_| CurrentOffloadRefusalReason::Incomplete)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(unix, windows))]
    use std::cell::RefCell;
    #[cfg(any(unix, windows))]
    use std::collections::VecDeque;
    #[cfg(any(unix, windows))]
    use std::io;

    #[cfg(any(unix, windows))]
    use chrono::DateTime;
    #[cfg(all(unix, feature = "full-tests"))]
    use chrono::FixedOffset;
    use serde_json::json;
    #[cfg(unix)]
    use sha2::{Digest, Sha256};
    #[cfg(any(unix, windows))]
    use solstone_core_backup::{
        Destination, generate_and_store_keys, record_backup_result, record_verification_result,
        set_destination, set_enabled, set_offload,
    };
    #[cfg(any(unix, windows))]
    use solstone_core_backup_runtime::BackupServices;
    #[cfg(any(unix, windows))]
    use solstone_core_backup_runtime::hosted_runtime::HttpError;
    #[cfg(any(unix, windows))]
    use solstone_core_backup_runtime::{
        Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
        JournalMaintenanceError, ToolOutput, ToolRequest, ToolRunner,
    };
    #[cfg(any(unix, windows))]
    use solstone_core_journal_io::JournalRoot;
    #[cfg(unix)]
    use solstone_core_retention::{
        Target,
        content::{ClosedHandlerSet, JournalMedia},
        policy::policy_from_journal_config,
    };
    #[cfg(any(unix, windows))]
    use std::fs;
    #[cfg(any(unix, windows))]
    use std::path::Path;
    #[cfg(unix)]
    use std::path::PathBuf;

    #[cfg(unix)]
    use crate::measurement::measure_local_media;

    #[cfg(all(test, unix, feature = "full-tests"))]
    mod planner;

    #[test]
    fn prerequisites_refuse_missing_malformed_future_and_expired_observations() {
        let now = 2_000_000;
        let ready = json!({
            "enabled":true,
            "last_backup":{"status":"ok","snapshot_id":"ready"},
            "last_verification":{"status":"ok","last_ok_time":now}
        });
        let check = |value: &Value| precondition(value.as_object().unwrap(), now);
        assert_eq!(check(&ready), None);
        for key in ["last_backup", "last_verification"] {
            let reason = if key == "last_backup" {
                OFFLOAD_STALL_BACKUP_NOT_READY
            } else {
                OFFLOAD_STALL_VERIFICATION_MISSING
            };
            for value in [Value::Null, json!([]), json!("ok"), json!(1)] {
                let mut changed = ready.clone();
                changed[key] = value;
                assert_eq!(check(&changed), Some(reason));
            }
            let mut missing = ready.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert_eq!(check(&missing), Some(reason));
        }
        for value in [Value::Null, json!("ready"), json!(1.5), json!(now + 1)] {
            let mut changed = ready.clone();
            changed["last_verification"]["last_ok_time"] = value;
            assert_eq!(check(&changed), Some(OFFLOAD_STALL_VERIFICATION_MISSING));
        }
        for key in ["status", "last_ok_time"] {
            let mut missing = ready.clone();
            missing["last_verification"]
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert_eq!(check(&missing), Some(OFFLOAD_STALL_VERIFICATION_MISSING));
        }
        for value in [Value::Null, json!(true), json!("unknown")] {
            let mut changed = ready.clone();
            changed["last_verification"]["status"] = value;
            assert_eq!(check(&changed), Some(OFFLOAD_STALL_VERIFICATION_MISSING));
        }
        for snapshot in [Value::Null, json!(""), json!(" \t"), json!(7)] {
            let mut changed = ready.clone();
            changed["last_backup"]["snapshot_id"] = snapshot;
            assert_eq!(check(&changed), Some(OFFLOAD_STALL_BACKUP_NOT_READY));
        }
        for (time, expected) in [
            (now - VERIFICATION_MAX_AGE_SECONDS, None),
            (
                now - VERIFICATION_MAX_AGE_SECONDS - 1,
                Some(OFFLOAD_STALL_VERIFICATION_OVERDUE),
            ),
            (i64::MIN, Some(OFFLOAD_STALL_VERIFICATION_OVERDUE)),
        ] {
            let mut changed = ready.clone();
            changed["last_verification"]["last_ok_time"] = json!(time);
            assert_eq!(check(&changed), expected);
        }
        for reason in VERIFICATION_INTEGRITY_REASONS {
            let mut changed = ready.clone();
            changed["last_verification"]["status"] = json!("error");
            changed["last_verification"]["reason"] = json!(reason);
            assert_eq!(check(&changed), Some(OFFLOAD_STALL_VERIFICATION_FAILED));
        }
        let mut transient = ready;
        transient["last_verification"]["status"] = json!("error");
        transient["last_verification"]["reason"] = json!("locked");
        assert_eq!(check(&transient), None);
    }

    #[cfg(any(unix, windows))]
    struct Script {
        outputs: RefCell<VecDeque<ToolOutput>>,
        calls: RefCell<Vec<Vec<String>>>,
    }
    #[cfg(any(unix, windows))]
    impl ToolRunner for Script {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.calls.borrow_mut().push(
                request
                    .argv
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
            );
            Ok(self
                .outputs
                .borrow_mut()
                .pop_front()
                .expect("fixture output"))
        }
    }
    #[cfg(any(unix, windows))]
    struct Http;
    #[cfg(any(unix, windows))]
    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            unreachable!("BYO offload does not use HTTP")
        }
    }
    #[cfg(any(unix, windows))]
    struct TestClock {
        now: i64,
    }
    #[cfg(any(unix, windows))]
    impl Clock for TestClock {
        fn now_unix(&self) -> i64 {
            self.now
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    #[cfg(any(unix, windows))]
    struct Maintenance;
    #[cfg(any(unix, windows))]
    impl JournalMaintenance for Maintenance {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
    }
    #[cfg(any(unix, windows))]
    fn output(stdout: String) -> ToolOutput {
        ToolOutput {
            returncode: 0,
            stdout: stdout.into_bytes(),
            stderr: vec![],
        }
    }

    #[cfg(unix)]
    fn write_eligible_sidecar(raw: &Path) {
        let size = raw.metadata().unwrap().len();
        let header = json!({"_solstone_processing": {
            "schema":"solstone.processing.v1",
            "state":"empty",
            "reason_code":"no_decodable_frames",
            "handler":"describe",
            "attempted_at":"2026-01-01T00:00:00Z",
            "input_size":size
        }});
        fs::write(raw.with_extension("jsonl"), format!("{header}\n")).unwrap();
    }

    /// Same shape as `write_eligible_sidecar`, but the `MediaClass::NoDecodableAudio`
    /// side: `.flac` extension and `no_decodable_audio` reason code, so `retention`'s
    /// `empty_audio_rule` (not the ordinary-media default rule) is what a policy
    /// check against this file consults.
    #[cfg(unix)]
    fn write_empty_audio_sidecar(raw: &Path) {
        let size = raw.metadata().unwrap().len();
        let header = json!({"_solstone_processing": {
            "schema":"solstone.processing.v1",
            "state":"empty",
            "reason_code":"no_decodable_audio",
            "handler":"transcribe",
            "attempted_at":"2026-01-01T00:00:00Z",
            "input_size":size
        }});
        fs::write(raw.with_extension("jsonl"), format!("{header}\n")).unwrap();
    }

    #[cfg(any(unix, windows))]
    fn configure_offload(journal: &Path, now: i64, budget_bytes: Option<u64>) {
        set_destination(
            journal,
            &Destination {
                repository: "s3:bucket/prefix".into(),
                backend: "s3".into(),
                credentials:
                    serde_json::json!({"access_key_id":"ACCESS","secret_access_key":"SECRET"})
                        .as_object()
                        .unwrap()
                        .clone(),
            },
        )
        .unwrap();
        generate_and_store_keys(journal).unwrap();
        set_enabled(journal, true).unwrap();
        record_backup_result(
            journal,
            "ok",
            json!(now),
            json!("ready"),
            Value::Null,
            None,
            None,
        )
        .unwrap();
        record_verification_result(journal, "ok", json!(now), Value::Null, json!("1/52")).unwrap();
        set_offload(
            journal,
            &serde_json::json!({"enabled":true,"budget_bytes":budget_bytes,"floor_bytes":1})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
    }
    #[cfg(any(unix, windows))]
    fn services<'a>(
        runner: &'a dyn ToolRunner,
        http: &'a Http,
        clock: &'a TestClock,
        maintenance: &'a Maintenance,
    ) -> BackupServices<'a> {
        BackupServices {
            runner,
            http,
            clock,
            restic: Some((Path::new("/fixture/bin/restic")).into()),
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }

    /// A scheduled backup or verify that finishes while offload is planning
    /// writes a newer record. Planning goes on while the backup is still
    /// healthy, and holds the moment it is not.
    #[cfg(any(unix, windows))]
    #[test]
    fn a_newer_healthy_backup_record_keeps_planning_and_a_failed_one_stops_it() {
        let now = 1_791_590_400;
        let journal = tempfile::tempdir().unwrap();
        configure_offload(journal.path(), now, None);
        let root = std::sync::Arc::new(JournalRoot::open(journal.path()).unwrap());
        let planned = current::CurrentPolicyObservation::capture(&root, now).unwrap();
        record_backup_result(
            journal.path(),
            "ok",
            json!(now + 1),
            json!("newer"),
            Value::Null,
            None,
            None,
        )
        .unwrap();
        record_verification_result(
            journal.path(),
            "ok",
            json!(now + 2),
            Value::Null,
            json!("2/52"),
        )
        .unwrap();
        let later = current::CurrentPolicyObservation::capture(&root, now + 3).unwrap();
        assert!(later == planned, "a healthy newer record changed the plan");
        record_backup_result(
            journal.path(),
            "error",
            json!(now + 4),
            Value::Null,
            json!("failed"),
            None,
            None,
        )
        .unwrap();
        assert!(matches!(
            current::CurrentPolicyObservation::capture(&root, now + 5),
            Err(current::CurrentOffloadRefusalReason::Prerequisite {
                reason: OFFLOAD_STALL_BACKUP_FAILING
            })
        ));
    }

    #[test]
    fn no_offload_run_reason_code_reaches_the_owner_as_an_identifier() {
        const EVERY_REASON: &[&str] = &[
            OFFLOAD_STALL_BACKUP_NOT_READY,
            OFFLOAD_STALL_BACKUP_FAILING,
            OFFLOAD_STALL_VERIFICATION_MISSING,
            OFFLOAD_STALL_VERIFICATION_OVERDUE,
            OFFLOAD_STALL_VERIFICATION_FAILED,
            OFFLOAD_STALL_LOCKED,
            OFFLOAD_STALL_ARCHIVE_FAILED,
            OFFLOAD_STALL_CONFIRM_FAILED,
            OFFLOAD_STALL_CONFIRM_TOOL_FAILED,
            OFFLOAD_STALL_SEGMENT_IDENTITY,
            OFFLOAD_STALL_UNEXPECTED_ERROR,
            OFFLOAD_OK_BUDGET_ALREADY_SATISFIED,
            OFFLOAD_OK_BUDGET_SATISFIED,
            OFFLOAD_OK_MARKABLE_MEDIA_EXHAUSTED,
            "restic_unavailable",
            "rclone_unavailable",
        ];
        for reason in EVERY_REASON {
            let rendered = offload_reason_text(reason);
            assert_ne!(rendered, *reason, "{reason} reaches the owner verbatim");
        }
        // ⛔ An unmapped code still reaches support rather than being swallowed.
        let unmapped = offload_reason_text("some_new_code");
        assert!(unmapped.contains("some_new_code"), "{unmapped}");
    }

    #[cfg(unix)]
    const CURRENT_NOW: i64 = 1_767_355_200;
    #[cfg(unix)]
    const CURRENT_REPO: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[cfg(unix)]
    struct CurrentFixture {
        journal: tempfile::TempDir,
        root: std::sync::Arc<JournalRoot>,
        marks: Vec<solstone_core_retention::marks::Mark>,
        segment: PathBuf,
        ledger: PathBuf,
    }

    #[cfg(unix)]
    impl CurrentFixture {
        fn new(layout: solstone_core_journal_io::paths::SegmentLayout) -> Self {
            use solstone_core_retention::archive_proof::{
                ArchiveFileProof, OffloadOffer, OffloadProof,
            };
            use solstone_core_retention::marks::{Mark, MarkId, MarkState, Proposal, RemovalClass};
            let journal = tempfile::tempdir().unwrap();
            configure_offload(journal.path(), CURRENT_NOW, Some(1));
            let target = Target {
                day: "20260101".into(),
                stream: "_default".into(),
                dir: "010000_001_exact".into(),
            };
            let relative = match layout {
                solstone_core_journal_io::paths::SegmentLayout::Direct => {
                    "chronicle/20260101/010000_001_exact"
                }
                solstone_core_journal_io::paths::SegmentLayout::Named => {
                    "chronicle/20260101/_default/010000_001_exact"
                }
            };
            let segment = journal.path().join(relative);
            fs::create_dir_all(&segment).unwrap();
            for (name, bytes) in [("first.webm", &b"one"[..]), ("second.webm", &b"four"[..])] {
                let raw = segment.join(name);
                fs::write(&raw, bytes).unwrap();
                write_eligible_sidecar(&raw);
            }
            let unmarked = journal
                .path()
                .join("chronicle/20260101/020000_001/unmarked.webm");
            fs::create_dir_all(unmarked.parent().unwrap()).unwrap();
            fs::write(unmarked, [7; 16]).unwrap();
            let root = std::sync::Arc::new(JournalRoot::open(journal.path()).unwrap());
            let directory = solstone_core_journal_io::media::MediaDirectory::open(
                std::sync::Arc::clone(&root),
                Path::new(relative),
            )
            .unwrap();
            let scan = solstone_core_retention::scan::scan_segment_bound(
                &directory,
                &ClosedHandlerSet,
                &JournalMedia,
                100,
            )
            .unwrap();
            let policy = policy_from_journal_config(&serde_json::Map::new());
            let at = DateTime::from_timestamp(CURRENT_NOW, 0).unwrap();
            let inspected = solstone_core_retention::grouped::inspect_grouped(
                &scan,
                &target,
                layout,
                &policy,
                at.date_naive(),
                at,
                solstone_core_retention::grouped::DecisionPurpose::OffloadProposal,
            );
            assert_eq!(inspected.eligible().count(), 2);
            let files = inspected
                .eligible()
                .map(|file| ArchiveFileProof {
                    witness_id: format!("witness-{}", file.name()),
                    name: file.name().into(),
                    bytes: file.bytes(),
                    sha256: format!(
                        "{:x}",
                        Sha256::digest(fs::read(segment.join(file.name())).unwrap())
                    ),
                    archive_path: format!("/owned/input/{}", file.name()),
                    processing: file.processing().clone(),
                })
                .collect::<Vec<_>>();
            let proof = OffloadProof {
                journal_id: root.identity().observation_id(),
                manifest_id: "current-manifest".into(),
                target: target.clone(),
                layout,
                repository_id: CURRENT_REPO.into(),
                snapshot_id: "b".repeat(64),
                files,
            };
            proof.validate().unwrap();
            let ledger = journal.path().join("health/offload/20260101.jsonl");
            fs::create_dir_all(ledger.parent().unwrap()).unwrap();
            let event = json!({"event_kind":"offload_archive_v2","schema_version":2,"time":CURRENT_NOW,
                "day":target.day,"stream":target.stream,"segment":"010000_001","segment_dir":target.dir,
                "layout":layout,"journal_id":proof.journal_id,"manifest_id":proof.manifest_id,
                "repository_id":proof.repository_id,"snapshot_id":proof.snapshot_id,"files":proof.files});
            fs::write(&ledger, format!("{event}\n")).unwrap();
            let mut marks = Vec::new();
            for count in [2, 1] {
                let mut selected = proof.clone();
                selected.files.truncate(count);
                let names = selected
                    .files
                    .iter()
                    .map(|file| file.name.clone())
                    .collect::<Vec<_>>();
                let bytes = selected.bytes().unwrap();
                marks.push(Mark {
                    id: MarkId::derive(RemovalClass::OffloadRawRelease, &target, &names),
                    class: RemovalClass::OffloadRawRelease,
                    target: target.clone(),
                    marked_at: "2026-01-02T12:00:00Z".into(),
                    proposal: Proposal {
                        bytes,
                        reason: "archived recording".into(),
                        names,
                    },
                    state: MarkState::Marked,
                    offload: Some(OffloadOffer::new(selected).unwrap()),
                });
            }
            let fixture = Self {
                journal,
                root,
                marks,
                segment,
                ledger,
            };
            fixture.save(&fixture.marks);
            fixture
        }
        fn save(&self, marks: &[solstone_core_retention::marks::Mark]) {
            let register = solstone_core_retention::marks::Register {
                version: if marks.iter().any(|mark| mark.offload.is_some()) {
                    2
                } else {
                    1
                },
                marks: marks
                    .iter()
                    .cloned()
                    .map(|mark| (mark.id.clone(), mark))
                    .collect(),
            };
            fs::write(
                self.journal.path().join("health/retention-marks.json"),
                serde_json::to_vec(&register).unwrap(),
            )
            .unwrap();
        }
        fn change_config(&self, change: impl FnOnce(&mut Value)) {
            let path = self.journal.path().join("config/journal.json");
            let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            change(&mut config);
            fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
        }
        fn add_offer(&mut self, proof: solstone_core_retention::archive_proof::OffloadProof) {
            use solstone_core_retention::marks::{Mark, MarkId, MarkState, Proposal, RemovalClass};
            use std::io::Write;
            let target = proof.target.clone();
            let event = json!({"event_kind":"offload_archive_v2","schema_version":2,"time":CURRENT_NOW,
                "day":target.day,"stream":target.stream,"segment":solstone_core_format::segment::segment_key(&target.dir).unwrap(),"segment_dir":target.dir,
                "layout":proof.layout,"journal_id":proof.journal_id,"manifest_id":proof.manifest_id,"repository_id":proof.repository_id,"snapshot_id":proof.snapshot_id,"files":proof.files});
            writeln!(
                fs::OpenOptions::new()
                    .append(true)
                    .open(&self.ledger)
                    .unwrap(),
                "{event}"
            )
            .unwrap();
            let names = proof
                .files
                .iter()
                .map(|file| file.name.clone())
                .collect::<Vec<_>>();
            let bytes = proof.bytes().unwrap();
            self.marks.push(Mark {
                id: MarkId::derive(RemovalClass::OffloadRawRelease, &target, &names),
                class: RemovalClass::OffloadRawRelease,
                target,
                marked_at: "2026-01-02T12:00:00Z".into(),
                proposal: Proposal {
                    bytes,
                    reason: "archived recording".into(),
                    names,
                },
                state: MarkState::Marked,
                offload: Some(
                    solstone_core_retention::archive_proof::OffloadOffer::new(proof).unwrap(),
                ),
            });
            self.save(&self.marks);
        }
        fn observe_offer(
            &self,
            layout: solstone_core_journal_io::paths::SegmentLayout,
            dir: &str,
            names: &[&str],
            manifest: &str,
        ) -> solstone_core_retention::archive_proof::OffloadProof {
            use solstone_core_journal_io::paths::SegmentLayout;
            use solstone_core_retention::archive_proof::{ArchiveFileProof, OffloadProof};
            let target = Target {
                dir: dir.into(),
                ..self.marks[0].target.clone()
            };
            let day = Path::new("chronicle").join(&target.day);
            let relative = match layout {
                SegmentLayout::Direct => day.join(dir),
                SegmentLayout::Named => day.join(&target.stream).join(dir),
            };
            let directory = solstone_core_journal_io::media::MediaDirectory::open(
                std::sync::Arc::clone(&self.root),
                &relative,
            )
            .unwrap();
            let scan = solstone_core_retention::scan::scan_segment_bound(
                &directory,
                &ClosedHandlerSet,
                &JournalMedia,
                100,
            )
            .unwrap();
            let at = DateTime::from_timestamp(CURRENT_NOW, 0).unwrap();
            let grouped = solstone_core_retention::grouped::inspect_grouped(
                &scan,
                &target,
                layout,
                &policy_from_journal_config(&serde_json::Map::new()),
                at.date_naive(),
                at,
                solstone_core_retention::grouped::DecisionPurpose::OffloadProposal,
            );
            let files = names
                .iter()
                .map(|name| {
                    let current = grouped
                        .eligible()
                        .find(|file| file.name() == *name)
                        .unwrap();
                    ArchiveFileProof {
                        witness_id: format!("{manifest}-{name}"),
                        name: (*name).into(),
                        bytes: current.bytes(),
                        sha256: format!(
                            "{:x}",
                            Sha256::digest(
                                fs::read(self.journal.path().join(&relative).join(name)).unwrap()
                            )
                        ),
                        archive_path: format!("/owned/{manifest}/{name}"),
                        processing: current.processing().clone(),
                    }
                })
                .collect();
            let proof = OffloadProof {
                journal_id: self.root.identity().observation_id(),
                manifest_id: manifest.into(),
                target,
                layout,
                repository_id: CURRENT_REPO.into(),
                snapshot_id: "b".repeat(64),
                files,
            };
            proof.validate().unwrap();
            proof
        }
        fn inspect(
            &self,
            expected: &[solstone_core_retention::marks::Mark],
        ) -> CurrentOffloadDecision {
            let runner = Script {
                outputs: RefCell::new(
                    (0..6)
                        .map(|_| output(json!({"id":CURRENT_REPO}).to_string()))
                        .collect(),
                ),
                calls: RefCell::new(vec![]),
            };
            let clock = TestClock { now: CURRENT_NOW };
            let services = services(&runner, &Http, &clock, &Maintenance);
            let mut operation = solstone_core_backup_runtime::admit_archive(
                std::sync::Arc::clone(&self.root),
                &services,
            )
            .unwrap();
            let carrier = std::sync::Arc::clone(operation.carrier());
            let decision = inspect_current_offload(
                &mut operation,
                expected,
                DateTime::from_timestamp(CURRENT_NOW, 0)
                    .unwrap()
                    .fixed_offset(),
            );
            // Each inspection is its own admission; retire its root activity
            // carrier so the next admission's census starts clean.
            drop(operation);
            let retirement = std::sync::Arc::try_unwrap(carrier)
                .unwrap_or_else(|_| panic!("inspection must release its activity carrier"))
                .retire_after_settlement();
            assert_eq!(
                retirement.receipt.effect(),
                solstone_core_journal_io::media::ActivityCarrierRetirementEffect::Removed
            );
            assert!(retirement.receipt.failures().is_empty());
            decision
        }
        fn project(
            &self,
            decision: &CurrentOffloadDecision,
        ) -> Result<Option<crate::measurement::ProposedRelief>, String> {
            crate::measurement::project_current_offload(
                &measure_local_media(self.journal.path()),
                decision,
            )
        }
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_uses_actual_admission_and_hashes_overlaps_once_per_call() {
        use solstone_core_journal_io::paths::SegmentLayout;
        use std::rc::Rc;
        for layout in [SegmentLayout::Direct, SegmentLayout::Named] {
            let fixture = CurrentFixture::new(layout);
            let hashes = Rc::new(RefCell::new(Vec::new()));
            let seen = Rc::clone(&hashes);
            let decision = current::with_checkpoint(
                move |phase, path| {
                    if phase == current::Checkpoint::BeforeHash {
                        seen.borrow_mut().push(path.to_path_buf());
                    }
                },
                || fixture.inspect(&fixture.marks),
            );
            assert_eq!(
                decision.availability(),
                CurrentOffloadAvailability::Available
            );
            assert!(decision.refusals().is_empty());
            assert_eq!(hashes.borrow().len(), 2);
            assert_eq!(decision.eligible().count(), 3);
            let projected = fixture.project(&decision).unwrap().unwrap();
            assert_eq!(projected.local.total_bytes, 23);
            assert_eq!(projected.eligible_bytes, 7);
            assert_eq!(projected.eligible_files, 2);
            assert_eq!(projected.projected_local_bytes, 16);
            assert_eq!(
                decision.root_observation(),
                fixture.root.identity().observation_id()
            );
            for file in decision.eligible() {
                assert_eq!(file.layout(), layout);
                assert_eq!(
                    file.class(),
                    solstone_core_retention::marks::RemovalClass::OffloadRawRelease
                );
                assert_eq!(
                    file.revision(),
                    &file.mark().offload.as_ref().unwrap().revision
                );
                assert!(file.witness_id().starts_with("witness-"));
            }
            let seen = Rc::clone(&hashes);
            let second = current::with_checkpoint(
                move |phase, path| {
                    if phase == current::Checkpoint::BeforeHash {
                        seen.borrow_mut().push(path.to_path_buf());
                    }
                },
                || fixture.inspect(&fixture.marks),
            );
            assert_eq!(fixture.project(&second).unwrap().unwrap().eligible_bytes, 7);
            assert_eq!(hashes.borrow().len(), 4);
        }
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_distinguishes_available_zero_from_unavailable() {
        let fixture = CurrentFixture::new(solstone_core_journal_io::paths::SegmentLayout::Direct);
        let zero = fixture.inspect(&[]);
        assert_eq!(zero.availability(), CurrentOffloadAvailability::Available);
        assert_eq!(zero.eligible().count(), 0);
        let projected = fixture.project(&zero).unwrap().unwrap();
        assert_eq!(projected.eligible_bytes, 0);
        assert_eq!(projected.projected_local_bytes, 23);
        fixture.change_config(|config| config["retention"] = json!([]));
        let unavailable = fixture.inspect(&fixture.marks);
        assert_eq!(
            unavailable.availability(),
            CurrentOffloadAvailability::Unavailable
        );
        assert_eq!(unavailable.eligible().count(), 0);
        assert!(!unavailable.refusals().is_empty());
        assert_eq!(fixture.project(&unavailable).unwrap(), None);
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_refuses_changed_missing_failed_legacy_and_other_classes() {
        use solstone_core_retention::marks::{Failure, MarkId, MarkState, RemovalClass};
        for case in 0..5 {
            let fixture =
                CurrentFixture::new(solstone_core_journal_io::paths::SegmentLayout::Direct);
            let mut selected = fixture.marks[0].clone();
            let reason = match case {
                0 => {
                    let mut changed = selected.clone();
                    changed.proposal.reason = "new review".into();
                    fixture.save(&[changed]);
                    CurrentOffloadRefusalReason::Stale
                }
                1 => {
                    fixture.save(&[]);
                    CurrentOffloadRefusalReason::Missing
                }
                2 => {
                    selected.state = MarkState::Failed(Failure {
                        at: "2026-01-02T12:00:00Z".into(),
                        reason: "interrupted".into(),
                        staged: None,
                    });
                    fixture.save(&[selected.clone()]);
                    CurrentOffloadRefusalReason::Failed
                }
                3 => {
                    selected.offload = None;
                    fixture.save(&[selected.clone()]);
                    CurrentOffloadRefusalReason::Unbound
                }
                _ => {
                    selected.offload = None;
                    selected.class = RemovalClass::PolicyRawRelease;
                    selected.id =
                        MarkId::derive(selected.class, &selected.target, &selected.proposal.names);
                    fixture.save(&[selected.clone()]);
                    CurrentOffloadRefusalReason::WrongClass
                }
            };
            let decision = fixture.inspect(&[selected]);
            assert_eq!(
                decision.availability(),
                CurrentOffloadAvailability::Available,
                "case {case}"
            );
            assert_eq!(decision.eligible().count(), 0, "case {case}");
            let projected = fixture.project(&decision).unwrap().unwrap();
            assert_eq!(projected.local.total_bytes, 23, "case {case}");
            assert_eq!(projected.eligible_bytes, 0, "case {case}");
            assert_eq!(projected.projected_local_bytes, 23, "case {case}");
            assert!(
                decision
                    .refusals()
                    .iter()
                    .any(|refusal| refusal.reason() == &reason),
                "case {case}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_refuses_wrong_bytes_processing_sibling_and_missing_witness() {
        for case in 0..5 {
            let fixture =
                CurrentFixture::new(solstone_core_journal_io::paths::SegmentLayout::Direct);
            match case {
                0 => fs::write(fixture.segment.join("first.webm"), b"bad").unwrap(),
                1 => {
                    let raw = fixture.segment.join("first.webm");
                    write_eligible_sidecar(&raw);
                    let path = raw.with_extension("jsonl");
                    let text = fs::read_to_string(&path)
                        .unwrap()
                        .replace("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z");
                    fs::write(path, text).unwrap();
                }
                2 => {
                    fs::write(fixture.segment.join("unselected.webm"), b"unproven sibling").unwrap()
                }
                3 => fs::write(&fixture.ledger, b"").unwrap(),
                _ => fs::remove_file(fixture.segment.join("first.webm")).unwrap(),
            }
            let decision = fixture.inspect(&fixture.marks[1..]);
            assert_eq!(
                decision.availability(),
                CurrentOffloadAvailability::Available,
                "case {case}"
            );
            assert_eq!(decision.eligible().count(), 0, "case {case}");
            let projected = fixture.project(&decision).unwrap().unwrap();
            assert_eq!(projected.eligible_bytes, 0, "case {case}");
            assert_eq!(
                projected.projected_local_bytes, projected.local.total_bytes,
                "case {case}"
            );
            if case == 4 {
                assert_eq!(projected.local.total_bytes, 20);
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_refuses_kept_empty_independently_from_ordinary_siblings() {
        let mut fixture =
            CurrentFixture::new(solstone_core_journal_io::paths::SegmentLayout::Direct);
        let empty = fixture.segment.join("empty.flac");
        fs::write(&empty, b"empty").unwrap();
        write_empty_audio_sidecar(&empty);
        let proof = fixture.observe_offer(
            solstone_core_journal_io::paths::SegmentLayout::Direct,
            "010000_001_exact",
            &["empty.flac"],
            "empty-manifest",
        );
        fixture.add_offer(proof);
        fixture.change_config(|config| config["transcribe"] = json!({"preserve_all":true}));
        // The unrelated empty class must not hold the proven ordinary offer.
        let decision = fixture.inspect(&fixture.marks);
        assert_eq!(
            decision.availability(),
            CurrentOffloadAvailability::Available
        );
        let projected = fixture.project(&decision).unwrap().unwrap();
        assert_eq!(projected.local.total_bytes, 28);
        assert_eq!(projected.eligible_bytes, 7);
        assert_eq!(projected.projected_local_bytes, 21);
        assert!(
            decision
                .refusals()
                .iter()
                .any(|refusal| matches!(refusal.reason(),
            CurrentOffloadRefusalReason::Kept {names} if names == &["empty.flac"]))
        );
        assert!(!decision.eligible().any(|file| file.name() == "empty.flac"));
        let kept = fixture.inspect(&fixture.marks[2..]);
        let projected = fixture.project(&kept).unwrap().unwrap();
        assert_eq!(projected.eligible_bytes, 0);
        assert_eq!(projected.projected_local_bytes, 28);
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_keeps_same_names_at_distinct_exact_locators() {
        use solstone_core_journal_io::paths::SegmentLayout;
        let mut fixture = CurrentFixture::new(SegmentLayout::Direct);
        // Keep both overlapping Direct offers. The Named two-file offer has a
        // distinct Mark ID while its first file differs only by exact layout.
        for (layout, dir, bytes, manifest) in [
            (
                SegmentLayout::Named,
                "010000_001_exact",
                &b"named"[..],
                "named-manifest",
            ),
            (
                SegmentLayout::Direct,
                "010000_001_other",
                &b"other!"[..],
                "other-manifest",
            ),
        ] {
            let day = fixture.journal.path().join("chronicle/20260101");
            let segment = match layout {
                SegmentLayout::Direct => day.join(dir),
                SegmentLayout::Named => day.join("_default").join(dir),
            };
            fs::create_dir_all(&segment).unwrap();
            let raw = segment.join("first.webm");
            fs::write(&raw, bytes).unwrap();
            write_eligible_sidecar(&raw);
            let names = if layout == SegmentLayout::Named {
                let raw = segment.join("named-only.webm");
                fs::write(&raw, b"x").unwrap();
                write_eligible_sidecar(&raw);
                vec!["first.webm", "named-only.webm"]
            } else {
                vec!["first.webm"]
            };
            fixture.add_offer(fixture.observe_offer(layout, dir, &names, manifest));
        }
        let decision = fixture.inspect(&fixture.marks);
        assert_eq!(
            decision.availability(),
            CurrentOffloadAvailability::Available
        );
        assert!(decision.refusals().is_empty(), "{:?}", decision.refusals());
        assert_eq!(decision.eligible().count(), 6);
        let projected = fixture.project(&decision).unwrap().unwrap();
        assert_eq!(projected.local.total_bytes, 35);
        assert_eq!(projected.eligible_bytes, 19);
        assert_eq!(projected.eligible_files, 5);
        assert_eq!(projected.projected_local_bytes, 16);
        assert_eq!(
            decision
                .eligible()
                .filter(|file| file.name() == "first.webm")
                .count(),
            4
        );
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_refuses_post_group_media_and_processing_drift_without_rehash() {
        use std::rc::Rc;
        for case in 0..4 {
            let fixture =
                CurrentFixture::new(solstone_core_journal_io::paths::SegmentLayout::Direct);
            let segment = fixture.segment.clone();
            let hashes = Rc::new(RefCell::new(0));
            let seen = Rc::clone(&hashes);
            let decision = current::with_checkpoint(
                move |phase, _| {
                    if phase == current::Checkpoint::BeforeHash {
                        *seen.borrow_mut() += 1;
                    }
                    if phase != current::Checkpoint::AfterGroup {
                        return;
                    }
                    match case {
                        0 => fs::write(segment.join("first.webm"), b"bad").unwrap(),
                        1 => {
                            let path = segment.join("first.jsonl");
                            let text = fs::read_to_string(&path)
                                .unwrap()
                                .replace("2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z");
                            fs::write(path, text).unwrap();
                        }
                        2 => fs::write(segment.join("late.webm"), b"new unprocessed sibling")
                            .unwrap(),
                        _ => fs::remove_file(segment.join("first.jsonl")).unwrap(),
                    }
                },
                || fixture.inspect(&fixture.marks),
            );
            assert_eq!(
                decision.availability(),
                CurrentOffloadAvailability::Unavailable,
                "case {case}"
            );
            assert_eq!(decision.eligible().count(), 0, "case {case}");
            assert_eq!(fixture.project(&decision).unwrap(), None);
            assert_eq!(*hashes.borrow(), 2, "case {case}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_refuses_reoffered_nonce_and_retired_witness() {
        use std::io::Write;
        let fixture = CurrentFixture::new(solstone_core_journal_io::paths::SegmentLayout::Direct);
        let expected = fixture.marks[0].clone();
        let mut replacement = expected.clone();
        replacement.offload = Some(
            solstone_core_retention::archive_proof::OffloadOffer::new(
                expected.offload.as_ref().unwrap().proof.clone(),
            )
            .unwrap(),
        );
        assert_ne!(
            replacement.offload.as_ref().unwrap().nonce,
            expected.offload.as_ref().unwrap().nonce
        );
        fixture.save(&[replacement]);
        let stale = fixture.inspect(std::slice::from_ref(&expected));
        assert_eq!(stale.availability(), CurrentOffloadAvailability::Available);
        assert_eq!(stale.eligible().count(), 0);
        assert!(
            stale
                .refusals()
                .iter()
                .any(|refusal| refusal.reason() == &CurrentOffloadRefusalReason::Stale)
        );
        fixture.save(&fixture.marks);
        let proof = &expected.offload.as_ref().unwrap().proof;
        let files = proof.files.iter().map(|file| json!({"witness_id":file.witness_id,"name":file.name,"disposition":"published"})).collect::<Vec<_>>();
        let event = json!({"event_kind":"offload_restore_v2","schema_version":2,"time":CURRENT_NOW,
            "day":expected.target.day,"stream":expected.target.stream,"segment":"010000_001",
            "operation_id":"restored","files":files});
        writeln!(
            fs::OpenOptions::new()
                .append(true)
                .open(&fixture.ledger)
                .unwrap(),
            "{event}"
        )
        .unwrap();
        let retired = fixture.inspect(&fixture.marks);
        assert_eq!(
            retired.availability(),
            CurrentOffloadAvailability::Available
        );
        assert_eq!(retired.eligible().count(), 0);
        assert_eq!(
            fixture.project(&retired).unwrap().unwrap().eligible_bytes,
            0
        );
        assert!(
            retired
                .refusals()
                .iter()
                .any(|refusal| refusal.reason() == &CurrentOffloadRefusalReason::Unbound)
        );
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_erases_prefix_credit_on_late_config_or_catalog_failure() {
        for case in 0..3 {
            let fixture =
                CurrentFixture::new(solstone_core_journal_io::paths::SegmentLayout::Direct);
            let config = fixture.journal.path().join("config/journal.json");
            let ledger = fixture.ledger.clone();
            let decision = current::with_checkpoint(
                move |phase, _| {
                    if phase != current::Checkpoint::AfterGroup {
                        return;
                    }
                    if case == 0 {
                        fs::write(&ledger, b"broken\n").unwrap();
                    } else {
                        let mut value: Value =
                            serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
                        if case == 1 {
                            value["transcribe"] = json!({"preserve_all":true});
                        } else {
                            value["backup"]["last_verification"]["status"] = json!("error");
                            value["backup"]["last_verification"]["reason"] =
                                json!("integrity_failed");
                        }
                        fs::write(&config, serde_json::to_vec(&value).unwrap()).unwrap();
                    }
                },
                || fixture.inspect(&fixture.marks),
            );
            assert_eq!(
                decision.availability(),
                CurrentOffloadAvailability::Unavailable,
                "case {case}"
            );
            assert_eq!(decision.eligible().count(), 0);
            assert_eq!(fixture.project(&decision).unwrap(), None);
        }
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_busy_group_does_not_block_unrelated_group() {
        use solstone_core_retention::marks::MarkId;
        use solstone_core_segment::output_attempt::OutputClaims;
        let mut fixture =
            CurrentFixture::new(solstone_core_journal_io::paths::SegmentLayout::Direct);
        let other = fixture.journal.path().join("chronicle/20260101/030000_001");
        fs::create_dir(&other).unwrap();
        let raw = other.join("third.webm");
        fs::write(&raw, b"other").unwrap();
        write_eligible_sidecar(&raw);
        let mut mark = fixture.marks[1].clone();
        mark.target.dir = "030000_001".into();
        mark.proposal.names = vec!["third.webm".into()];
        mark.proposal.bytes = 5;
        let offer = mark.offload.as_mut().unwrap();
        let mut proof = offer.proof.clone();
        proof.target = mark.target.clone();
        proof.manifest_id = "other-manifest".into();
        let file = &mut proof.files[0];
        file.name = "third.webm".into();
        file.witness_id = "other-witness".into();
        file.bytes = 5;
        file.sha256 = format!("{:x}", Sha256::digest(b"other"));
        file.archive_path = "/owned/input/third.webm".into();
        file.processing.record.as_mut().unwrap()["input_size"] = json!(5);
        *offer = solstone_core_retention::archive_proof::OffloadOffer::new(proof.clone()).unwrap();
        mark.id = MarkId::derive(mark.class, &mark.target, &mark.proposal.names);
        let event = json!({"event_kind":"offload_archive_v2","schema_version":2,"time":CURRENT_NOW,"day":mark.target.day,"stream":mark.target.stream,"segment":"030000_001","segment_dir":mark.target.dir,"layout":"direct","journal_id":proof.journal_id,"manifest_id":proof.manifest_id,"repository_id":proof.repository_id,"snapshot_id":proof.snapshot_id,"files":proof.files});
        use std::io::Write;
        let mut ledger = fs::OpenOptions::new()
            .append(true)
            .open(&fixture.ledger)
            .unwrap();
        writeln!(ledger, "{event}").unwrap();
        fixture.marks.push(mark);
        fixture.save(&fixture.marks);
        let _busy = OutputClaims::discover(
            &fixture.segment,
            solstone_core_journal_io::LockOptions {
                timeout: std::time::Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        let decision = fixture.inspect(&fixture.marks);
        assert_eq!(
            decision.availability(),
            CurrentOffloadAvailability::Available
        );
        let projected = fixture.project(&decision).unwrap().unwrap();
        assert_eq!(projected.local.total_bytes, 28);
        assert_eq!(projected.eligible_bytes, 5);
        assert_eq!(projected.projected_local_bytes, 23);
        assert!(
            decision
                .refusals()
                .iter()
                .any(|refusal| refusal.reason() == &CurrentOffloadRefusalReason::Busy)
        );
    }

    #[test]
    #[cfg(unix)]
    fn current_batch_projection_requires_complete_exact_local_intersection() {
        use crate::measurement::project_current_offload;
        use solstone_core_journal_io::paths::SegmentLayout;
        for layout in [SegmentLayout::Direct, SegmentLayout::Named] {
            let fixture = CurrentFixture::new(layout);
            let decision = fixture.inspect(&fixture.marks);
            let local = measure_local_media(fixture.journal.path());
            for case in 0..6 {
                let mut changed = local.clone();
                let file = changed
                    .files
                    .iter_mut()
                    .find(|file| file.key.name == "first.webm")
                    .unwrap();
                match case {
                    0 => file.key.day = "20260102".into(),
                    1 => file.key.stream = "other".into(),
                    2 => file.key.segment_dir = "010000_001_other".into(),
                    3 => {
                        file.key.layout = match layout {
                            SegmentLayout::Direct => SegmentLayout::Named,
                            SegmentLayout::Named => SegmentLayout::Direct,
                        }
                    }
                    4 => file.key.name = "renamed.webm".into(),
                    _ => file.bytes += 1,
                }
                let projected = project_current_offload(&changed, &decision)
                    .unwrap()
                    .unwrap();
                assert_eq!(projected.eligible_bytes, 4, "case {case}");
                assert_eq!(projected.eligible_files, 1, "case {case}");
                assert_eq!(
                    projected.projected_local_bytes,
                    projected.local.total_bytes - 4
                );
            }
            let mut metadata = local.clone();
            metadata
                .files
                .iter_mut()
                .find(|file| file.key.name == "first.webm")
                .unwrap()
                .key
                .segment = "grouping metadata only".into();
            assert_eq!(
                project_current_offload(&metadata, &decision)
                    .unwrap()
                    .unwrap()
                    .eligible_bytes,
                7
            );
            let mut duplicate = local.clone();
            duplicate.files.push(duplicate.files[0].clone());
            assert!(project_current_offload(&duplicate, &decision).is_err());
            let mut incomplete = local.clone();
            incomplete.unknown_paths.push(fixture.segment.clone());
            assert!(project_current_offload(&incomplete, &decision).is_err());
            let mut overflow = local;
            overflow.files[0].bytes = u64::MAX;
            assert!(project_current_offload(&overflow, &decision).is_err());
        }
    }

    #[test]
    #[cfg(windows)]
    fn current_batch_with_no_marks_is_available_on_windows() {
        struct RepositoryOnly;
        impl ToolRunner for RepositoryOnly {
            fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
                assert!(
                    request
                        .argv
                        .windows(2)
                        .any(|args| args == ["cat", "config"])
                );
                Ok(output(json!({"id":"a".repeat(64)}).to_string()))
            }
        }
        let journal = tempfile::tempdir().unwrap();
        configure_offload(journal.path(), 100, Some(1));
        let root = std::sync::Arc::new(JournalRoot::open(journal.path()).unwrap());
        let clock = TestClock { now: 100 };
        let services = services(&RepositoryOnly, &Http, &clock, &Maintenance);
        let mut operation = solstone_core_backup_runtime::admit_archive(root, &services).unwrap();
        let decision = inspect_current_offload(
            &mut operation,
            &[],
            DateTime::from_timestamp(100, 0).unwrap().fixed_offset(),
        );
        assert_eq!(
            decision.availability(),
            CurrentOffloadAvailability::Available
        );
        assert_eq!(decision.eligible().count(), 0);
        assert!(decision.refusals().is_empty());
    }
}
