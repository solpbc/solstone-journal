// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retained current inputs for archive offers. Capture does not confirm an
//! archive, create consent, publish a Mark or authorize removal.

#[cfg(any(unix, windows))]
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_backup::archive_activity::PreparedArchiveOperation;
use solstone_core_backup::archive_readback::{ReadOperation, ReadbackError};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::media::{MediaDirectory, MediaObservation};
use solstone_core_journal_io::paths::SegmentLayout;

use crate::archive_proof::{AcceptedProcessingObservation, OffloadProof};
use crate::marks::Mark;
#[cfg(any(unix, windows))]
use crate::marks::{MarkId, RemovalClass};
use crate::receipt::Target;

mod owned;
pub(crate) mod reconciliation;
pub use owned::{
    OwnedOffloadOfferFailure, OwnedOffloadOfferOutcome, OwnedOffloadOffersHold,
    OwnedSettledOffloadOffers, OwnedVerifiedOffloadOffers, SettledOffloadOfferReport,
    publish_owned_offload_offers, verify_owned_offload_offers,
};
pub use reconciliation::{
    OffloadReconciliation, OffloadReconciliationDisposition, OffloadReconciliationFailure,
    OffloadReconciliationGroup, OffloadReconciliationReport, OffloadReconciliationStop,
    reconcile_offload_empty,
};

/// Requested coordinates only; the owning capture independently reads them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OffloadOfferSelection {
    pub target: Target,
    pub layout: SegmentLayout,
    pub names: Vec<String>,
}

#[derive(Debug)]
pub enum OffloadOfferRefusal {
    Unsupported,
    InvalidSelection,
    Changed,
    Kept(crate::policy::Eligibility),
    Unproven,
    Admission(solstone_core_backup::archive_admission::AdmissionError),
    Io(std::io::Error),
    Review(crate::review_context::ReviewRefusal),
    Policy(Box<crate::remove_marked::ReviewedRemovalRefusal>),
    Proof(crate::archive_proof::ProofError),
    Readback(ReadbackError),
    Approval,
    Cleanup(solstone_core_journal_io::media::Cleanup),
    InsufficientSpace { available: u64, required: u64 },
    Metadata,
    Catalog(String),
    Register(Box<crate::mark_mutation::BoundMutationRefusal>),
}

impl From<std::io::Error> for OffloadOfferRefusal {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

enum OfferArchiveOwner<'clock> {
    Pending(Box<ReadOperation<'clock>>),
    Failed(crate::archive_approval::ArchiveApprovalFailure<'clock>),
}

/// Failed verification keeps the original read, inputs and attempted targets.
#[must_use]
pub struct OffloadOfferFailure<'clock> {
    observed: Box<ObservedOffloadOffers>,
    owner: OfferArchiveOwner<'clock>,
    reason: OffloadOfferRefusal,
}

impl OffloadOfferFailure<'_> {
    pub fn reason(&self) -> &OffloadOfferRefusal {
        &self.reason
    }
    pub fn observed(&self) -> &ObservedOffloadOffers {
        &self.observed
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        match &self.owner {
            OfferArchiveOwner::Pending(read) => read.journal_root(),
            OfferArchiveOwner::Failed(failure) => failure.journal_root(),
        }
    }
    pub fn activity(&self) -> &Arc<ActivityLease> {
        match &self.owner {
            OfferArchiveOwner::Pending(read) => read.activity(),
            OfferArchiveOwner::Failed(failure) => failure.activity(),
        }
    }
    pub fn archive_reads(
        &self,
    ) -> impl Iterator<Item = crate::remove_marked::ArchiveReadState<'_>> {
        match &self.owner {
            OfferArchiveOwner::Pending(_) => crate::archive_approval::ArchiveReadStates::empty(),
            OfferArchiveOwner::Failed(failure) => failure.read_states(),
        }
    }
    pub fn readback_error(&self) -> Option<ReadbackError> {
        match &self.owner {
            OfferArchiveOwner::Failed(failure) => match failure.reason() {
                crate::archive_approval::ArchiveApprovalError::Readback(error) => Some(*error),
                _ => None,
            },
            OfferArchiveOwner::Pending(_) => None,
        }
    }
    pub fn cleanup(&self) -> Option<solstone_core_journal_io::media::Cleanup> {
        match &self.owner {
            OfferArchiveOwner::Failed(failure) => match failure.reason() {
                crate::archive_approval::ArchiveApprovalError::Cleanup(cleanup) => Some(*cleanup),
                _ => None,
            },
            OfferArchiveOwner::Pending(_) => None,
        }
    }
    pub fn native_error(&self) -> Option<&std::io::Error> {
        match &self.owner {
            OfferArchiveOwner::Failed(failure) => match failure.reason() {
                crate::archive_approval::ArchiveApprovalError::Native(error) => Some(error),
                _ => None,
            },
            OfferArchiveOwner::Pending(_) => None,
        }
    }
}

/// Actual archive confirmation; no public constructor or removal authority.
#[must_use]
pub struct VerifiedOffloadOffers<'clock> {
    observed: Box<ObservedOffloadOffers>,
    approval: crate::archive_approval::ArchiveApprovalBatch<'clock>,
}

impl VerifiedOffloadOffers<'_> {
    pub fn observed(&self) -> &ObservedOffloadOffers {
        &self.observed
    }
    pub fn marks(&self) -> &[crate::marks::Mark] {
        self.approval.expected_marks()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.approval.journal_root()
    }
    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.approval.activity()
    }
    pub fn archive_reads(
        &self,
    ) -> impl Iterator<Item = crate::remove_marked::ArchiveReadState<'_>> {
        self.approval.read_states()
    }
}

/// Publication facts and all original verification owners remain together.
#[must_use]
pub struct OffloadOfferOutcome<'clock> {
    verified: VerifiedOffloadOffers<'clock>,
    publication: Result<crate::mark_mutation::BoundMutationOutcome, OffloadOfferRefusal>,
}

impl OffloadOfferOutcome<'_> {
    pub fn verified(&self) -> &VerifiedOffloadOffers<'_> {
        &self.verified
    }
    pub fn publication(
        &self,
    ) -> &Result<crate::mark_mutation::BoundMutationOutcome, OffloadOfferRefusal> {
        &self.publication
    }
}

/// Actual retained bytes and accepted processing observed under writer claims.
pub struct ObservedOffloadFile {
    name: String,
    observation: MediaObservation,
    processing: AcceptedProcessingObservation,
}

impl ObservedOffloadFile {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn observation(&self) -> &MediaObservation {
        &self.observation
    }
    pub fn processing(&self) -> &AcceptedProcessingObservation {
        &self.processing
    }
}

pub struct ObservedOffloadGroup {
    selection: OffloadOfferSelection,
    directory: Arc<MediaDirectory>,
    #[cfg(any(unix, windows))]
    owner: solstone_core_segment::RetainedOwnerDeletion,
    #[cfg(any(unix, windows))]
    scan: crate::scan::CheckedSegment,
    files: Vec<ObservedOffloadFile>,
    expected: Option<Mark>,
}

impl ObservedOffloadGroup {
    pub fn selection(&self) -> &OffloadOfferSelection {
        &self.selection
    }
    pub fn directory(&self) -> &Arc<MediaDirectory> {
        &self.directory
    }
    pub fn files(&self) -> &[ObservedOffloadFile] {
        &self.files
    }
    pub fn expected_mark(&self) -> Option<&Mark> {
        self.expected.as_ref()
    }
}

/// An owning observation, with no public constructor or archive authority.
#[must_use]
pub struct ObservedOffloadOffers {
    root: Arc<JournalRoot>,
    activity: Arc<ActivityLease>,
    groups: Vec<ObservedOffloadGroup>,
    #[cfg(any(unix, windows))]
    settings: ReviewedSettings,
}

/// The journal settings an offload decides with: the retention policy and
/// whether every original is kept. Planning observes exactly these, and
/// backup and offload settings are pinned by the archive admission itself.
/// Anything else the journal records in the same file while an offload runs,
/// such as the record a scheduled backup, prune or verify writes when it
/// finishes, is not a reason to stop.
#[cfg(any(unix, windows))]
#[derive(PartialEq)]
struct ReviewedSettings {
    retention: Option<serde_json::Value>,
    preserve_all: Option<serde_json::Value>,
}

#[cfg(any(unix, windows))]
impl ReviewedSettings {
    fn of(policy: &crate::remove_marked::ReviewedPolicy) -> Self {
        let config = policy.observation().0.config.as_ref();
        Self {
            retention: config.and_then(|config| config.get("retention")).cloned(),
            preserve_all: config
                .and_then(|config| config.get("transcribe"))
                .and_then(|transcribe| transcribe.get("preserve_all"))
                .cloned(),
        }
    }
}

impl ObservedOffloadOffers {
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn activity(&self) -> &Arc<ActivityLease> {
        &self.activity
    }
    pub fn groups(&self) -> &[ObservedOffloadGroup] {
        &self.groups
    }
}

#[cfg(any(unix, windows))]
fn selected_id(selection: &OffloadOfferSelection) -> MarkId {
    MarkId::derive(
        RemovalClass::OffloadRawRelease,
        &selection.target,
        &selection.names,
    )
}

#[cfg(any(unix, windows))]
fn selection_path(selection: &OffloadOfferSelection) -> String {
    crate::layout::segment_rel_with_layout(
        &selection.target.day,
        &selection.target.stream,
        &selection.target.dir,
        selection.layout,
    )
}

#[cfg(any(unix, windows))]
fn validate_selections(selections: &[OffloadOfferSelection]) -> Result<(), OffloadOfferRefusal> {
    use std::collections::BTreeSet;

    use solstone_core_journal_io::paths::DEFAULT_STREAM;

    use crate::content::{ContentName, JournalMedia, MediaClassifier};

    let mut ids = BTreeSet::new();
    let mut files = BTreeSet::new();
    if selections.is_empty() {
        return Err(OffloadOfferRefusal::InvalidSelection);
    }
    for selection in selections {
        let target = &selection.target;
        if target.day.len() != 8
            || !target.day.bytes().all(|byte| byte.is_ascii_digit())
            || NaiveDate::parse_from_str(&target.day, "%Y%m%d").is_err()
            || !crate::archive_catalog::codec::component(&target.stream)
            || !crate::archive_catalog::codec::component(&target.dir)
            || (selection.layout == SegmentLayout::Direct && target.stream != DEFAULT_STREAM)
            || selection.names.is_empty()
            || selection.names.windows(2).any(|names| {
                names
                    .first()
                    .zip(names.get(1))
                    .is_some_and(|(left, right)| left >= right)
            })
            || !ids.insert(selected_id(selection))
        {
            return Err(OffloadOfferRefusal::InvalidSelection);
        }
        for name in &selection.names {
            if !crate::archive_catalog::codec::component(name)
                || !ContentName::new(name).is_some_and(|name| JournalMedia.is_owner_media(&name))
                || !files.insert((selection_path(selection), name))
            {
                return Err(OffloadOfferRefusal::InvalidSelection);
            }
        }
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn scan(directory: &MediaDirectory) -> Result<crate::scan::CheckedSegment, OffloadOfferRefusal> {
    Ok(crate::scan::scan_segment_bound(
        directory,
        &crate::content::ClosedHandlerSet,
        &crate::content::JournalMedia,
        4096,
    )?)
}

#[cfg(any(unix, windows))]
fn check_group(group: &ObservedOffloadGroup) -> Result<(), OffloadOfferRefusal> {
    group.directory.revalidate()?;
    if group
        .owner
        .state()
        .map_err(|error| OffloadOfferRefusal::Io(std::io::Error::other(error)))?
        != solstone_core_segment::OwnerDeletionState::Live
    {
        return Err(OffloadOfferRefusal::Changed);
    }
    let current = scan(&group.directory)?;
    if current.files().len() != group.scan.files().len()
        || group
            .scan
            .files()
            .iter()
            .zip(current.files())
            .any(|(before, after)| {
                before.name != after.name
                    || before.size != after.size
                    || before.sidecar.record != after.sidecar.record
                    || before.sidecar.has_analysis_row != after.sidecar.has_analysis_row
                    || group.scan.bound_entry(&before.name) != current.bound_entry(&after.name)
            })
    {
        return Err(OffloadOfferRefusal::Changed);
    }
    Ok(())
}

#[cfg(any(unix, windows))]
struct ClaimedSegment {
    relative: String,
    absolute: std::path::PathBuf,
    directory: Arc<MediaDirectory>,
    claims: solstone_core_segment::output_attempt::OutputClaims,
}

#[cfg(any(unix, windows))]
fn acquire_claims(
    root: &Arc<JournalRoot>,
    selections: &[OffloadOfferSelection],
) -> Result<Vec<ClaimedSegment>, OffloadOfferRefusal> {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use solstone_core_journal_io::LockOptions;
    use solstone_core_segment::output_attempt::OutputClaims;

    let options = LockOptions {
        timeout: Duration::ZERO,
        ..LockOptions::default()
    };
    selections
        .iter()
        .map(selection_path)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|relative| {
            let directory = MediaDirectory::open(Arc::clone(root), Path::new(&relative))?;
            let absolute = root
                .canonical_path()
                .join(&relative)
                .components()
                .collect::<std::path::PathBuf>();
            let claims = OutputClaims::discover(&absolute, options)?;
            if claims.segment_identity(&absolute)? != directory.object_identity() {
                return Err(OffloadOfferRefusal::Changed);
            }
            Ok(ClaimedSegment {
                relative,
                absolute,
                directory,
                claims,
            })
        })
        .collect()
}

/// Capture each distinct selected file once, under the existing writer protocol.
/// The returned observation retains inputs and the original activity, not locks.
pub fn capture_offload_offers(
    operation: &PreparedArchiveOperation<'_>,
    selections: &[OffloadOfferSelection],
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<ObservedOffloadOffers, OffloadOfferRefusal> {
    #[cfg(not(any(unix, windows)))]
    {
        let _parameters = (operation, selections, today, now);
        Err(OffloadOfferRefusal::Unsupported)
    }
    #[cfg(any(unix, windows))]
    {
        use crate::grouped::{DecisionPurpose, inspect_grouped};
        use std::ffi::OsStr;

        validate_selections(selections)?;
        operation
            .revalidate()
            .map_err(OffloadOfferRefusal::Admission)?;
        let root = Arc::clone(operation.journal_root());
        let mut selections = selections.to_vec();
        selections.sort_by_key(selection_path);
        let claimed = acquire_claims(&root, &selections)?;
        let mut commits = Vec::new();
        for segment in &claimed {
            let absolute = &segment.absolute;
            let commit = segment.claims.commit(absolute)?;
            commit.require_outputs(
                &segment.claims,
                absolute,
                selections
                    .iter()
                    .filter(|selection| selection_path(selection) == segment.relative)
                    .flat_map(|selection| selection.names.iter())
                    .map(|name| absolute.join(name).with_extension("jsonl")),
            )?;
            commits.push(commit);
        }
        let policy = crate::remove_marked::ReviewedPolicy::capture(&root)
            .map_err(|error| OffloadOfferRefusal::Policy(Box::new(error)))?;
        let register =
            crate::review_context::load_at_root(&root).map_err(OffloadOfferRefusal::Review)?;
        let mut groups = Vec::new();
        for selection in selections {
            let directory = Arc::clone(
                &claimed
                    .iter()
                    .find(|segment| segment.relative == selection_path(&selection))
                    .ok_or(OffloadOfferRefusal::Changed)?
                    .directory,
            );
            let parent_relative = crate::layout::stream_rel_with_layout(
                &selection.target.day,
                &selection.target.stream,
                selection.layout,
            );
            let parent = MediaDirectory::open(Arc::clone(&root), Path::new(&parent_relative))?;
            let owner = solstone_core_segment::RetainedOwnerDeletion::open(
                parent,
                OsStr::new(&selection.target.dir),
                Some(&directory),
                4096,
            )
            .map_err(|error| OffloadOfferRefusal::Io(std::io::Error::other(error)))?;
            if owner
                .state()
                .map_err(|error| OffloadOfferRefusal::Io(std::io::Error::other(error)))?
                != solstone_core_segment::OwnerDeletionState::Live
            {
                return Err(OffloadOfferRefusal::Changed);
            }
            let scan = scan(&directory)?;
            let inspection = inspect_grouped(
                &scan,
                &selection.target,
                selection.layout,
                policy.policy(),
                today,
                now,
                DecisionPurpose::OffloadProposal,
            );
            let mut files = Vec::new();
            for name in &selection.names {
                let content_name = crate::content::ContentName::new(name)
                    .ok_or(OffloadOfferRefusal::InvalidSelection)?;
                let entry = scan
                    .bound_entry(&content_name)
                    .ok_or(OffloadOfferRefusal::Changed)?;
                let accepted = inspection
                    .eligible()
                    .find(|file| file.name() == name)
                    .ok_or_else(|| {
                        let group = if inspection.empty().members().contains(name) {
                            inspection.empty()
                        } else {
                            inspection.ordinary()
                        };
                        match group.verdict() {
                            crate::grouped::GroupVerdict::Kept(verdict) => {
                                OffloadOfferRefusal::Kept(*verdict)
                            }
                            _ => OffloadOfferRefusal::Unproven,
                        }
                    })?;
                let before = directory
                    .read_prefix(OsStr::new(name), 0)?
                    .ok_or(OffloadOfferRefusal::Changed)?;
                if before.entry != *entry {
                    return Err(OffloadOfferRefusal::Changed);
                }
                let observation = directory
                    .observe_file(OsStr::new(name))?
                    .ok_or(OffloadOfferRefusal::Changed)?;
                let after = directory
                    .read_prefix(OsStr::new(name), 0)?
                    .ok_or(OffloadOfferRefusal::Changed)?;
                if after.entry != *entry
                    || observation.identity != entry.identity
                    || observation.digest.size != accepted.bytes()
                {
                    return Err(OffloadOfferRefusal::Changed);
                }
                files.push(ObservedOffloadFile {
                    name: name.clone(),
                    observation,
                    processing: accepted.processing().clone(),
                });
            }
            let expected = register.marks.get(&selected_id(&selection)).cloned();
            let group = ObservedOffloadGroup {
                selection,
                directory,
                owner,
                scan,
                files,
                expected,
            };
            check_group(&group)?;
            groups.push(group);
        }
        let observed = ObservedOffloadOffers {
            root,
            activity: Arc::clone(operation.activity()),
            groups,
            settings: ReviewedSettings::of(&policy),
        };
        for commit in &commits {
            commit.revalidate()?;
        }
        check_capture(&observed, operation, &policy)?;
        Ok(observed)
    }
}

#[cfg(any(unix, windows))]
fn check_capture(
    observed: &ObservedOffloadOffers,
    operation: &PreparedArchiveOperation<'_>,
    policy: &crate::remove_marked::ReviewedPolicy,
) -> Result<(), OffloadOfferRefusal> {
    if !Arc::ptr_eq(&observed.root, operation.journal_root())
        || !Arc::ptr_eq(&observed.activity, operation.activity())
        || ReviewedSettings::of(policy) != observed.settings
    {
        return Err(OffloadOfferRefusal::Changed);
    }
    operation
        .revalidate()
        .map_err(OffloadOfferRefusal::Admission)?;
    policy
        .revalidate()
        .map_err(|error| OffloadOfferRefusal::Policy(Box::new(error)))?;
    let current =
        crate::review_context::load_at_root(&observed.root).map_err(OffloadOfferRefusal::Review)?;
    for group in &observed.groups {
        if current.marks.get(&selected_id(&group.selection)) != group.expected.as_ref() {
            return Err(OffloadOfferRefusal::Changed);
        }
        check_group(group)?;
    }
    operation
        .revalidate()
        .map_err(OffloadOfferRefusal::Admission)
}

#[cfg(any(unix, windows))]
fn candidate_marks(
    observed: &ObservedOffloadOffers,
    read: &ReadOperation<'_>,
    proofs: Vec<OffloadProof>,
    now: DateTime<Utc>,
) -> Result<Vec<Mark>, OffloadOfferRefusal> {
    use std::collections::BTreeMap;

    use crate::marks::Register;

    if proofs.len() != observed.groups.len()
        || !Arc::ptr_eq(read.journal_root(), &observed.root)
        || !Arc::ptr_eq(read.activity(), &observed.activity)
    {
        return Err(OffloadOfferRefusal::InvalidSelection);
    }
    let mut by_id = BTreeMap::new();
    for proof in proofs {
        proof.validate().map_err(OffloadOfferRefusal::Proof)?;
        let names = proof
            .files
            .iter()
            .map(|file| file.name.clone())
            .collect::<Vec<_>>();
        let id = MarkId::derive(RemovalClass::OffloadRawRelease, &proof.target, &names);
        if by_id.insert(id, proof).is_some() {
            return Err(OffloadOfferRefusal::InvalidSelection);
        }
    }
    let mut marks = Vec::new();
    for group in &observed.groups {
        let id = selected_id(&group.selection);
        let proof = by_id
            .remove(&id)
            .ok_or(OffloadOfferRefusal::InvalidSelection)?;
        if proof.target != group.selection.target
            || proof.layout != group.selection.layout
            || proof.journal_id != observed.root.identity().observation_id()
            || proof.repository_id != read.repository_id()
            || proof.files.len() != group.files.len()
            || proof
                .files
                .iter()
                .zip(&group.files)
                .any(|(file, captured)| {
                    file.name != captured.name
                        || file.bytes != captured.observation.digest.size
                        || file.sha256 != captured.observation.digest.sha256
                        || file.processing != captured.processing
                })
        {
            return Err(OffloadOfferRefusal::Changed);
        }
        marks.push(candidate_mark(group, proof, now)?);
    }
    let register = Register {
        version: 2,
        marks: marks
            .iter()
            .map(|mark| (mark.id.clone(), mark.clone()))
            .collect(),
    };
    crate::marks::validate(&register).map_err(|error| {
        OffloadOfferRefusal::Review(crate::review_context::ReviewRefusal::Register(error))
    })?;
    if serde_json::to_vec_pretty(&register)
        .map_err(|_| OffloadOfferRefusal::Metadata)?
        .len()
        > crate::review_context::MAX_REGISTER_BYTES
    {
        return Err(OffloadOfferRefusal::Metadata);
    }
    Ok(marks)
}

#[cfg(any(unix, windows))]
fn candidate_mark(
    group: &ObservedOffloadGroup,
    proof: OffloadProof,
    now: DateTime<Utc>,
) -> Result<Mark, OffloadOfferRefusal> {
    use crate::archive_proof::OffloadOffer;
    use crate::marks::{MarkState, Proposal};
    if let Some(expected) = &group.expected
        && expected.state == MarkState::Marked
        && expected
            .offload
            .as_ref()
            .is_some_and(|offer| offer.proof == proof)
    {
        return Ok(expected.clone());
    }
    let bytes = proof.bytes().map_err(OffloadOfferRefusal::Proof)?;
    let reason = format!("restic-snapshot:{}", proof.snapshot_id);
    let offload = OffloadOffer::new(proof).map_err(OffloadOfferRefusal::Proof)?;
    Ok(Mark {
        id: selected_id(&group.selection),
        class: RemovalClass::OffloadRawRelease,
        target: group.selection.target.clone(),
        marked_at: now.to_rfc3339(),
        proposal: Proposal {
            names: group.selection.names.clone(),
            bytes,
            reason,
        },
        state: MarkState::Marked,
        offload: Some(offload),
    })
}

#[cfg(any(unix, windows))]
fn check_current_metadata(
    observed: &ObservedOffloadOffers,
    policy: &crate::remove_marked::ReviewedPolicy,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<(), OffloadOfferRefusal> {
    if ReviewedSettings::of(policy) != observed.settings {
        return Err(OffloadOfferRefusal::Changed);
    }
    policy
        .revalidate()
        .map_err(|error| OffloadOfferRefusal::Policy(Box::new(error)))?;
    let current =
        crate::review_context::load_at_root(&observed.root).map_err(OffloadOfferRefusal::Review)?;
    for group in &observed.groups {
        if current.marks.get(&selected_id(&group.selection)) != group.expected.as_ref() {
            return Err(OffloadOfferRefusal::Changed);
        }
        check_group(group)?;
        let inspection = crate::grouped::inspect_grouped(
            &group.scan,
            &group.selection.target,
            group.selection.layout,
            policy.policy(),
            today,
            now,
            crate::grouped::DecisionPurpose::OffloadProposal,
        );
        for captured in &group.files {
            if !inspection
                .eligible()
                .any(|file| file.name() == captured.name)
            {
                return Err(OffloadOfferRefusal::Changed);
            }
        }
    }
    policy
        .revalidate()
        .map_err(|error| OffloadOfferRefusal::Policy(Box::new(error)))
}

#[cfg(any(unix, windows))]
fn check_current_inputs(
    observed: &ObservedOffloadOffers,
    policy: &crate::remove_marked::ReviewedPolicy,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<(), OffloadOfferRefusal> {
    use std::ffi::OsStr;
    check_current_metadata(observed, policy, today, now)?;
    for group in &observed.groups {
        for captured in &group.files {
            let current = group
                .directory
                .observe_file(OsStr::new(&captured.name))?
                .ok_or(OffloadOfferRefusal::Changed)?;
            if current != captured.observation {
                return Err(OffloadOfferRefusal::Changed);
            }
        }
    }
    check_current_metadata(observed, policy, today, now)
}

#[cfg(any(unix, windows))]
fn admission_refusal(error: crate::archive_approval::ArchiveApprovalError) -> OffloadOfferRefusal {
    use crate::archive_approval::ArchiveApprovalError;
    match error {
        ArchiveApprovalError::Readback(error) => OffloadOfferRefusal::Readback(error),
        ArchiveApprovalError::Native(error) => OffloadOfferRefusal::Io(error),
        ArchiveApprovalError::Cleanup(cleanup) => OffloadOfferRefusal::Cleanup(cleanup),
        ArchiveApprovalError::InvalidSelection => OffloadOfferRefusal::InvalidSelection,
        ArchiveApprovalError::SourceRootChanged
        | ArchiveApprovalError::RepositoryChanged
        | ArchiveApprovalError::RootChanged => OffloadOfferRefusal::Changed,
    }
}

#[cfg(any(unix, windows))]
fn offer_metadata_bytes(marks: &[Mark]) -> Result<u64, OffloadOfferRefusal> {
    let initial = (crate::review_context::MAX_REGISTER_BYTES as u64)
        .checked_add(1024 * 1024)
        .ok_or(OffloadOfferRefusal::Metadata)?;
    marks.iter().try_fold(initial, |bytes, mark| {
        let proof = &mark
            .offload
            .as_ref()
            .ok_or(OffloadOfferRefusal::InvalidSelection)?
            .proof;
        let event = crate::archive_catalog::codec::Event::ArchiveV2 {
            schema_version: 2,
            time: u64::MAX,
            day: proof.target.day.clone(),
            stream: proof.target.stream.clone(),
            segment: solstone_core_format::segment::segment_key(&proof.target.dir)
                .ok_or(OffloadOfferRefusal::InvalidSelection)?,
            segment_dir: proof.target.dir.clone(),
            layout: proof.layout,
            journal_id: proof.journal_id.clone(),
            manifest_id: proof.manifest_id.clone(),
            repository_id: proof.repository_id.clone(),
            snapshot_id: proof.snapshot_id.clone(),
            files: proof.files.clone(),
        };
        let encoded = serde_json::to_vec(&event).map_err(|_| OffloadOfferRefusal::Metadata)?;
        bytes
            .checked_add(u64::try_from(encoded.len()).map_err(|_| OffloadOfferRefusal::Metadata)?)
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or(OffloadOfferRefusal::Metadata)
    })
}

#[cfg(any(unix, windows))]
fn preflight_verification(
    read: &ReadOperation<'_>,
    observed: &ObservedOffloadOffers,
    marks: &[Mark],
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<(), OffloadOfferRefusal> {
    read.revalidate_admission()
        .map_err(OffloadOfferRefusal::Readback)?;
    let selections = observed
        .groups
        .iter()
        .map(|group| group.selection.clone())
        .collect::<Vec<_>>();
    let claimed = acquire_claims(&observed.root, &selections)?;
    let mut commits = Vec::new();
    for segment in &claimed {
        let absolute = &segment.absolute;
        let commit = segment.claims.commit(absolute)?;
        commit.require_outputs(
            &segment.claims,
            absolute,
            selections
                .iter()
                .filter(|selection| selection_path(selection) == segment.relative)
                .flat_map(|selection| &selection.names)
                .map(|name| absolute.join(name).with_extension("jsonl")),
        )?;
        if observed
            .groups
            .iter()
            .filter(|group| selection_path(&group.selection) == segment.relative)
            .any(|group| group.directory.object_identity() != segment.directory.object_identity())
        {
            return Err(OffloadOfferRefusal::Changed);
        }
        commits.push(commit);
    }
    let policy = crate::remove_marked::ReviewedPolicy::capture(&observed.root)
        .map_err(|error| OffloadOfferRefusal::Policy(Box::new(error)))?;
    check_current_inputs(observed, &policy, today, now)?;
    let maximum = marks
        .iter()
        .filter_map(|mark| mark.offload.as_ref())
        .flat_map(|offer| &offer.proof.files)
        .map(|file| file.bytes)
        .max()
        .ok_or(OffloadOfferRefusal::InvalidSelection)?;
    let required = read
        .offload_required_bytes(maximum, offer_metadata_bytes(marks)?)
        .map_err(OffloadOfferRefusal::Readback)?;
    let parent = MediaDirectory::open(Arc::clone(read.journal_root()), Path::new(""))?;
    let available = parent.available_bytes()?;
    if available < required {
        return Err(OffloadOfferRefusal::InsufficientSpace {
            available,
            required,
        });
    }
    parent.revalidate()?;
    for commit in &commits {
        commit.revalidate()?;
    }
    read.revalidate_admission()
        .map_err(OffloadOfferRefusal::Readback)
}

/// Verify captured inputs through actual sequential archive readback. All short
/// locks end before helper work; every result retains the original read owners.
/// Runtime cooperation/settlement remains a prerequisite for invocation.
pub fn verify_offload_offers<'clock>(
    read: ReadOperation<'clock>,
    observed: ObservedOffloadOffers,
    proofs: Vec<OffloadProof>,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<VerifiedOffloadOffers<'clock>, OffloadOfferFailure<'clock>> {
    let observed = Box::new(observed);
    #[cfg(any(unix, windows))]
    let candidates = candidate_marks(&observed, &read, proofs, now).and_then(|marks| {
        preflight_verification(&read, &observed, &marks, today, now)?;
        Ok(marks)
    });
    #[cfg(not(any(unix, windows)))]
    let candidates: Result<Vec<crate::marks::Mark>, OffloadOfferRefusal> = {
        let _parameters = (proofs, today, now);
        Err(OffloadOfferRefusal::Unsupported)
    };
    let marks = match candidates {
        Ok(marks) => marks,
        Err(reason) => {
            return Err(OffloadOfferFailure {
                observed,
                owner: OfferArchiveOwner::Pending(Box::new(read)),
                reason,
            });
        }
    };
    let carrier = Arc::clone(read.carrier());
    match crate::archive_approval::create_archive_approval_batch(read, carrier, &marks) {
        Ok(approval) => Ok(VerifiedOffloadOffers { observed, approval }),
        Err(failure) => Err(OffloadOfferFailure {
            observed,
            owner: OfferArchiveOwner::Failed(failure),
            reason: OffloadOfferRefusal::Approval,
        }),
    }
}

#[cfg(any(unix, windows))]
fn check_active_witnesses(
    root: &Arc<JournalRoot>,
    marks: &[Mark],
) -> Result<(), OffloadOfferRefusal> {
    let catalog = crate::archive_catalog::read_complete_catalog(root)
        .map_err(OffloadOfferRefusal::Catalog)?;
    let segments = || catalog.days.iter().flat_map(|day| &day.segments);
    if catalog.degraded()
        || catalog.days.iter().any(|day| day.degraded())
        || segments().any(|segment| segment.degraded())
    {
        return Err(OffloadOfferRefusal::Changed);
    }
    for mark in marks {
        let proof = &mark
            .offload
            .as_ref()
            .ok_or(OffloadOfferRefusal::InvalidSelection)?
            .proof;
        if segments()
            .flat_map(|segment| &segment.history)
            .filter(|manifest| manifest.manifest_id == proof.manifest_id)
            .count()
            != 1
        {
            return Err(OffloadOfferRefusal::Changed);
        }
        for file in &proof.files {
            if catalog.active_witness(proof, file).is_none()
                || segments()
                    .flat_map(|segment| &segment.history)
                    .flat_map(|manifest| &manifest.files)
                    .filter(|witness| witness.witness_id == file.witness_id)
                    .count()
                    != 1
            {
                return Err(OffloadOfferRefusal::Changed);
            }
        }
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn publish_with_approval(
    observed: &ObservedOffloadOffers,
    approval: crate::archive_approval::ArchiveApprovalRef<'_, '_>,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<crate::mark_mutation::BoundMutationOutcome, OffloadOfferRefusal> {
    approval.revalidate_admission().map_err(admission_refusal)?;
    if !Arc::ptr_eq(approval.journal_root(), &observed.root)
        || !Arc::ptr_eq(approval.activity(), &observed.activity)
    {
        return Err(OffloadOfferRefusal::Changed);
    }
    let selections = observed
        .groups
        .iter()
        .map(|group| group.selection.clone())
        .collect::<Vec<_>>();
    let claimed = acquire_claims(&observed.root, &selections)?;
    let mut commits = Vec::new();
    for segment in &claimed {
        let absolute = &segment.absolute;
        let commit = segment.claims.commit(absolute)?;
        commit.require_outputs(
            &segment.claims,
            absolute,
            selections
                .iter()
                .filter(|selection| selection_path(selection) == segment.relative)
                .flat_map(|selection| &selection.names)
                .map(|name| absolute.join(name).with_extension("jsonl")),
        )?;
        if observed
            .groups
            .iter()
            .filter(|group| selection_path(&group.selection) == segment.relative)
            .any(|group| group.directory.object_identity() != segment.directory.object_identity())
        {
            return Err(OffloadOfferRefusal::Changed);
        }
        commits.push(commit);
    }
    // Governing claims precede every current catalog read and remain held until
    // final register publication, including the fresh read after register lock.
    let policy = crate::remove_marked::ReviewedPolicy::capture(approval.journal_root())
        .map_err(|error| OffloadOfferRefusal::Policy(Box::new(error)))?;
    check_current_metadata(observed, &policy, today, now)?;
    check_active_witnesses(approval.journal_root(), approval.expected_marks())?;
    let selected = observed
        .groups
        .iter()
        .zip(approval.expected_marks())
        .map(|(group, mark)| (group.expected.as_ref(), mark))
        .collect::<Vec<_>>();
    let guard = crate::mark_mutation::OfferMarkGuard::begin(approval.journal_root(), &selected)
        .map_err(|error| OffloadOfferRefusal::Register(Box::new(error)))?;
    check_current_inputs(observed, &policy, today, now)?;
    check_active_witnesses(approval.journal_root(), approval.expected_marks())?;
    for commit in &commits {
        commit.revalidate()?;
    }
    approval.revalidate_admission().map_err(admission_refusal)?;
    guard
        .publish()
        .map_err(|error| OffloadOfferRefusal::Register(Box::new(error)))
}

/// Publish only after the private orchestrator retains confirmed, fully synced
/// witness-write receipts. This entry checks fresh current identity, not append
/// durability; readable history alone cannot satisfy the caller's receipt gate.
/// The returned outcome retains all verification and native owners on failure.
pub fn publish_offload_offers<'clock>(
    verified: VerifiedOffloadOffers<'clock>,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> OffloadOfferOutcome<'clock> {
    #[cfg(any(unix, windows))]
    let publication =
        publish_with_approval(&verified.observed, verified.approval.view(), today, now);
    #[cfg(not(any(unix, windows)))]
    let publication = {
        let _parameters = (today, now);
        Err(OffloadOfferRefusal::Unsupported)
    };
    OffloadOfferOutcome {
        verified,
        publication,
    }
}

#[cfg(all(test, unix))]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "retained synthetic input observations exercise the real claim and capture entry"
)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use solstone_core_backup::archive_admission::{ArchiveBudget, Clock};
    use solstone_core_journal_io::{LockOptions, paths::DEFAULT_STREAM};
    use solstone_core_segment::output_attempt::OutputClaims;
    use std::fs;
    use std::time::Duration;

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_unix(&self) -> i64 {
            1_791_590_400
        }
        fn iso_week(&self) -> u8 {
            41
        }
    }

    pub(super) fn time() -> (NaiveDate, DateTime<Utc>) {
        (
            NaiveDate::from_ymd_opt(2026, 10, 10).unwrap(),
            Utc.with_ymd_and_hms(2026, 10, 10, 0, 0, 0).unwrap(),
        )
    }

    pub(super) fn fixture() -> (tempfile::TempDir, Arc<JournalRoot>) {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir(journal.path().join("config")).unwrap();
        fs::write(journal.path().join("config/journal.json"), b"{}").unwrap();
        let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
        (journal, root)
    }

    pub(super) fn operation(root: &Arc<JournalRoot>) -> PreparedArchiveOperation<'static> {
        PreparedArchiveOperation::prepare(
            Arc::clone(root),
            ArchiveBudget::new(&FixedClock).unwrap(),
            &FixedClock,
        )
        .unwrap()
    }

    pub(super) fn selection(layout: SegmentLayout, names: &[&str]) -> OffloadOfferSelection {
        OffloadOfferSelection {
            target: Target {
                day: "20261009".into(),
                stream: match layout {
                    SegmentLayout::Direct => DEFAULT_STREAM.into(),
                    SegmentLayout::Named => "camera".into(),
                },
                dir: "120000_60_suffix".into(),
            },
            layout,
            names: names.iter().map(|name| (*name).into()).collect(),
        }
    }

    pub(super) fn seed(
        root: &JournalRoot,
        selection: &OffloadOfferSelection,
        name: &str,
        state: &str,
    ) {
        let directory = root.canonical_path().join(selection_path(selection));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join(name), b"current").unwrap();
        let header = json!({"_solstone_processing": {
            "schema":"solstone.processing.v1", "handler":"transcribe",
            "state":state, "reason_code":if state == "empty" { "no_decodable_audio" } else { "ok" },
            "input_size":7, "attempted_at":"2026-10-09T00:00:00Z"
        }});
        let rows = if state == "analyzed" {
            "{\"start\":0,\"text\":\"words\"}\n"
        } else {
            ""
        };
        fs::write(
            directory.join(name).with_extension("jsonl"),
            format!("{header}\n{rows}"),
        )
        .unwrap();
    }

    pub(super) fn capture(
        operation: &PreparedArchiveOperation<'_>,
        selections: &[OffloadOfferSelection],
    ) -> Result<ObservedOffloadOffers, OffloadOfferRefusal> {
        let (today, now) = time();
        capture_offload_offers(operation, selections, today, now)
    }

    pub(super) fn proof(observed: &ObservedOffloadOffers) -> OffloadProof {
        let group = &observed.groups()[0];
        OffloadProof {
            journal_id: observed.journal_root().identity().observation_id(),
            manifest_id: "manifest".into(),
            target: group.selection().target.clone(),
            layout: group.selection().layout,
            repository_id: "a".repeat(64),
            snapshot_id: "b".repeat(64),
            files: group
                .files()
                .iter()
                .map(|file| crate::archive_proof::ArchiveFileProof {
                    witness_id: format!("witness-{}", file.name()),
                    name: file.name().into(),
                    bytes: file.observation().digest.size,
                    sha256: file.observation().digest.sha256.clone(),
                    archive_path: format!("/owned/{}", file.name()),
                    processing: file.processing().clone(),
                })
                .collect(),
        }
    }

    fn save_mark(root: &JournalRoot, mark: &Mark) {
        let mut register = crate::marks::Register::empty();
        register.version = 2;
        register.marks.insert(mark.id.clone(), mark.clone());
        crate::marks::validate(&register).unwrap();
        fs::create_dir_all(root.canonical_path().join("health")).unwrap();
        fs::write(
            root.canonical_path().join("health/retention-marks.json"),
            serde_json::to_vec(&register).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn actual_captured_pending_offer_reuses_full_row_and_reoffer_renews_consent() {
        let (_journal, root) = fixture();
        let selected = selection(SegmentLayout::Named, &["audio.flac"]);
        seed(&root, &selected, "audio.flac", "analyzed");
        let operation = operation(&root);
        let observed = capture(&operation, std::slice::from_ref(&selected)).unwrap();
        let proof = proof(&observed);
        let (_, now) = time();
        let mut first = candidate_mark(&observed.groups()[0], proof.clone(), now).unwrap();
        first.proposal.reason = "original proposal".into();
        first.marked_at = "original time".into();
        save_mark(&root, &first);
        let observed = capture(&operation, std::slice::from_ref(&selected)).unwrap();
        assert_eq!(
            candidate_mark(&observed.groups()[0], proof.clone(), now).unwrap(),
            first
        );
        let mut changed = proof.clone();
        changed.snapshot_id = "c".repeat(64);
        let renewed = candidate_mark(&observed.groups()[0], changed, now).unwrap();
        assert_ne!(
            renewed.offload.as_ref().unwrap().nonce,
            first.offload.as_ref().unwrap().nonce
        );
        assert_ne!(
            renewed.offload.as_ref().unwrap().revision,
            first.offload.as_ref().unwrap().revision
        );
        let mut failed = first.clone();
        failed.state = crate::marks::MarkState::Failed(crate::marks::Failure {
            at: "failed time".into(),
            reason: "failed before raw claim".into(),
            staged: None,
        });
        save_mark(&root, &failed);
        let observed = capture(&operation, std::slice::from_ref(&selected)).unwrap();
        let renewed = candidate_mark(&observed.groups()[0], proof, now).unwrap();
        assert_ne!(
            renewed.offload.as_ref().unwrap().nonce,
            first.offload.as_ref().unwrap().nonce
        );
        assert_eq!(renewed.state, crate::marks::MarkState::Marked);
        assert_eq!(
            crate::review_context::load_at_root(&root)
                .unwrap()
                .marks
                .get(&failed.id),
            Some(&failed)
        );
    }

    #[test]
    fn private_offer_register_phase_supports_absence_and_refuses_full_row_drift() {
        let (_journal, root) = fixture();
        let selected = selection(SegmentLayout::Named, &["audio.flac"]);
        seed(&root, &selected, "audio.flac", "analyzed");
        let operation = operation(&root);
        let observed = capture(&operation, std::slice::from_ref(&selected)).unwrap();
        let (_, now) = time();
        let first = candidate_mark(&observed.groups()[0], proof(&observed), now).unwrap();
        fs::create_dir(root.canonical_path().join("health")).unwrap();
        let result = crate::mark_mutation::OfferMarkGuard::begin(&root, &[(None, &first)])
            .unwrap()
            .publish()
            .unwrap();
        assert!(matches!(
            result,
            crate::mark_mutation::BoundMutationOutcome::Published(_)
        ));
        let result = crate::mark_mutation::OfferMarkGuard::begin(&root, &[(Some(&first), &first)])
            .unwrap()
            .publish()
            .unwrap();
        assert!(
            matches!(result, crate::mark_mutation::BoundMutationOutcome::Unchanged(ref result) if result.status == crate::mark_mutation::MutationStatus::AlreadyApplied)
        );
        let mut newer = first.clone();
        newer.proposal.reason = "newer row".into();
        save_mark(&root, &newer);
        let path = root.canonical_path().join("health/retention-marks.json");
        let bytes = fs::read(&path).unwrap();
        assert!(matches!(
            crate::mark_mutation::OfferMarkGuard::begin(&root, &[(Some(&first), &first)]),
            Err(crate::mark_mutation::BoundMutationRefusal::Review(
                crate::review_context::ReviewRefusal::Stale { .. }
            ))
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(&path, b"{torn").unwrap();
        assert!(crate::mark_mutation::OfferMarkGuard::begin(&root, &[(None, &first)]).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{torn");
        // This private writer-leaf control does not manufacture a Verified offer
        // or claim the mandatory actual actor/current-witness composition gate.
    }

    #[test]
    fn actual_capture_both_layouts_absent_register_and_released_claims() {
        for layout in [SegmentLayout::Direct, SegmentLayout::Named] {
            let (_journal, root) = fixture();
            let selected = selection(layout, &["audio.flac"]);
            seed(&root, &selected, "audio.flac", "analyzed");
            let operation = operation(&root);
            let observed = capture(&operation, std::slice::from_ref(&selected)).unwrap();
            assert!(Arc::ptr_eq(
                observed.journal_root(),
                operation.journal_root()
            ));
            assert!(Arc::ptr_eq(observed.activity(), operation.activity()));
            assert_eq!(observed.groups().len(), 1);
            let group = &observed.groups()[0];
            assert!(group.expected.is_none());
            assert_eq!(group.files()[0].observation().digest.size, 7);
            assert_eq!(
                group.files()[0].observation().digest.sha256,
                format!("{:x}", Sha256::digest(b"current"))
            );
            assert!(group.files()[0].processing().has_analysis_row);
            assert!(
                !root
                    .canonical_path()
                    .join("health/retention-marks.json")
                    .exists()
            );
            let path = root.canonical_path().join(selection_path(&selected));
            let claims = OutputClaims::discover(
                &path,
                LockOptions {
                    timeout: Duration::ZERO,
                    ..LockOptions::default()
                },
            )
            .unwrap();
            let commit = claims.commit(&path).unwrap();
            commit
                .require_outputs(&claims, &path, [path.join("audio.jsonl")])
                .unwrap();
            assert!(crate::remove_marked::ReviewedPolicy::capture(&root).is_ok());
        }
    }

    #[test]
    fn disjoint_rows_share_one_segment_claim_and_keep_exact_locators() {
        let (_journal, root) = fixture();
        let a = selection(SegmentLayout::Named, &["a.flac"]);
        let b = selection(SegmentLayout::Named, &["b.flac"]);
        let direct = selection(SegmentLayout::Direct, &["a.flac"]);
        for selected in [&a, &b, &direct] {
            seed(&root, selected, &selected.names[0], "analyzed");
        }
        let operation = operation(&root);
        let observed = capture(&operation, &[b, direct, a]).unwrap();
        assert_eq!(observed.groups().len(), 3);
        assert_eq!(
            observed
                .groups()
                .iter()
                .flat_map(|group| group.files())
                .count(),
            3
        );
        let paths = observed
            .groups()
            .iter()
            .map(|group| group.directory().object_identity())
            .collect::<Vec<_>>();
        assert_ne!(paths[0], paths[1]);
        assert_eq!(paths[1], paths[2]);
    }

    #[test]
    fn overlap_refuses_before_busy_claim_and_valid_selection_reports_contention() {
        let (_journal, root) = fixture();
        let a = selection(SegmentLayout::Named, &["a.flac"]);
        let both = selection(SegmentLayout::Named, &["a.flac", "b.flac"]);
        seed(&root, &both, "a.flac", "analyzed");
        seed(&root, &both, "b.flac", "analyzed");
        let operation = operation(&root);
        let path = root.canonical_path().join(selection_path(&a));
        let _claims = OutputClaims::discover(
            &path,
            LockOptions {
                timeout: Duration::ZERO,
                ..LockOptions::default()
            },
        )
        .unwrap();
        assert!(matches!(
            capture(&operation, &[a.clone(), both]),
            Err(OffloadOfferRefusal::InvalidSelection)
        ));
        assert!(
            matches!(capture(&operation, &[a]), Err(OffloadOfferRefusal::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        assert!(
            !root
                .canonical_path()
                .join("health/retention-marks.json")
                .exists()
        );
    }

    #[test]
    fn full_ordinary_sibling_proof_and_independent_empty_group_are_governing() {
        let (_journal, root) = fixture();
        let a = selection(SegmentLayout::Named, &["a.flac"]);
        let empty = selection(SegmentLayout::Named, &["empty.flac"]);
        seed(&root, &a, "a.flac", "analyzed");
        seed(&root, &a, "b.flac", "pending");
        seed(&root, &empty, "empty.flac", "empty");
        let operation = operation(&root);
        assert!(matches!(
            capture(&operation, &[a]),
            Err(OffloadOfferRefusal::Unproven)
        ));
        assert_eq!(
            capture(&operation, &[empty]).unwrap().groups()[0].files()[0].name(),
            "empty.flac"
        );
        assert_eq!(
            fs::read(root.canonical_path().join("config/journal.json")).unwrap(),
            b"{}"
        );
    }

    #[test]
    fn strict_boolean_preserve_all_keeps_only_empty_group() {
        for preserve in [json!(true), json!("true")] {
            let (_journal, root) = fixture();
            fs::write(
                root.canonical_path().join("config/journal.json"),
                json!({"transcribe":{"preserve_all":preserve}}).to_string(),
            )
            .unwrap();
            let empty = selection(SegmentLayout::Named, &["empty.flac"]);
            let ordinary = selection(SegmentLayout::Named, &["ordinary.flac"]);
            seed(&root, &empty, "empty.flac", "empty");
            seed(&root, &ordinary, "ordinary.flac", "analyzed");
            let operation = operation(&root);
            let captured = capture(&operation, &[empty]);
            if preserve == json!(true) {
                assert!(matches!(captured, Err(OffloadOfferRefusal::Kept(_))));
            } else {
                assert!(captured.is_ok());
            }
            assert!(capture(&operation, &[ordinary]).is_ok());
        }
    }

    #[test]
    fn malformed_register_refuses_without_replacing_bytes_or_inputs() {
        let (_journal, root) = fixture();
        let selected = selection(SegmentLayout::Named, &["audio.flac"]);
        seed(&root, &selected, "audio.flac", "analyzed");
        fs::create_dir(root.canonical_path().join("health")).unwrap();
        let path = root.canonical_path().join("health/retention-marks.json");
        fs::write(&path, b"{torn").unwrap();
        let operation = operation(&root);
        assert!(matches!(
            capture(&operation, std::slice::from_ref(&selected)),
            Err(OffloadOfferRefusal::Review(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), b"{torn");
        assert_eq!(
            fs::read(
                root.canonical_path()
                    .join(selection_path(&selected))
                    .join("audio.flac")
            )
            .unwrap(),
            b"current"
        );
    }

    #[test]
    fn raw_and_processing_drift_invalidate_actual_capture() {
        for sidecar in [false, true] {
            let (_journal, root) = fixture();
            let selected = selection(SegmentLayout::Named, &["audio.flac"]);
            seed(&root, &selected, "audio.flac", "analyzed");
            let operation = operation(&root);
            let observed = capture(&operation, std::slice::from_ref(&selected)).unwrap();
            let directory = root.canonical_path().join(selection_path(&selected));
            let path = if sidecar {
                directory.join("audio.jsonl")
            } else {
                directory.join("audio.flac")
            };
            fs::write(
                path,
                if sidecar {
                    b"{}\n".as_slice()
                } else {
                    b"changed".as_slice()
                },
            )
            .unwrap();
            assert!(matches!(
                check_group(&observed.groups()[0]),
                Err(OffloadOfferRefusal::Changed)
            ));
        }
    }

    /// A scheduled backup, prune or verify that finishes while an offload runs
    /// records its result in the journal's configuration, and other parts of
    /// the journal record there too. None of that changes what the offload
    /// decided with, so it carries on; a retention setting that does change
    /// still stops it.
    #[test]
    fn a_finished_backup_run_record_is_not_a_journal_change() {
        for setting in [false, true] {
            let (_journal, root) = fixture();
            let selected = selection(SegmentLayout::Named, &["audio.flac"]);
            seed(&root, &selected, "audio.flac", "analyzed");
            let operation = operation(&root);
            let observed = capture(&operation, std::slice::from_ref(&selected)).unwrap();
            let journal = root.canonical_path();
            solstone_core_backup::record_backup_result(
                journal,
                "ok",
                json!(1_791_590_400),
                json!("c".repeat(64)),
                serde_json::Value::Null,
                None,
                None,
            )
            .unwrap();
            solstone_core_backup::record_prune_result(
                journal,
                "ok",
                json!(1_791_590_401),
                serde_json::Value::Null,
            )
            .unwrap();
            solstone_core_backup::record_verification_result(
                journal,
                "ok",
                json!(1_791_590_402),
                serde_json::Value::Null,
                json!("1/52"),
            )
            .unwrap();
            let path = journal.join("config/journal.json");
            let mut config: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            config["daily_inputs"] = json!({"since": "20261009"});
            if setting {
                config["retention"] = json!({"raw_media": "days", "raw_media_days": 1});
            }
            fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
            let policy = crate::remove_marked::ReviewedPolicy::capture(&root).unwrap();
            let (today, now) = time();
            let captured = check_capture(&observed, &operation, &policy);
            let verified = check_current_metadata(&observed, &policy, today, now);
            if setting {
                assert!(matches!(captured, Err(OffloadOfferRefusal::Changed)));
                assert!(matches!(verified, Err(OffloadOfferRefusal::Changed)));
            } else {
                assert!(captured.is_ok(), "{captured:?}");
                assert!(verified.is_ok(), "{verified:?}");
            }
        }
    }

    #[test]
    fn captured_configuration_and_exact_initial_row_are_rechecked() {
        for configuration in [false, true] {
            let (_journal, root) = fixture();
            let selected = selection(SegmentLayout::Named, &["audio.flac"]);
            seed(&root, &selected, "audio.flac", "analyzed");
            let operation = operation(&root);
            let observed = capture(&operation, std::slice::from_ref(&selected)).unwrap();
            if configuration {
                fs::write(
                    root.canonical_path().join("config/journal.json"),
                    b"{\"transcribe\":{\"preserve_all\":true}}",
                )
                .unwrap();
            } else {
                let mark = Mark {
                    id: selected_id(&selected),
                    class: RemovalClass::OffloadRawRelease,
                    target: selected.target.clone(),
                    marked_at: "now".into(),
                    proposal: crate::marks::Proposal {
                        bytes: 7,
                        reason: "archive".into(),
                        names: selected.names.clone(),
                    },
                    state: crate::marks::MarkState::Marked,
                    offload: None,
                };
                let mut register = crate::marks::Register::empty();
                register.marks.insert(mark.id.clone(), mark);
                fs::create_dir(root.canonical_path().join("health")).unwrap();
                fs::write(
                    root.canonical_path().join("health/retention-marks.json"),
                    serde_json::to_vec(&register).unwrap(),
                )
                .unwrap();
            }
            let policy = crate::remove_marked::ReviewedPolicy::capture(&root).unwrap();
            assert!(matches!(
                check_capture(&observed, &operation, &policy),
                Err(OffloadOfferRefusal::Changed)
            ));
        }
    }
}

#[cfg(all(test, feature = "full-tests", unix))]
#[path = "archive_offer/reconciliation_tests.rs"]
mod reconciliation_tests;
