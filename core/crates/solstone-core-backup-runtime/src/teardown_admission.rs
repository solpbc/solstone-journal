// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retained teardown admission. Fresh checks do not authorize deletion.

use std::sync::Arc;

use solstone_core_backup::teardown_configuration::{
    TeardownConfiguration, TeardownConfigurationRefusal,
};
use solstone_core_journal_config_write::{
    BoundConfigOutcome, BoundConfigPublication, BoundConfigRefusal,
};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity::{ActivityAdmissionError, TeardownLease};
use solstone_core_journal_io::media::{
    ActivityCarrierPreparationReceipt, RootCarrierObservation, RootTeardownCarrier,
    TeardownCarrierPreparationFailure,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeardownReaderRefusal {
    CatalogUnavailable,
    RecoveryUnavailable,
    RecoveryHeld,
    /// Some cataloged media is not verifiably present locally, so the backup may
    /// hold the only copy.
    BackupOnlyHeld,
    /// Some media only in the backup has no single local recording to return to,
    /// so restoring it first is not possible here.
    BackupOnlyUnplaced,
}

pub type TeardownReader<'a> = dyn for<'carrier> FnMut(
        &Arc<JournalRoot>,
        &TeardownConfiguration,
        Option<RootCarrierObservation<'carrier>>,
    ) -> Result<(), TeardownReaderRefusal>
    + 'a;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeardownAdmissionRefusal {
    Unsupported,
    ActivityBusy,
    ActivityUnavailable,
    CarrierUnavailable,
    Configuration(TeardownConfigurationRefusal),
    Reader(TeardownReaderRefusal),
}

/// The original activity lease and immutable configuration observation.
/// No serialized or cached successful check can construct this admission.
pub struct TeardownAdmission {
    lease: Arc<TeardownLease>,
    configuration: TeardownConfiguration,
}

#[cfg(any(unix, windows))]
mod binding;
#[cfg(any(unix, windows))]
pub use binding::{
    BindingTeardownAdmission, BoundTeardownAdmission, TeardownBindingFailure,
    TeardownBindingRefusal, TeardownFinishAdmission,
};

/// Admit observation only. Destructive execution requires additional boundaries.
pub fn admit_teardown(
    root: Arc<JournalRoot>,
) -> Result<TeardownAdmission, TeardownAdmissionRefusal> {
    #[cfg(any(unix, windows))]
    {
        let lease = TeardownLease::try_acquire(root).map_err(activity_refusal)?;
        // A teardown that ended mid-run left only an empty reservation; it
        // must not hold this one.
        solstone_core_journal_io::media::retire_orphan_root_carriers_for_teardown(&lease)
            .map_err(|_| TeardownAdmissionRefusal::ActivityUnavailable)?;
        let configuration = TeardownConfiguration::capture(lease.journal_root())
            .map_err(TeardownAdmissionRefusal::Configuration)?;
        let admitted = TeardownAdmission {
            lease,
            configuration,
        };
        admitted.revalidate()?;
        Ok(admitted)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = root;
        Err(TeardownAdmissionRefusal::Unsupported)
    }
}

/// iOS caller supplies a freshly acquired real exclusion and Backup's actual
/// existing-only configuration capture. No generic orphan sweep is run here.
#[cfg(target_os = "ios")]
pub fn admit_ios_configuration(
    lease: Arc<TeardownLease>,
    configuration: TeardownConfiguration,
) -> Result<TeardownAdmission, TeardownAdmissionRefusal> {
    let admitted = TeardownAdmission {
        lease,
        configuration,
    };
    admitted.revalidate()?;
    Ok(admitted)
}

impl TeardownAdmission {
    /// Reopen the exact original reservation before the reader excludes it.
    /// Missing, changed or nonempty reservations remain held.
    #[cfg(target_os = "ios")]
    pub fn reacquire_carrier(
        self,
        expected: &solstone_core_journal_io::media::StageObservation,
        reader: &mut TeardownReader<'_>,
    ) -> Result<CarrierTeardownAdmission, TeardownCarrierFailure> {
        if let Err(refusal) = self.revalidate() {
            return Err(TeardownCarrierFailure::new(
                self,
                None,
                TeardownCarrierRefusal::Admission(refusal),
            ));
        }
        let carrier = match RootTeardownCarrier::reacquire(self.lease.clone(), expected) {
            Ok(carrier) => carrier,
            Err(_) => {
                return Err(TeardownCarrierFailure::new(
                    self,
                    None,
                    TeardownCarrierRefusal::Preparation,
                ));
            }
        };
        let prepared = CarrierTeardownAdmission {
            admission: self,
            carrier: Arc::new(carrier),
        };
        if let Err(refusal) = prepared.check(reader) {
            return Err(TeardownCarrierFailure::new(
                prepared.admission,
                Some(RetainedCarrier::Prepared(prepared.carrier)),
                TeardownCarrierRefusal::Admission(refusal),
            ));
        }
        Ok(prepared)
    }
    /// The same original exclusion, retained by outside attempt custody.
    pub fn teardown_lease(&self) -> &Arc<TeardownLease> {
        &self.lease
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.lease.journal_root()
    }

    pub fn configuration(&self) -> &TeardownConfiguration {
        &self.configuration
    }

    pub fn revalidate(&self) -> Result<(), TeardownAdmissionRefusal> {
        self.lease.revalidate().map_err(activity_refusal)?;
        self.configuration
            .revalidate()
            .map_err(TeardownAdmissionRefusal::Configuration)?;
        self.lease.revalidate().map_err(activity_refusal)
    }

    /// Invoke a fresh reader once under the original admission, with no cached verdict.
    /// Postcheck admission refusal takes precedence over a simultaneous reader refusal.
    pub fn check(&self, reader: &mut TeardownReader<'_>) -> Result<(), TeardownAdmissionRefusal> {
        self.revalidate()?;
        let result = reader(self.journal_root(), self.configuration(), None);
        self.revalidate()?;
        result.map_err(TeardownAdmissionRefusal::Reader)
    }

    /// Prepare only under fresh evidence and keep the original lease on every outcome.
    pub fn prepare_carrier(
        self,
        reader: &mut TeardownReader<'_>,
    ) -> Result<CarrierTeardownAdmission, TeardownCarrierFailure> {
        if let Err(refusal) = self.check(reader) {
            return Err(TeardownCarrierFailure::new(
                self,
                None,
                TeardownCarrierRefusal::Admission(refusal),
            ));
        }
        let carrier = match RootTeardownCarrier::prepare(self.lease.clone()) {
            Ok(carrier) => carrier,
            Err(failure) => {
                return Err(TeardownCarrierFailure::new(
                    self,
                    Some(RetainedCarrier::Failed(failure)),
                    TeardownCarrierRefusal::Preparation,
                ));
            }
        };
        let prepared = CarrierTeardownAdmission {
            admission: self,
            carrier: Arc::new(carrier),
        };
        if let Err(refusal) = prepared.check(reader) {
            return Err(TeardownCarrierFailure::new(
                prepared.admission,
                Some(RetainedCarrier::Prepared(prepared.carrier)),
                TeardownCarrierRefusal::Admission(refusal),
            ));
        }
        Ok(prepared)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeardownCarrierRefusal {
    Admission(TeardownAdmissionRefusal),
    Preparation,
}

enum RetainedCarrier {
    Prepared(Arc<RootTeardownCarrier>),
    Failed(TeardownCarrierPreparationFailure),
}

/// Retains actual preparation facts and reservations without authorizing cleanup or retry.
pub struct TeardownCarrierFailure {
    admission: Box<TeardownAdmission>,
    carrier: Option<Box<RetainedCarrier>>,
    refusal: TeardownCarrierRefusal,
}

impl TeardownCarrierFailure {
    fn new(
        admission: TeardownAdmission,
        carrier: Option<RetainedCarrier>,
        refusal: TeardownCarrierRefusal,
    ) -> Self {
        Self {
            admission: Box::new(admission),
            carrier: carrier.map(Box::new),
            refusal,
        }
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.admission.journal_root()
    }
    pub fn refusal(&self) -> TeardownCarrierRefusal {
        self.refusal
    }
    pub fn preparation_error(&self) -> Option<&std::io::Error> {
        match self.carrier.as_deref()? {
            RetainedCarrier::Failed(failure) => Some(failure.error()),
            RetainedCarrier::Prepared(_) => None,
        }
    }
    pub fn preparation(&self) -> Option<&ActivityCarrierPreparationReceipt> {
        Some(match self.carrier.as_deref()? {
            RetainedCarrier::Prepared(carrier) => carrier.preparation(),
            RetainedCarrier::Failed(failure) => failure.receipt(),
        })
    }
    pub fn reservation_path(&self) -> Option<std::path::PathBuf> {
        match self.carrier.as_deref()? {
            RetainedCarrier::Prepared(carrier) => Some(carrier.diagnostic_path()),
            RetainedCarrier::Failed(failure) => failure.diagnostic_path(),
        }
    }
}

impl std::fmt::Debug for TeardownCarrierFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeardownCarrierFailure")
            .field("refusal", &self.refusal)
            .field("preparation_observed", &self.carrier.is_some())
            .finish()
    }
}

/// Original admission and its actual live reservation. No public constructor.
pub struct CarrierTeardownAdmission {
    admission: TeardownAdmission,
    carrier: Arc<RootTeardownCarrier>,
}

impl CarrierTeardownAdmission {
    /// Shares the one actual prepared reservation; it does not prepare another.
    pub fn carrier(&self) -> &Arc<RootTeardownCarrier> {
        &self.carrier
    }

    pub fn teardown_lease(&self) -> &Arc<TeardownLease> {
        self.admission.teardown_lease()
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.admission.journal_root()
    }
    pub fn configuration(&self) -> &TeardownConfiguration {
        self.admission.configuration()
    }
    pub fn revalidate(&self) -> Result<(), TeardownAdmissionRefusal> {
        self.admission.revalidate()?;
        self.carrier
            .revalidate()
            .map_err(|_| TeardownAdmissionRefusal::CarrierUnavailable)?;
        self.admission.revalidate()
    }
    pub fn check(&self, reader: &mut TeardownReader<'_>) -> Result<(), TeardownAdmissionRefusal> {
        self.revalidate()?;
        let result = reader(
            self.journal_root(),
            self.configuration(),
            Some(self.carrier.observation()),
        );
        self.revalidate()?;
        result.map_err(TeardownAdmissionRefusal::Reader)
    }

    /// Clear through the original owner and retain the exact confirmed transition.
    pub fn clear_configuration(
        self,
        reader: &mut TeardownReader<'_>,
    ) -> Result<ClearedTeardownAdmission, TeardownClearFailure> {
        if let Err(refusal) = self.check(reader) {
            return Err(TeardownClearFailure::new(
                self,
                false,
                TeardownClearRefusal::Admission(refusal),
                None,
            ));
        }
        run_clear_hook(ClearCheckpoint::BeforeClear);
        let attempted = true;
        let outcome = self.admission.configuration.clear_configuration();
        let outcome = match outcome {
            Err(refusal) => {
                return Err(TeardownClearFailure::new(
                    self,
                    attempted,
                    TeardownClearRefusal::Clear(Box::new(refusal)),
                    None,
                ));
            }
            Ok(outcome) => outcome,
        };
        if let Some(refusal) = outcome_refusal(&outcome) {
            let publication = match outcome {
                BoundConfigOutcome::Published(publication) => Some(Arc::new(publication)),
                BoundConfigOutcome::Unchanged => None,
            };
            return Err(TeardownClearFailure::new(
                self,
                attempted,
                refusal,
                publication,
            ));
        }
        let BoundConfigOutcome::Published(publication) = outcome else {
            unreachable!("unchanged refused above")
        };
        let publication = Arc::new(publication);
        run_clear_hook(ClearCheckpoint::AfterClear);
        if let Err(refusal) = self.revalidate_publication(&publication) {
            return Err(TeardownClearFailure::new(
                self,
                attempted,
                TeardownClearRefusal::Admission(refusal),
                Some(publication),
            ));
        }
        Ok(ClearedTeardownAdmission {
            admission: self,
            publication,
        })
    }

    fn revalidate_publication(
        &self,
        publication: &BoundConfigPublication,
    ) -> Result<(), TeardownAdmissionRefusal> {
        self.admission
            .lease
            .revalidate()
            .map_err(activity_refusal)?;
        self.carrier
            .revalidate()
            .map_err(|_| TeardownAdmissionRefusal::CarrierUnavailable)?;
        self.admission
            .configuration
            .revalidate_published_clear(publication)
            .map_err(TeardownAdmissionRefusal::Configuration)?;
        self.carrier
            .revalidate()
            .map_err(|_| TeardownAdmissionRefusal::CarrierUnavailable)?;
        self.admission.lease.revalidate().map_err(activity_refusal)
    }
}

/// Refusal facts, separate from whether the owned clear published anything.
pub enum TeardownClearRefusal {
    Admission(TeardownAdmissionRefusal),
    Clear(Box<BoundConfigRefusal>),
    Unchanged,
    PublicationUncertain,
}

impl std::fmt::Debug for TeardownClearRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission(refusal) => f.debug_tuple("Admission").field(refusal).finish(),
            Self::Clear(_) => f.write_str("Clear"),
            Self::Unchanged => f.write_str("Unchanged"),
            Self::PublicationUncertain => f.write_str("PublicationUncertain"),
        }
    }
}

/// Keeps exclusion and all observed effect facts until explicitly dropped.
pub struct TeardownClearFailure {
    admission: Box<CarrierTeardownAdmission>,
    attempted: bool,
    refusal: TeardownClearRefusal,
    publication: Option<Arc<BoundConfigPublication>>,
}

impl TeardownClearFailure {
    fn new(
        admission: CarrierTeardownAdmission,
        attempted: bool,
        refusal: TeardownClearRefusal,
        publication: Option<Arc<BoundConfigPublication>>,
    ) -> Self {
        Self {
            admission: Box::new(admission),
            attempted,
            refusal,
            publication,
        }
    }

    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.admission.journal_root()
    }
    pub fn attempted(&self) -> bool {
        self.attempted
    }
    pub fn refusal(&self) -> &TeardownClearRefusal {
        &self.refusal
    }
    pub fn publication(&self) -> Option<&BoundConfigPublication> {
        self.publication.as_deref()
    }
    pub fn publication_receipt(&self) -> Option<&Arc<BoundConfigPublication>> {
        self.publication.as_ref()
    }
}

impl std::fmt::Debug for TeardownClearFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeardownClearFailure")
            .field("attempted", &self.attempted)
            .field("refusal", &self.refusal)
            .field("published", &self.publication.is_some())
            .finish()
    }
}

/// Original admission plus its own exact confirmed publication. No public factory.
pub struct ClearedTeardownAdmission {
    admission: CarrierTeardownAdmission,
    publication: Arc<BoundConfigPublication>,
}

impl ClearedTeardownAdmission {
    pub fn carrier(&self) -> &Arc<RootTeardownCarrier> {
        self.admission.carrier()
    }
    pub fn teardown_lease(&self) -> &Arc<TeardownLease> {
        self.admission.teardown_lease()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.admission.journal_root()
    }
    pub fn configuration(&self) -> &TeardownConfiguration {
        self.admission.configuration()
    }
    pub fn publication(&self) -> &BoundConfigPublication {
        &self.publication
    }
    pub fn publication_receipt(&self) -> &Arc<BoundConfigPublication> {
        &self.publication
    }
    pub fn revalidate(&self) -> Result<(), TeardownAdmissionRefusal> {
        self.admission.revalidate_publication(&self.publication)
    }

    /// Fresh callback under the exact owned transition; drift takes precedence.
    pub fn check(&self, reader: &mut TeardownReader<'_>) -> Result<(), TeardownAdmissionRefusal> {
        self.revalidate()?;
        let result = reader(
            self.journal_root(),
            self.configuration(),
            Some(self.admission.carrier.observation()),
        );
        self.revalidate()?;
        result.map_err(TeardownAdmissionRefusal::Reader)
    }
}

/// A flush that ran and failed is refused. Windows has no directory flush, so
/// its publication (`DirectoryFlush::Unavailable`) is accepted as it is.
fn outcome_refusal(outcome: &BoundConfigOutcome) -> Option<TeardownClearRefusal> {
    let BoundConfigOutcome::Published(publication) = outcome else {
        return Some(TeardownClearRefusal::Unchanged);
    };
    if publication.durability_uncertainty.is_some()
        || publication.observation_uncertainty.is_some()
        || publication.confirmation_refusal.is_some()
    {
        Some(TeardownClearRefusal::PublicationUncertain)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ClearCheckpoint {
    BeforeClear,
    AfterClear,
}

#[cfg(all(test, feature = "full-tests"))]
type ClearHook = (ClearCheckpoint, Box<dyn FnOnce()>);
#[cfg(all(test, feature = "full-tests"))]
thread_local! { static CLEAR_HOOK: std::cell::RefCell<Option<ClearHook>> = const { std::cell::RefCell::new(None) }; }

fn run_clear_hook(point: ClearCheckpoint) {
    #[cfg(all(test, feature = "full-tests"))]
    {
        let hook = CLEAR_HOOK.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|(at, _)| *at == point) {
                slot.take()
            } else {
                None
            }
        });
        if let Some((_, hook)) = hook {
            hook();
        }
    }
    let _ = point;
}

fn activity_refusal(error: ActivityAdmissionError) -> TeardownAdmissionRefusal {
    match error {
        ActivityAdmissionError::Busy => TeardownAdmissionRefusal::ActivityBusy,
        ActivityAdmissionError::Unavailable(_) => TeardownAdmissionRefusal::ActivityUnavailable,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod clear_tests;
#[cfg(all(test, feature = "full-tests"))]
mod tests;
