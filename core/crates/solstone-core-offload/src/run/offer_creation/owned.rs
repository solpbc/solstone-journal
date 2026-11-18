// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One installed creator, its original source copies and actual offer settlement.

use std::sync::Arc;

use solstone_core_backup::archive_activity::OwnedPreparedArchiveOperation;
use solstone_core_backup::archive_readback::{
    OwnedDirectReadHold, OwnedReadOperation, OwnedSettledDirectRead,
};
use solstone_core_backup_runtime::{
    OwnedAdmittedArchive, OwnedAdmittedArchiveHold, OwnedArchiveAdmissionFailure,
};
use solstone_core_journal_io::activity_obligations::{NativeObligation, RecoveryObservationError};
use solstone_core_journal_io::media::{Cleanup, MediaDigest, MediaIdentity, RootActivityCarrier};
use solstone_core_retention::archive_catalog::JournalCatalog;
use solstone_core_retention::archive_offer::{
    OwnedOffloadOfferFailure, OwnedOffloadOfferOutcome, OwnedOffloadOffersHold,
    OwnedVerifiedOffloadOffers, SettledOffloadOfferReport,
};

use super::*;

/// Captured inputs from the same original prepared operation, before admission.
#[must_use]
pub struct OwnedOffloadInputs {
    carrier: Arc<RootActivityCarrier>,
    resources: Resources,
    catalog: JournalCatalog,
}

impl OwnedOffloadInputs {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn observed(&self) -> &ObservedOffloadOffers {
        self.resources.observed.as_ref().expect("captured inputs")
    }
    /// Nothing was copied, archived or recorded from these inputs.
    pub fn unstaged(&self) -> bool {
        self.resources.unstaged()
    }
    pub(super) fn refuse(mut self, reason: OffloadCreationRefusal) -> OwnedOffloadInputFailure {
        self.resources.reason = Some(reason);
        OwnedOffloadInputFailure {
            carrier: self.carrier,
            resources: Box::new(self.resources),
        }
    }
}

/// The caller retains this failure with the same closed prepared owner, without
/// admitting a creator or starting remote work.
#[must_use]
pub struct OwnedOffloadInputFailure {
    carrier: Arc<RootActivityCarrier>,
    resources: Box<Resources>,
}

impl OwnedOffloadInputFailure {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn observed(&self) -> Option<&ObservedOffloadOffers> {
        self.resources.observed.as_ref()
    }
    pub fn reason(&self) -> &OffloadCreationRefusal {
        self.resources.reason.as_ref().expect("refused preparation")
    }
    /// Nothing was copied, archived or recorded before this refusal.
    pub fn unstaged(&self) -> bool {
        self.resources.unstaged()
    }
}

/// Capture and check capacity before the caller admits the original creator.
pub fn prepare_owned_offload_inputs(
    original: &OwnedPreparedArchiveOperation,
    selections: &[OffloadOfferSelection],
    today: Date,
    now: Timestamp,
) -> Result<OwnedOffloadInputs, OwnedOffloadInputFailure> {
    let inputs = capture_inputs(
        original.as_prepared(),
        Arc::clone(original.carrier()),
        selections,
        today,
        now,
    )?;
    #[cfg(any(unix, windows))]
    if let Err(reason) = native::capacity(original.as_prepared(), inputs.observed()) {
        return Err(inputs.refuse(reason));
    }
    Ok(inputs)
}

// Both exact creation and the planner use this one capture. The planner performs
// its payload reserve only after inspecting actual current pending relief.
pub(super) fn capture_inputs(
    operation: &PreparedArchiveOperation<'_>,
    carrier: Arc<RootActivityCarrier>,
    selections: &[OffloadOfferSelection],
    today: Date,
    now: Timestamp,
) -> Result<OwnedOffloadInputs, OwnedOffloadInputFailure> {
    let mut resources = Resources::default();
    #[cfg(any(unix, windows))]
    let result = (|| {
        use solstone_core_retention::{
            archive_catalog::read_complete_catalog, archive_offer::capture_offload_offers,
            review_context::load_at_root,
        };
        operation
            .revalidate()
            .map_err(|error| native::inputs(OffloadOfferRefusal::Admission(error)))?;
        let catalog = read_complete_catalog(operation.journal_root())
            .map_err(|error| native::inputs(OffloadOfferRefusal::Catalog(error)))?;
        let register = load_at_root(operation.journal_root())
            .map_err(|error| native::inputs(OffloadOfferRefusal::Review(error)))?;
        let selections = native::partition(&catalog, &register, selections)?;
        resources.observed = Some(
            capture_offload_offers(operation, &selections, today, now).map_err(native::inputs)?,
        );
        Ok(catalog)
    })();
    #[cfg(not(any(unix, windows)))]
    let result: Result<JournalCatalog, OffloadCreationRefusal> = {
        let _parameters = (operation, selections, today, now);
        Err(OffloadCreationRefusal::Inputs(
            OffloadOfferRefusal::Unsupported,
        ))
    };
    match result {
        Ok(catalog) => Ok(OwnedOffloadInputs {
            carrier,
            resources,
            catalog,
        }),
        Err(reason) => {
            resources.reason = Some(reason);
            Err(OwnedOffloadInputFailure {
                carrier,
                resources: Box::new(resources),
            })
        }
    }
}

enum ActiveOwner {
    Admitted(Box<OwnedAdmittedArchive>),
    AdmissionFailure(Box<OwnedArchiveAdmissionFailure>),
    Read(Box<OwnedReadOperation>),
    VerificationFailure(Box<OwnedOffloadOfferFailure>),
    Verified(Box<OwnedVerifiedOffloadOffers>),
    Published(Box<OwnedOffloadOfferOutcome>),
}

/// Borrowed concrete phases; these views provide no extraction or finish grant.
pub enum OwnedOffloadCreationOwner<'a> {
    Admitted(&'a OwnedAdmittedArchive),
    AdmissionFailure(&'a OwnedArchiveAdmissionFailure),
    Read(&'a OwnedReadOperation),
    VerificationFailure(&'a OwnedOffloadOfferFailure),
    Verified(&'a OwnedVerifiedOffloadOffers),
    Published(&'a OwnedOffloadOfferOutcome),
}

#[must_use = "Finish this actual creator or retain its whole closed owner"]
pub struct OwnedOffloadCreation {
    carrier: Arc<RootActivityCarrier>,
    owner: ActiveOwner,
    resources: Resources,
}

impl OwnedOffloadCreation {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn refused_input_carrier(&self) -> Option<&Arc<RootActivityCarrier>> {
        self.resources.refused_input_carrier.as_ref()
    }
    pub fn owner(&self) -> OwnedOffloadCreationOwner<'_> {
        match &self.owner {
            ActiveOwner::Admitted(owner) => OwnedOffloadCreationOwner::Admitted(owner),
            ActiveOwner::AdmissionFailure(owner) => {
                OwnedOffloadCreationOwner::AdmissionFailure(owner)
            }
            ActiveOwner::Read(owner) => OwnedOffloadCreationOwner::Read(owner),
            ActiveOwner::VerificationFailure(owner) => {
                OwnedOffloadCreationOwner::VerificationFailure(owner)
            }
            ActiveOwner::Verified(owner) => OwnedOffloadCreationOwner::Verified(owner),
            ActiveOwner::Published(owner) => OwnedOffloadCreationOwner::Published(owner),
        }
    }
    pub fn reason(&self) -> Option<&OffloadCreationRefusal> {
        self.resources.reason.as_ref()
    }
    pub fn observed(&self) -> Option<&ObservedOffloadOffers> {
        match &self.owner {
            ActiveOwner::VerificationFailure(failure) => Some(failure.observed()),
            ActiveOwner::Verified(verified) => Some(verified.observed()),
            ActiveOwner::Published(outcome) => Some(outcome.verified().observed()),
            _ => self.resources.observed.as_ref(),
        }
    }
    pub fn source_copies(&self) -> impl Iterator<Item = &PreparedMedia> {
        self.resources.copies.iter().flatten()
    }
    pub fn partial_copy(&self) -> Option<OffloadCreationNative<'_>> {
        partial_copy(&self.resources)
    }
    pub fn native_error(&self) -> Option<&std::io::Error> {
        native_error(&self.resources)
    }
    pub fn archives(&self) -> &[Result<ArchiveSnapshot, ArchiveFailure>] {
        &self.resources.archives
    }
    pub fn ledger_writes(&self) -> impl Iterator<Item = OffloadLedgerWrite<'_>> {
        ledger_writes(&self.resources.ledger)
    }
    pub fn into_owned_hold(self) -> OwnedOffloadCreationHold {
        let owner = match self.owner {
            ActiveOwner::Admitted(owner) => {
                ClosedOwner::Admitted(Box::new(owner.into_owned_hold()))
            }
            ActiveOwner::AdmissionFailure(owner) => ClosedOwner::AdmissionFailure(owner),
            ActiveOwner::Read(owner) => ClosedOwner::Read(Box::new(owner.into_owned_hold())),
            ActiveOwner::VerificationFailure(owner) => {
                ClosedOwner::Offers(Box::new(owner.into_owned_hold()))
            }
            ActiveOwner::Verified(owner) => ClosedOwner::Offers(Box::new(owner.into_owned_hold())),
            ActiveOwner::Published(owner) => ClosedOwner::Offers(Box::new(owner.into_owned_hold())),
        };
        OwnedOffloadCreationHold {
            carrier: self.carrier,
            owner,
            resources: self.resources,
            cleanups: Vec::new(),
            refusal: None,
        }
    }
    /// Settle the actual writer/read before cleaning only this operation's copies.
    pub fn finish(self) -> OwnedOffloadCreationFinish {
        if self.resources.reason.is_some()
            || self.resources.partial.is_some()
            || !matches!(self.owner, ActiveOwner::Published(_))
        {
            return OwnedOffloadCreationFinish::Held(Box::new(
                self.into_owned_hold()
                    .refuse(OwnedOffloadCreationFinishRefusal::IncompleteCreation),
            ));
        }
        let Self {
            carrier,
            owner,
            mut resources,
        } = self;
        let ActiveOwner::Published(outcome) = owner else {
            unreachable!("checked publication phase")
        };
        let settled = match outcome.settle() {
            Ok(settled) => settled,
            Err(offers) => {
                return OwnedOffloadCreationFinish::Held(Box::new(OwnedOffloadCreationHold {
                    carrier,
                    owner: ClosedOwner::Offers(Box::new(offers)),
                    resources,
                    cleanups: Vec::new(),
                    refusal: Some(Box::new(OwnedOffloadCreationFinishRefusal::OfferSettlement)),
                }));
            }
        };
        let (read, observed, report) = settled.into_parts();
        let mut cleanups = Vec::new();
        let finish = (|| {
            read.revalidate()
                .map_err(OwnedOffloadCreationFinishRefusal::Readback)?;
            carrier
                .revalidate()
                .map_err(OwnedOffloadCreationFinishRefusal::Changed)?;
            carrier.activity().revalidate().map_err(|error| {
                OwnedOffloadCreationFinishRefusal::Changed(std::io::Error::other(error))
            })?;
            for (index, copy) in resources.copies.iter_mut().flatten().enumerate() {
                let cleanup = cleanup_copy(copy, index);
                cleanups.push(OffloadSourceCleanup { index, cleanup });
                if cleanup != Cleanup::Complete {
                    return Err(OwnedOffloadCreationFinishRefusal::Cleanup(cleanup));
                }
            }
            read.revalidate()
                .map_err(OwnedOffloadCreationFinishRefusal::Readback)?;
            let coverage = solstone_core_journal_io::activity_obligations::observe_with_carrier(
                carrier.journal_root(),
                carrier.observation(),
            )
            .map_err(OwnedOffloadCreationFinishRefusal::Census)?;
            if coverage.unresolved_entries().next().is_some() {
                return Err(OwnedOffloadCreationFinishRefusal::Obligations(
                    coverage.entries().to_vec(),
                ));
            }
            Ok(())
        })();
        if let Err(refusal) = finish {
            return OwnedOffloadCreationFinish::Held(Box::new(OwnedOffloadCreationHold {
                carrier,
                owner: ClosedOwner::Settled {
                    read: Box::new(read.into_owned_hold()),
                    observed: Box::new(observed),
                    report: Box::new(report),
                },
                resources,
                cleanups,
                refusal: Some(Box::new(refusal)),
            }));
        }
        let copies = resources
            .copies
            .iter()
            .flatten()
            .zip(&cleanups)
            .map(|(copy, cleanup)| OffloadSourceCopyReport {
                digest: copy.digest().clone(),
                identity: copy.identity(),
                cleanup: cleanup.cleanup,
            })
            .collect();
        let report = SettledOffloadCreationReport {
            offers: report,
            archives: resources.archives,
            ledger: resources.ledger,
            copies,
        };
        // Only actual Complete copies reach this point. Release their extra
        // activity references before the outside runtime's unique retirement.
        drop(resources.copies);
        drop(resources.observed);
        drop(observed);
        drop(carrier);
        OwnedOffloadCreationFinish::Settled(Box::new(SettledOffloadCreation { read, report }))
    }
}

fn partial_copy(resources: &Resources) -> Option<OffloadCreationNative<'_>> {
    resources.partial.as_ref().map(|partial| match partial {
        PartialCopy::Stage(failure) => OffloadCreationNative::Stage(failure.owned.as_ref()),
        PartialCopy::Target(failure) => OffloadCreationNative::Target(&failure.owned),
        PartialCopy::Copy(failure) => OffloadCreationNative::Copy(&failure.owned),
    })
}
fn native_error(resources: &Resources) -> Option<&std::io::Error> {
    resources.partial.as_ref().map(|partial| match partial {
        PartialCopy::Stage(failure) => &failure.error,
        PartialCopy::Target(failure) => &failure.error,
        PartialCopy::Copy(failure) => &failure.error,
    })
}

enum ClosedOwner {
    Admitted(Box<OwnedAdmittedArchiveHold>),
    AdmissionFailure(Box<OwnedArchiveAdmissionFailure>),
    Read(Box<OwnedDirectReadHold>),
    Offers(Box<OwnedOffloadOffersHold>),
    Settled {
        read: Box<OwnedDirectReadHold>,
        observed: Box<ObservedOffloadOffers>,
        report: Box<SettledOffloadOfferReport>,
    },
}

#[derive(Debug)]
pub enum OwnedOffloadCreationFinishRefusal {
    IncompleteCreation,
    OfferSettlement,
    Readback(ReadbackError),
    Changed(std::io::Error),
    Cleanup(Cleanup),
    Census(RecoveryObservationError),
    Obligations(Vec<NativeObligation>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OffloadSourceCleanup {
    pub index: usize,
    pub cleanup: Cleanup,
}

/// Closed custody, retaining earlier successful effects and cleanup facts.
#[must_use]
pub struct OwnedOffloadCreationHold {
    carrier: Arc<RootActivityCarrier>,
    owner: ClosedOwner,
    resources: Resources,
    cleanups: Vec<OffloadSourceCleanup>,
    refusal: Option<Box<OwnedOffloadCreationFinishRefusal>>,
}

impl OwnedOffloadCreationHold {
    fn refuse(mut self, refusal: OwnedOffloadCreationFinishRefusal) -> Self {
        self.refusal = Some(Box::new(refusal));
        self
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn refused_input_carrier(&self) -> Option<&Arc<RootActivityCarrier>> {
        self.resources.refused_input_carrier.as_ref()
    }
    pub fn reason(&self) -> Option<&OffloadCreationRefusal> {
        self.resources.reason.as_ref()
    }
    pub fn finish_refusal(&self) -> Option<&OwnedOffloadCreationFinishRefusal> {
        self.refusal.as_deref()
    }
    pub fn observed(&self) -> Option<&ObservedOffloadOffers> {
        match &self.owner {
            ClosedOwner::Offers(offers) => Some(offers.observed()),
            ClosedOwner::Settled { observed, .. } => Some(observed),
            _ => self.resources.observed.as_ref(),
        }
    }
    pub fn admitted(&self) -> Option<&OwnedAdmittedArchiveHold> {
        match &self.owner {
            ClosedOwner::Admitted(owner) => Some(owner),
            _ => None,
        }
    }
    pub fn admission_failure(&self) -> Option<&OwnedArchiveAdmissionFailure> {
        match &self.owner {
            ClosedOwner::AdmissionFailure(owner) => Some(owner),
            _ => None,
        }
    }
    pub fn read(&self) -> Option<&OwnedDirectReadHold> {
        match &self.owner {
            ClosedOwner::Read(owner) | ClosedOwner::Settled { read: owner, .. } => Some(owner),
            _ => None,
        }
    }
    pub fn offers(&self) -> Option<&OwnedOffloadOffersHold> {
        match &self.owner {
            ClosedOwner::Offers(owner) => Some(owner),
            _ => None,
        }
    }
    pub fn settled_offers(&self) -> Option<&SettledOffloadOfferReport> {
        match &self.owner {
            ClosedOwner::Settled { report, .. } => Some(report),
            _ => None,
        }
    }
    pub fn source_copies(&self) -> impl Iterator<Item = &PreparedMedia> {
        self.resources.copies.iter().flatten()
    }
    pub fn partial_copy(&self) -> Option<OffloadCreationNative<'_>> {
        partial_copy(&self.resources)
    }
    pub fn native_error(&self) -> Option<&std::io::Error> {
        native_error(&self.resources)
    }
    pub fn archives(&self) -> &[Result<ArchiveSnapshot, ArchiveFailure>] {
        &self.resources.archives
    }
    pub fn ledger_writes(&self) -> impl Iterator<Item = OffloadLedgerWrite<'_>> {
        ledger_writes(&self.resources.ledger)
    }
    pub fn source_cleanups(&self) -> &[OffloadSourceCleanup] {
        &self.cleanups
    }
}

/// Data from an actually cleaned source; it grants no execution or cleanup.
pub struct OffloadSourceCopyReport {
    pub digest: MediaDigest,
    pub identity: MediaIdentity,
    pub cleanup: Cleanup,
}

pub struct SettledOffloadCreationReport {
    offers: SettledOffloadOfferReport,
    archives: Vec<Result<ArchiveSnapshot, ArchiveFailure>>,
    ledger: Vec<Result<LedgerWriteReceipt, LedgerWriteRefusal>>,
    copies: Vec<OffloadSourceCopyReport>,
}
impl SettledOffloadCreationReport {
    pub fn offers(&self) -> &SettledOffloadOfferReport {
        &self.offers
    }
    pub fn archives(&self) -> &[Result<ArchiveSnapshot, ArchiveFailure>] {
        &self.archives
    }
    pub fn ledger_writes(&self) -> impl Iterator<Item = OffloadLedgerWrite<'_>> {
        ledger_writes(&self.ledger)
    }
    pub fn source_copies(&self) -> &[OffloadSourceCopyReport] {
        &self.copies
    }
}

#[must_use]
pub struct SettledOffloadCreation {
    read: OwnedSettledDirectRead,
    report: SettledOffloadCreationReport,
}
impl SettledOffloadCreation {
    pub fn into_parts(self) -> (OwnedSettledDirectRead, SettledOffloadCreationReport) {
        (self.read, self.report)
    }
}

#[must_use]
pub enum OwnedOffloadCreationFinish {
    Settled(Box<SettledOffloadCreation>),
    Held(Box<OwnedOffloadCreationHold>),
}

fn cleanup_copy(copy: &mut PreparedMedia, index: usize) -> Cleanup {
    #[cfg(all(test, unix, feature = "full-tests"))]
    if SOURCE_CLEANUP_FAULT.with(|armed| armed.get() == Some(index)) {
        SOURCE_CLEANUP_FAULT.with(|armed| armed.set(None));
        return solstone_core_journal_io::media::run_with_media_hook(
            solstone_core_journal_io::media::MediaPrimitive::StageRemove,
            || Err(std::io::Error::from_raw_os_error(13)),
            || copy.cleanup(),
        )
        .0;
    }
    let _index = index;
    copy.cleanup()
}

#[cfg(all(test, unix, feature = "full-tests"))]
thread_local! {
    #[cfg(target_os = "android")]
    static SOURCE_CLEANUP_FAULT: Box<std::cell::Cell<Option<usize>>> = Box::new(std::cell::Cell::new(None));
    #[cfg(not(target_os = "android"))]
    static SOURCE_CLEANUP_FAULT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
#[cfg(all(test, unix, feature = "full-tests"))]
pub(in crate::run) fn with_source_cleanup_fault<T>(
    index: usize,
    run: impl FnOnce() -> T,
) -> (T, bool) {
    SOURCE_CLEANUP_FAULT.with(|armed| assert!(armed.replace(Some(index)).is_none()));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            SOURCE_CLEANUP_FAULT.with(|armed| armed.set(None));
        }
    }
    let _reset = Reset;
    let result = run();
    let consumed = SOURCE_CLEANUP_FAULT.with(|armed| armed.get().is_none());
    (result, consumed)
}

fn fail(
    owner: ActiveOwner,
    carrier: Arc<RootActivityCarrier>,
    mut resources: Resources,
    reason: OffloadCreationRefusal,
) -> OwnedOffloadCreation {
    resources.reason = Some(reason);
    OwnedOffloadCreation {
        carrier,
        owner,
        resources,
    }
}

/// Consume only the original installed creator and its captured input value.
/// The caller keeps the SAME outside runtime finish owner across actor unwind.
pub fn create_owned_offload_offers(
    mut admitted: OwnedAdmittedArchive,
    inputs: OwnedOffloadInputs,
    today: Date,
    now: Timestamp,
) -> OwnedOffloadCreation {
    let carrier = Arc::clone(admitted.carrier());
    let OwnedOffloadInputs {
        carrier: input_carrier,
        mut resources,
        catalog,
    } = inputs;
    let observed = resources.observed.as_ref().expect("captured inputs");
    if !Arc::ptr_eq(&input_carrier, &carrier)
        || !Arc::ptr_eq(observed.journal_root(), admitted.journal_root())
        || !Arc::ptr_eq(observed.activity(), admitted.activity())
    {
        resources.refused_input_carrier = Some(input_carrier);
        return fail(
            ActiveOwner::Admitted(Box::new(admitted)),
            carrier,
            resources,
            OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Changed),
        );
    }
    drop(input_carrier);
    #[cfg(not(any(unix, windows)))]
    {
        let _parameters = (catalog, today, now);
        fail(
            ActiveOwner::Admitted(Box::new(admitted)),
            carrier,
            resources,
            OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Unsupported),
        )
    }
    #[cfg(any(unix, windows))]
    {
        let copied = (|| {
            admitted
                .revalidate_local()
                .map_err(OffloadCreationRefusal::Archive)?;
            let original = admitted.prepared_operation();
            let parent = native::capacity(
                original,
                resources.observed.as_ref().expect("captured inputs"),
            )?;
            native::copy_inputs(original, parent, &mut resources)
        })();
        if let Err(reason) = copied {
            return fail(
                ActiveOwner::Admitted(Box::new(admitted)),
                carrier,
                resources,
                reason,
            );
        }
        let plan = match native::archive_groups(
            native::ArchiveBorrow::Owned(&mut admitted),
            &mut resources,
            &catalog,
        ) {
            Ok(plan) => plan,
            Err(reason) => {
                return fail(
                    ActiveOwner::Admitted(Box::new(admitted)),
                    carrier,
                    resources,
                    reason,
                );
            }
        };
        let mut read = match admitted.into_read_operation() {
            Ok(read) => read,
            Err(failure) => {
                let reason = OffloadCreationRefusal::Archive(failure.error());
                return fail(
                    ActiveOwner::AdmissionFailure(Box::new(failure)),
                    carrier,
                    resources,
                    reason,
                );
            }
        };
        let (proofs, writes) = match native::discover_proofs(
            native::ReadBorrow::Owned(&mut read),
            &mut resources,
            plan,
        ) {
            Ok(discovered) => discovered,
            Err(reason) => {
                return fail(
                    ActiveOwner::Read(Box::new(read)),
                    carrier,
                    resources,
                    reason,
                );
            }
        };
        let observed = resources.observed.take().expect("captured inputs");
        let verified = match solstone_core_retention::archive_offer::verify_owned_offload_offers(
            read, observed, proofs, today, now,
        ) {
            Ok(verified) => verified,
            Err(failure) => {
                return fail(
                    ActiveOwner::VerificationFailure(Box::new(failure)),
                    carrier,
                    resources,
                    OffloadCreationRefusal::Verification,
                );
            }
        };
        if let Err(reason) = native::register_metadata(verified.journal_root(), verified.marks()) {
            return fail(
                ActiveOwner::Verified(Box::new(verified)),
                carrier,
                resources,
                reason,
            );
        }
        if let Err(reason) =
            native::write_witnesses(verified.journal_root(), &mut resources, writes, now)
        {
            return fail(
                ActiveOwner::Verified(Box::new(verified)),
                carrier,
                resources,
                reason,
            );
        }
        let outcome = solstone_core_retention::archive_offer::publish_owned_offload_offers(
            verified, today, now,
        );
        native::record_publication(&mut resources, outcome.publication());
        OwnedOffloadCreation {
            carrier,
            owner: ActiveOwner::Published(Box::new(outcome)),
            resources,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn require<T: Send + 'static>() {}
    #[test]
    fn closed_creation_custody_and_reports_are_send_static() {
        require::<OwnedOffloadInputFailure>();
        require::<OwnedOffloadCreationHold>();
        require::<OwnedOffloadCreationFinish>();
        require::<SettledOffloadCreation>();
        require::<SettledOffloadCreationReport>();
    }

    #[cfg(all(test, unix, feature = "full-tests"))]
    #[test]
    fn resources_retain_refused_input_carrier_across_thread_transfer() {
        use solstone_core_journal_io::activity::ActivityLease;
        use solstone_core_journal_io::journal_root::JournalRoot;
        let temp = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
        let carrier = Arc::new(
            RootActivityCarrier::prepare(activity)
                .unwrap_or_else(|failure| panic!("carrier preparation: {:?}", failure.error())),
        );
        let weak = Arc::downgrade(&carrier);
        let resources = Resources {
            refused_input_carrier: Some(carrier),
            ..Resources::default()
        };
        let resources = std::thread::spawn(move || resources).join().unwrap();
        let retained = resources.refused_input_carrier.as_ref().unwrap();
        assert!(Arc::ptr_eq(retained, &weak.upgrade().unwrap()));
        retained.revalidate().unwrap();
        assert!(ActivityLease::try_acquire(Arc::clone(&root)).is_err());
        drop(resources);
        assert!(weak.upgrade().is_none());
    }
}
