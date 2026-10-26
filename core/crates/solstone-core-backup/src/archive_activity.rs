// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One original activity reservation and an archive's nonrenewable admission.

use std::fmt;
use std::io;
use std::sync::Arc;

use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::activity_obligations::observe_obligations;
use solstone_core_journal_io::media::{
    ActivityCarrierPreparationFailure, ActivityCarrierRetirement, ActivityCarrierRetirementEffect,
    ActivityCarrierRetirementReceipt, Confirmation, Durability, RootActivityCarrier,
    retire_orphan_root_carriers,
};

use crate::archive_admission::{AdmissionError, ArchiveBudget, ArchiveConfiguration, Clock};

/// A retained reservation. Cloning its native Arc never admits another operation.
#[must_use = "Retain the original activity until its actual owning work has ended"]
pub(crate) struct ArchiveActivity {
    carrier: Arc<RootActivityCarrier>,
    origin: Box<
        Result<
            solstone_core_journal_io::archive_origin::LocalArchiveOrigin,
            solstone_core_journal_io::archive_origin::OriginObservationFailure,
        >,
    >,
}

#[must_use = "Preparation failure may retain an original lease and partial reservation"]
pub struct ActivityPreparationFailure {
    inner: Box<ActivityPreparationFailureInner>,
}

struct ActivityPreparationFailureInner {
    error: io::Error,
    root: Arc<JournalRoot>,
    lease: Option<Arc<ActivityLease>>,
    partial: Option<ActivityCarrierPreparationFailure>,
}

impl fmt::Debug for ActivityPreparationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActivityPreparationFailure")
            .field("kind", &self.inner.error.kind())
            .finish_non_exhaustive()
    }
}
impl fmt::Display for ActivityPreparationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.error.fmt(f)
    }
}
impl std::error::Error for ActivityPreparationFailure {}

impl ActivityPreparationFailure {
    pub fn error(&self) -> &io::Error {
        &self.inner.error
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.inner.root
    }
    pub fn activity(&self) -> Option<&Arc<ActivityLease>> {
        self.inner.lease.as_ref()
    }
    pub fn partial(&self) -> Option<&ActivityCarrierPreparationFailure> {
        self.inner.partial.as_ref()
    }
}

impl ArchiveActivity {
    fn prepare(root: Arc<JournalRoot>) -> Result<Self, ActivityPreparationFailure> {
        let lease = Self::acquire(Arc::clone(&root))?;
        Self::prepare_acquired(root, lease)
    }

    fn acquire(root: Arc<JournalRoot>) -> Result<Arc<ActivityLease>, ActivityPreparationFailure> {
        ActivityLease::try_acquire(Arc::clone(&root)).map_err(|error| ActivityPreparationFailure {
            inner: Box::new(ActivityPreparationFailureInner {
                error: io::Error::other(error.to_string()),
                root,
                lease: None,
                partial: None,
            }),
        })
    }

    fn prepare_acquired(
        root: Arc<JournalRoot>,
        lease: Arc<ActivityLease>,
    ) -> Result<Self, ActivityPreparationFailure> {
        let mut failure = ActivityPreparationFailureInner {
            error: io::Error::other("activity preparation failed"),
            root: Arc::clone(&root),
            lease: Some(Arc::clone(&lease)),
            partial: None,
        };
        if !Arc::ptr_eq(lease.journal_root(), &root) {
            failure.error = io::Error::other("original activity root changed");
            return Err(ActivityPreparationFailure {
                inner: Box::new(failure),
            });
        }
        if let Err(error) = lease.revalidate() {
            failure.error = io::Error::other(error);
            return Err(ActivityPreparationFailure {
                inner: Box::new(failure),
            });
        }
        // Reservations an ended owner left behind would otherwise hold every
        // later operation. The fresh census below still decides admission.
        // Desktop observation failure still reports nothing retired. An
        // unqualified phone container refuses preparation.
        if let Err(error) = retire_orphan_root_carriers(&lease) {
            failure.error = error;
            return Err(ActivityPreparationFailure {
                inner: Box::new(failure),
            });
        }
        let census = match observe_obligations(&root) {
            Ok(census) => census,
            Err(error) => {
                failure.error = io::Error::other(error.to_string());
                return Err(ActivityPreparationFailure {
                    inner: Box::new(failure),
                });
            }
        };
        if !census.entries().is_empty() {
            failure.error = io::Error::other("unresolved journal activity");
            return Err(ActivityPreparationFailure {
                inner: Box::new(failure),
            });
        }
        match RootActivityCarrier::prepare(lease) {
            Ok(carrier) => Ok(Self {
                carrier: Arc::new(carrier),
                origin: Box::new(
                    solstone_core_journal_io::archive_origin::observe_archive_origin(root),
                ),
            }),
            Err(partial) => {
                failure.error = io::Error::new(partial.error().kind(), partial.error().to_string());
                failure.partial = Some(partial);
                Err(ActivityPreparationFailure {
                    inner: Box::new(failure),
                })
            }
        }
    }

    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub(crate) fn origin(
        &self,
    ) -> Result<
        &solstone_core_journal_io::archive_origin::LocalArchiveOrigin,
        &solstone_core_journal_io::archive_origin::OriginObservationFailure,
    > {
        self.origin.as_ref().as_ref()
    }
    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.carrier.activity()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.carrier.journal_root()
    }
    pub fn revalidate(&self) -> io::Result<()> {
        self.journal_root()
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        self.activity().revalidate().map_err(io::Error::other)?;
        self.carrier.revalidate()
    }

    /// End an activity whose operation launched nothing still running and holds
    /// no stage. Its empty carrier retires if no other owner shares it; any
    /// reservation left behind is retired by the next admission.
    pub(crate) fn release_unused(self) {
        if let Ok(carrier) = Arc::try_unwrap(self.carrier) {
            drop(carrier.retire_after_settlement());
        }
    }
}

impl ActivityPreparationFailure {
    fn release_unused(self) {
        if let Some(partial) = self.inner.partial {
            drop(partial.retire_after_settlement());
        }
    }
}

pub(crate) enum ArchiveOperationClock<'a> {
    Borrowed(&'a dyn Clock),
    Owned(Arc<dyn Clock + Send + Sync>),
}
impl ArchiveOperationClock<'_> {
    fn clock(&self) -> &dyn Clock {
        match self {
            Self::Borrowed(clock) => *clock,
            Self::Owned(clock) => clock.as_ref(),
        }
    }
}

/// Closed original fields; this value admits no further work.
#[must_use]
pub struct OwnedPreparedArchiveHold {
    pub(crate) activity: ArchiveActivity,
    pub(crate) _configuration: ArchiveConfiguration,
    pub(crate) _budget: ArchiveBudget,
    pub(crate) clock: Arc<dyn Clock + Send + Sync>,
}
impl OwnedPreparedArchiveHold {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.activity.carrier()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.activity.journal_root()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.clock
    }
    /// End a prepared operation that launched no child and staged nothing.
    pub fn release_unused(self) {
        self.activity.release_unused();
    }

    /// Close an unused operation while retaining any uncertain native retirement.
    pub fn retire_unused(self) -> Result<(), OwnedUnusedArchiveRetirement> {
        let root = Arc::clone(self.journal_root());
        OwnedUnusedArchiveRetirement::retire(root, self.clock, Some(self.activity), None)
    }
    #[cfg(any(unix, windows))]
    pub(crate) fn revalidate(&self) -> Result<(), AdmissionError> {
        revalidate_original(
            &self.activity,
            &self._configuration,
            &self._budget,
            self.clock.as_ref(),
        )
    }
}

#[must_use]
pub struct OwnedArchivePreparationFailure {
    reason: AdmissionError,
    root: Arc<JournalRoot>,
    activity: Option<ArchiveActivity>,
    preparation: Option<ActivityPreparationFailure>,
    configuration: Option<Box<ArchiveConfiguration>>,
    _budget: Box<ArchiveBudget>,
    clock: Arc<dyn Clock + Send + Sync>,
}
impl OwnedArchivePreparationFailure {
    pub fn reason(&self) -> AdmissionError {
        self.reason
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn carrier(&self) -> Option<&Arc<RootActivityCarrier>> {
        self.activity.as_ref().map(ArchiveActivity::carrier)
    }
    pub fn preparation(&self) -> Option<&ActivityPreparationFailure> {
        self.preparation.as_ref()
    }
    pub fn configuration(&self) -> Option<&ArchiveConfiguration> {
        self.configuration.as_deref()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.clock
    }
    /// A refused preparation launched no child and staged nothing beyond its
    /// own reservation, so it always ends without leaving an effect.
    pub fn release(self) {
        if let Some(activity) = self.activity {
            activity.release_unused();
        }
        if let Some(preparation) = self.preparation {
            preparation.release_unused();
        }
    }

    /// Close a refused preparation, retaining its actual partial reservation
    /// when removal, synchronization or confirmation was not proved.
    pub fn retire_unused(self) -> Result<(), OwnedUnusedArchiveRetirement> {
        OwnedUnusedArchiveRetirement::retire(self.root, self.clock, self.activity, self.preparation)
    }
}

/// Closed native custody from an unused preparation. It admits no work and
/// preserves the actual retirement receipt; a failed receipt is never retried
/// as though an already removed carrier were a fresh operation.
#[must_use]
pub struct OwnedUnusedArchiveRetirement {
    root: Arc<JournalRoot>,
    clock: Arc<dyn Clock + Send + Sync>,
    owners: Vec<UnusedCarrier>,
}

enum UnusedCarrier {
    Shared(Arc<RootActivityCarrier>),
    Retired(ActivityCarrierRetirement<RootActivityCarrier>),
    Partial(ActivityCarrierRetirement<ActivityCarrierPreparationFailure>),
}

/// A release either keeps its original owner or the actual closed retirement.
#[must_use]
pub enum UnusedArchiveRelease<T> {
    Held(T),
    Retired(OwnedUnusedArchiveRetirement),
}

impl OwnedUnusedArchiveRetirement {
    fn retire(
        root: Arc<JournalRoot>,
        clock: Arc<dyn Clock + Send + Sync>,
        activity: Option<ArchiveActivity>,
        preparation: Option<ActivityPreparationFailure>,
    ) -> Result<(), Self> {
        let mut owners = Vec::new();
        if let Some(activity) = activity {
            owners.push(match Arc::try_unwrap(activity.carrier) {
                Ok(carrier) => UnusedCarrier::Retired(carrier.retire_after_settlement()),
                Err(carrier) => UnusedCarrier::Shared(carrier),
            });
        }
        if let Some(preparation) = preparation
            && let Some(partial) = preparation.inner.partial
        {
            owners.push(UnusedCarrier::Partial(partial.retire_after_settlement()));
        }
        let held = Self {
            root,
            clock,
            owners,
        };
        if held.released() { Ok(()) } else { Err(held) }
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.clock
    }
    pub fn retirement_receipts(&self) -> impl Iterator<Item = &ActivityCarrierRetirementReceipt> {
        self.owners.iter().filter_map(|owner| match owner {
            UnusedCarrier::Shared(_) => None,
            UnusedCarrier::Retired(retirement) => Some(&retirement.receipt),
            UnusedCarrier::Partial(retirement) => Some(&retirement.receipt),
        })
    }
    pub fn released(&self) -> bool {
        self.owners.iter().all(|owner| {
            let receipt = match owner {
                UnusedCarrier::Shared(carrier) => {
                    // Retain the native allocation, including when another
                    // owner still prevents its original retirement.
                    let _ = carrier;
                    return false;
                }
                UnusedCarrier::Retired(retirement) => &retirement.receipt,
                UnusedCarrier::Partial(retirement) => &retirement.receipt,
            };
            receipt.effect() == ActivityCarrierRetirementEffect::Removed
                && receipt.root_durability()
                    == if cfg!(windows) {
                        Durability::Unproven
                    } else {
                        Durability::Synced
                    }
                && receipt.confirmation() == Confirmation::Confirmed
                && receipt.failures().is_empty()
        })
    }
}

/// One unused archive admission. Its lifetime cannot be extracted or rebound.
#[must_use = "Move the original operation through archive and read admission"]
pub struct PreparedArchiveOperation<'a> {
    pub(crate) activity: ArchiveActivity,
    pub(crate) configuration: ArchiveConfiguration,
    pub(crate) budget: ArchiveBudget,
    pub(crate) clock: ArchiveOperationClock<'a>,
}

#[must_use = "An archive preparation refusal retains its original admission"]
pub struct ArchiveOperationPreparationFailure<'a> {
    inner: Box<ArchiveOperationPreparationFailureInner<'a>>,
}

struct ArchiveOperationPreparationFailureInner<'a> {
    reason: AdmissionError,
    root: Arc<JournalRoot>,
    activity: Option<ArchiveActivity>,
    preparation: Option<ActivityPreparationFailure>,
    _budget: ArchiveBudget,
    _clock: ArchiveOperationClock<'a>,
}
impl fmt::Debug for ArchiveOperationPreparationFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArchiveOperationPreparationFailure")
            .field("reason", &self.inner.reason)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for ArchiveOperationPreparationFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.reason.fmt(f)
    }
}
impl std::error::Error for ArchiveOperationPreparationFailure<'_> {}
impl ArchiveOperationPreparationFailure<'_> {
    pub fn reason(&self) -> AdmissionError {
        self.inner.reason
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.inner.root
    }
    pub fn carrier(&self) -> Option<&Arc<RootActivityCarrier>> {
        self.inner.activity.as_ref().map(ArchiveActivity::carrier)
    }
    pub fn preparation(&self) -> Option<&ActivityPreparationFailure> {
        self.inner.preparation.as_ref()
    }
    pub fn owned_clock(&self) -> Option<&Arc<dyn Clock + Send + Sync>> {
        match &self.inner._clock {
            ArchiveOperationClock::Owned(clock) => Some(clock),
            ArchiveOperationClock::Borrowed(_) => None,
        }
    }
    pub fn into_owned_hold(self) -> Result<OwnedArchivePreparationFailure, Self> {
        if matches!(self.inner._clock, ArchiveOperationClock::Borrowed(_)) {
            return Err(self);
        }
        let ArchiveOperationPreparationFailureInner {
            reason,
            root,
            activity,
            preparation,
            _budget,
            _clock,
        } = *self.inner;
        let ArchiveOperationClock::Owned(clock) = _clock else {
            unreachable!()
        };
        Ok(OwnedArchivePreparationFailure {
            reason,
            root,
            activity,
            preparation,
            configuration: None,
            _budget: Box::new(_budget),
            clock,
        })
    }
}

impl<'a> PreparedArchiveOperation<'a> {
    pub fn prepare(
        root: Arc<JournalRoot>,
        budget: ArchiveBudget,
        clock: &'a dyn Clock,
    ) -> Result<Self, ArchiveOperationPreparationFailure<'a>> {
        Self::prepare_with(root, budget, ArchiveOperationClock::Borrowed(clock))
    }
    fn prepare_with(
        root: Arc<JournalRoot>,
        budget: ArchiveBudget,
        clock: ArchiveOperationClock<'a>,
    ) -> Result<Self, ArchiveOperationPreparationFailure<'a>> {
        Self::prepare_with_lease(root, budget, clock, None)
    }

    fn prepare_with_lease(
        root: Arc<JournalRoot>,
        budget: ArchiveBudget,
        clock: ArchiveOperationClock<'a>,
        lease: Option<Arc<ActivityLease>>,
    ) -> Result<Self, ArchiveOperationPreparationFailure<'a>> {
        let mut failure = ArchiveOperationPreparationFailureInner {
            reason: AdmissionError::JournalIdentityChanged,
            root: Arc::clone(&root),
            activity: None,
            preparation: None,
            _budget: budget,
            _clock: clock,
        };
        if let Err(reason) = failure._budget.remaining(failure._clock.clock()) {
            failure.reason = reason;
            if let Some(lease) = lease {
                failure.preparation = Some(ActivityPreparationFailure {
                    inner: Box::new(ActivityPreparationFailureInner {
                        error: io::Error::other(reason),
                        root: Arc::clone(&failure.root),
                        lease: Some(lease),
                        partial: None,
                    }),
                });
            }
            return Err(ArchiveOperationPreparationFailure {
                inner: Box::new(failure),
            });
        }
        let prepared_activity = match lease {
            Some(lease) => ArchiveActivity::prepare_acquired(root, lease),
            None => ArchiveActivity::prepare(root),
        };
        let activity = match prepared_activity {
            Ok(activity) => activity,
            Err(preparation) => {
                failure.preparation = Some(preparation);
                return Err(ArchiveOperationPreparationFailure {
                    inner: Box::new(failure),
                });
            }
        };
        let configuration = match ArchiveConfiguration::capture(activity.journal_root()) {
            Ok(configuration) => configuration,
            Err(reason) => {
                failure.reason = reason;
                failure.activity = Some(activity);
                return Err(ArchiveOperationPreparationFailure {
                    inner: Box::new(failure),
                });
            }
        };
        Ok(Self {
            activity,
            configuration,
            budget: failure._budget,
            clock: failure._clock,
        })
    }
    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.activity.activity()
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.activity.carrier()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.activity.journal_root()
    }
    pub fn configuration(&self) -> &ArchiveConfiguration {
        &self.configuration
    }
    pub fn budget(&self) -> &ArchiveBudget {
        &self.budget
    }
    pub fn clock(&self) -> &dyn Clock {
        self.clock.clock()
    }
    pub fn owned_clock(&self) -> Option<&Arc<dyn Clock + Send + Sync>> {
        match &self.clock {
            ArchiveOperationClock::Owned(clock) => Some(clock),
            ArchiveOperationClock::Borrowed(_) => None,
        }
    }
    pub fn into_owned_hold(self) -> Result<OwnedPreparedArchiveHold, Box<Self>> {
        if matches!(self.clock, ArchiveOperationClock::Borrowed(_)) {
            return Err(Box::new(self));
        }
        let Self {
            activity,
            configuration,
            budget,
            clock,
        } = self;
        let ArchiveOperationClock::Owned(clock) = clock else {
            unreachable!()
        };
        Ok(OwnedPreparedArchiveHold {
            activity,
            _configuration: configuration,
            _budget: budget,
            clock,
        })
    }
    pub fn cap_credential_expiry(&mut self, expiry: i64) {
        self.budget.cap_credential_expiry(expiry);
    }
    pub fn revalidate(&self) -> Result<(), AdmissionError> {
        revalidate_original(
            &self.activity,
            &self.configuration,
            &self.budget,
            self.clock(),
        )
    }
}

fn revalidate_original(
    activity: &ArchiveActivity,
    configuration: &ArchiveConfiguration,
    budget: &ArchiveBudget,
    clock: &dyn Clock,
) -> Result<(), AdmissionError> {
    budget.remaining(clock)?;
    activity
        .revalidate()
        .map_err(|_| AdmissionError::JournalIdentityChanged)?;
    configuration.revalidate(activity.journal_root())
}

impl PreparedArchiveOperation<'static> {
    /// Fresh admission using the actual clock captured before the original budget.
    pub fn prepare_owned(
        root: Arc<JournalRoot>,
        budget: ArchiveBudget,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, ArchiveOperationPreparationFailure<'static>> {
        Self::prepare_with(root, budget, ArchiveOperationClock::Owned(clock))
    }
}

/// An original operation whose clock ownership was captured at preparation.
#[must_use = "Move this original operation into admission or closed custody"]
pub struct OwnedPreparedArchiveOperation {
    operation: PreparedArchiveOperation<'static>,
    clock: Arc<dyn Clock + Send + Sync>,
}

impl OwnedPreparedArchiveOperation {
    pub fn prepare(
        root: Arc<JournalRoot>,
        budget: ArchiveBudget,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, OwnedArchivePreparationFailure> {
        Self::from_preparation(
            PreparedArchiveOperation::prepare_owned(root, budget, Arc::clone(&clock)),
            clock,
        )
    }

    fn from_preparation(
        prepared: Result<
            PreparedArchiveOperation<'static>,
            ArchiveOperationPreparationFailure<'static>,
        >,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<Self, OwnedArchivePreparationFailure> {
        match prepared {
            Ok(operation) => Ok(Self { operation, clock }),
            Err(failure) => {
                let ArchiveOperationPreparationFailureInner {
                    reason,
                    root,
                    activity,
                    preparation,
                    _budget,
                    _clock,
                } = *failure.inner;
                drop(_clock);
                Err(OwnedArchivePreparationFailure {
                    reason,
                    root,
                    activity,
                    preparation,
                    configuration: None,
                    _budget: Box::new(_budget),
                    clock,
                })
            }
        }
    }

    pub fn as_prepared(&self) -> &PreparedArchiveOperation<'static> {
        &self.operation
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        self.operation.carrier()
    }
    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.operation.activity()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.operation.journal_root()
    }
    pub fn configuration(&self) -> &ArchiveConfiguration {
        self.operation.configuration()
    }
    pub fn budget(&self) -> &ArchiveBudget {
        self.operation.budget()
    }
    pub fn clock(&self) -> &dyn Clock {
        self.clock.as_ref()
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.clock
    }
    pub fn revalidate(&self) -> Result<(), AdmissionError> {
        self.operation.revalidate()
    }
    pub fn cap_credential_expiry(&mut self, expiry: i64) {
        self.operation.cap_credential_expiry(expiry);
    }
    pub fn into_owned_hold(self) -> OwnedPreparedArchiveHold {
        let Self { operation, clock } = self;
        close_owned_prepared(operation, clock)
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        PreparedArchiveOperation<'static>,
        Arc<dyn Clock + Send + Sync>,
    ) {
        (self.operation, self.clock)
    }
}

/// Original native exclusion kept outside consuming preparation and finish calls.
/// This closed owner grants no stage construction, admission or retirement.
#[must_use = "Retain original custody through the actual receiving boundary"]
pub struct ArchivePreparationCustody {
    root: Arc<JournalRoot>,
    lease: Arc<ActivityLease>,
    clock: Arc<dyn Clock + Send + Sync>,
}

impl ArchivePreparationCustody {
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.clock
    }
    /// Borrow the original allocation without exposing a cloneable capability.
    pub fn activity(&self) -> &ActivityLease {
        self.lease.as_ref()
    }
    pub fn revalidate(&self) -> io::Result<()> {
        self.lease.revalidate().map_err(io::Error::other)
    }
}

/// The original budget and one already acquired lease, before any stage exists.
#[must_use = "Consume the original input or retain it in closed custody"]
pub struct OwnedArchivePreparationInput {
    root: Arc<JournalRoot>,
    lease: Arc<ActivityLease>,
    budget: ArchiveBudget,
    clock: Arc<dyn Clock + Send + Sync>,
}

/// Original unused input with no execution entrance.
#[must_use]
pub struct OwnedArchivePreparationInputHold {
    input: OwnedArchivePreparationInput,
}

impl OwnedArchivePreparationInputHold {
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.input.root
    }
    pub fn owned_clock(&self) -> &Arc<dyn Clock + Send + Sync> {
        &self.input.clock
    }
    pub fn activity(&self) -> &ActivityLease {
        self.input.lease.as_ref()
    }
}

impl OwnedArchivePreparationInput {
    /// Acquire native exclusion before any consuming stage/config/helper work.
    pub fn capture(
        root: Arc<JournalRoot>,
        budget: ArchiveBudget,
        clock: Arc<dyn Clock + Send + Sync>,
    ) -> Result<(Self, ArchivePreparationCustody), OwnedArchivePreparationFailure> {
        let mut failure = OwnedArchivePreparationFailure {
            reason: AdmissionError::JournalIdentityChanged,
            root: Arc::clone(&root),
            activity: None,
            preparation: None,
            configuration: None,
            _budget: Box::new(budget),
            clock: Arc::clone(&clock),
        };
        if let Err(reason) = failure._budget.remaining(clock.as_ref()) {
            failure.reason = reason;
            return Err(failure);
        }
        let lease = match ArchiveActivity::acquire(Arc::clone(&root)) {
            Ok(lease) => lease,
            Err(preparation) => {
                failure.preparation = Some(preparation);
                return Err(failure);
            }
        };
        let custody = ArchivePreparationCustody {
            root: Arc::clone(&root),
            lease: Arc::clone(&lease),
            clock: Arc::clone(&clock),
        };
        Ok((
            Self {
                root,
                lease,
                budget: *failure._budget,
                clock,
            },
            custody,
        ))
    }

    pub fn prepare(self) -> Result<OwnedPreparedArchiveOperation, OwnedArchivePreparationFailure> {
        let Self {
            root,
            lease,
            budget,
            clock,
        } = self;
        let prepared = PreparedArchiveOperation::prepare_with_lease(
            root,
            budget,
            ArchiveOperationClock::Owned(Arc::clone(&clock)),
            Some(lease),
        );
        let owned = OwnedPreparedArchiveOperation::from_preparation(prepared, clock)?;
        if let Err(reason) = owned.revalidate() {
            let OwnedPreparedArchiveOperation { operation, clock } = owned;
            let PreparedArchiveOperation {
                activity,
                configuration,
                budget,
                clock: old_clock,
            } = operation;
            drop(old_clock);
            return Err(OwnedArchivePreparationFailure {
                reason,
                root: Arc::clone(activity.journal_root()),
                activity: Some(activity),
                preparation: None,
                configuration: Some(Box::new(configuration)),
                _budget: Box::new(budget),
                clock,
            });
        }
        Ok(owned)
    }

    pub fn into_owned_hold(self) -> OwnedArchivePreparationInputHold {
        OwnedArchivePreparationInputHold { input: self }
    }
}

#[cfg(all(test, unix, feature = "full-tests"))]
mod custody_tests;

// Only the owned preparation/read wrappers supply their captured clock here.
pub(crate) fn close_owned_prepared(
    operation: PreparedArchiveOperation<'static>,
    clock: Arc<dyn Clock + Send + Sync>,
) -> OwnedPreparedArchiveHold {
    let PreparedArchiveOperation {
        activity,
        configuration,
        budget,
        clock: original_clock,
    } = operation;
    drop(original_clock);
    OwnedPreparedArchiveHold {
        activity,
        _configuration: configuration,
        _budget: budget,
        clock,
    }
}
