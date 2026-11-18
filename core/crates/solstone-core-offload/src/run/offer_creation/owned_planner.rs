// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Plan against one original installed creator and retain its concrete custody.

use std::sync::Arc;

use solstone_core_backup::archive_activity::OwnedPreparedArchiveOperation;
use solstone_core_backup::archive_readback::{
    OwnedDirectReadHold, OwnedHeldDirectRead, OwnedReadOperation, OwnedSettledDirectRead,
};
use solstone_core_backup_runtime::{
    OwnedAdmittedArchive, OwnedAdmittedArchiveHold, OwnedArchiveAdmissionFailure,
};
use solstone_core_journal_io::activity_obligations::{NativeObligation, RecoveryObservationError};
use solstone_core_journal_io::media::RootActivityCarrier;

use super::*;

/// Local observations captured before admitting the original installed creator.
#[must_use]
pub struct OwnedOffloadPlanInputs {
    carrier: Arc<RootActivityCarrier>,
    planning: OffloadPlanning,
    at: ZonedDateTime,
}
impl OwnedOffloadPlanInputs {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn planning(&self) -> &OffloadPlanning {
        &self.planning
    }
}

/// Retain this with the same closed preparation, without starting admission.
#[must_use]
pub struct OwnedOffloadPlanInputFailure {
    carrier: Arc<RootActivityCarrier>,
    planning: Box<OffloadPlanning>,
    reason: Box<OffloadCreationRefusal>,
}
impl OwnedOffloadPlanInputFailure {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn planning(&self) -> &OffloadPlanning {
        &self.planning
    }
    pub fn reason(&self) -> &OffloadCreationRefusal {
        &self.reason
    }
}

pub fn prepare_owned_offload_plan(
    original: &OwnedPreparedArchiveOperation,
    at: ZonedDateTime,
) -> Result<OwnedOffloadPlanInputs, OwnedOffloadPlanInputFailure> {
    let carrier = Arc::clone(original.carrier());
    let mut planning = OffloadPlanning::new();
    #[cfg(any(unix, windows))]
    let result = bound::preflight(original.as_prepared(), &mut planning, &at);
    #[cfg(not(any(unix, windows)))]
    let result = Err(OffloadCreationRefusal::Inputs(
        OffloadOfferRefusal::Unsupported,
    ));
    match result {
        Ok(()) => Ok(OwnedOffloadPlanInputs {
            carrier,
            planning,
            at,
        }),
        Err(reason) => Err(OwnedOffloadPlanInputFailure {
            carrier,
            planning: Box::new(planning),
            reason: Box::new(reason),
        }),
    }
}

enum ActiveOwner {
    Admitted(Box<OwnedAdmittedArchive>),
    AdmissionFailure(Box<OwnedArchiveAdmissionFailure>),
    Read(Box<OwnedReadOperation>),
    Creation(Box<OwnedOffloadCreation>),
}

pub enum OwnedOffloadPlanOwner<'a> {
    Admitted(&'a OwnedAdmittedArchive),
    AdmissionFailure(&'a OwnedArchiveAdmissionFailure),
    Read(&'a OwnedReadOperation),
    Creation(&'a OwnedOffloadCreation),
}

#[must_use = "Finish this actual plan or retain its whole closed owner"]
pub struct OwnedOffloadPlan {
    carrier: Arc<RootActivityCarrier>,
    input_carrier: Arc<RootActivityCarrier>,
    planning: OffloadPlanning,
    owner: ActiveOwner,
    reason: Option<Box<OffloadCreationRefusal>>,
    capture_failure: Option<Box<OwnedOffloadInputFailure>>,
}
impl OwnedOffloadPlan {
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn planning(&self) -> &OffloadPlanning {
        &self.planning
    }
    pub fn reason(&self) -> Option<&OffloadCreationRefusal> {
        self.reason
            .as_deref()
            .or_else(|| {
                self.capture_failure
                    .as_ref()
                    .map(|failure| failure.reason())
            })
            .or_else(|| match &self.owner {
                ActiveOwner::Creation(creation) => creation.reason(),
                _ => None,
            })
    }
    pub fn capture_failure(&self) -> Option<&OwnedOffloadInputFailure> {
        self.capture_failure.as_deref()
    }
    pub fn owner(&self) -> OwnedOffloadPlanOwner<'_> {
        match &self.owner {
            ActiveOwner::Admitted(owner) => OwnedOffloadPlanOwner::Admitted(owner),
            ActiveOwner::AdmissionFailure(owner) => OwnedOffloadPlanOwner::AdmissionFailure(owner),
            ActiveOwner::Read(owner) => OwnedOffloadPlanOwner::Read(owner),
            ActiveOwner::Creation(owner) => OwnedOffloadPlanOwner::Creation(owner),
        }
    }
    pub fn into_owned_hold(self) -> OwnedOffloadPlanHold {
        let owner = match self.owner {
            ActiveOwner::Admitted(owner) => {
                ClosedOwner::Admitted(Box::new(owner.into_owned_hold()))
            }
            ActiveOwner::AdmissionFailure(owner) => ClosedOwner::AdmissionFailure(owner),
            ActiveOwner::Read(owner) => ClosedOwner::Read(Box::new(owner.into_owned_hold())),
            ActiveOwner::Creation(owner) => {
                ClosedOwner::Creation(Box::new(owner.into_owned_hold()))
            }
        };
        OwnedOffloadPlanHold {
            carrier: self.carrier,
            _input_carrier: self.input_carrier,
            planning: self.planning,
            owner,
            reason: self.reason,
            capture_failure: self.capture_failure,
            refusal: None,
        }
    }
    pub fn finish(self) -> OwnedOffloadPlanFinish {
        if self.reason.is_some()
            || self.capture_failure.is_some()
            || !matches!(self.owner, ActiveOwner::Creation(_) | ActiveOwner::Read(_))
        {
            return OwnedOffloadPlanFinish::Held(Box::new(
                self.into_owned_hold()
                    .refuse(OwnedOffloadPlanFinishRefusal::IncompletePlan),
            ));
        }
        let Self {
            carrier,
            input_carrier,
            mut planning,
            owner,
            reason,
            capture_failure,
        } = self;
        let settled = match owner {
            ActiveOwner::Creation(creation) => match creation.finish() {
                OwnedOffloadCreationFinish::Settled(settled) => {
                    planning.phase = OffloadPlanningPhase::OffersPublished;
                    let (read, report) = settled.into_parts();
                    Ok((read, Some(report)))
                }
                OwnedOffloadCreationFinish::Held(held) => Err((
                    ClosedOwner::Creation(held),
                    OwnedOffloadPlanFinishRefusal::Creation,
                )),
            },
            ActiveOwner::Read(read) => {
                if !matches!(
                    planning.phase,
                    OffloadPlanningPhase::AlreadySatisfied | OffloadPlanningPhase::Exhausted
                ) {
                    Err((
                        ClosedOwner::Read(Box::new(read.into_owned_hold())),
                        OwnedOffloadPlanFinishRefusal::IncompletePlan,
                    ))
                } else {
                    match read.settle_direct() {
                        Err(held) => {
                            let refusal = OwnedOffloadPlanFinishRefusal::Readback(held.reason());
                            Err((ClosedOwner::SettlementFailure(Box::new(held)), refusal))
                        }
                        Ok(read) => {
                            let checked = (|| {
                                read.revalidate()
                                    .map_err(OwnedOffloadPlanFinishRefusal::Readback)?;
                                let coverage = solstone_core_journal_io::activity_obligations::observe_with_carrier(
                                    carrier.journal_root(), carrier.observation(),
                                ).map_err(OwnedOffloadPlanFinishRefusal::Census)?;
                                if coverage.unresolved_entries().next().is_some() {
                                    return Err(OwnedOffloadPlanFinishRefusal::Obligations(
                                        coverage.entries().to_vec(),
                                    ));
                                }
                                read.revalidate()
                                    .map_err(OwnedOffloadPlanFinishRefusal::Readback)
                            })();
                            match checked {
                                Ok(()) => Ok((read, None)),
                                Err(refusal) => Err((
                                    ClosedOwner::SettledRead {
                                        read: Box::new(read.into_owned_hold()),
                                        creation: None,
                                    },
                                    refusal,
                                )),
                            }
                        }
                    }
                }
            }
            _ => unreachable!("checked finish phase"),
        };
        let (read, creation) = match settled {
            Ok(settled) => settled,
            Err((owner, refusal)) => {
                return OwnedOffloadPlanFinish::Held(Box::new(OwnedOffloadPlanHold {
                    carrier,
                    _input_carrier: input_carrier,
                    planning,
                    owner,
                    reason,
                    capture_failure,
                    refusal: Some(Box::new(refusal)),
                }));
            }
        };
        let planning = match planning.into_report() {
            Ok(report) => report,
            Err(planning) => {
                return OwnedOffloadPlanFinish::Held(Box::new(OwnedOffloadPlanHold {
                    carrier,
                    _input_carrier: input_carrier,
                    planning: *planning,
                    owner: ClosedOwner::SettledRead {
                        read: Box::new(read.into_owned_hold()),
                        creation: creation.map(Box::new),
                    },
                    reason,
                    capture_failure,
                    refusal: Some(Box::new(OwnedOffloadPlanFinishRefusal::IncompletePlan)),
                }));
            }
        };
        drop(carrier);
        drop(input_carrier);
        OwnedOffloadPlanFinish::Settled(Box::new(SettledOffloadPlan {
            read,
            report: SettledOffloadPlanReport { planning, creation },
        }))
    }
}

enum ClosedOwner {
    Admitted(Box<OwnedAdmittedArchiveHold>),
    AdmissionFailure(Box<OwnedArchiveAdmissionFailure>),
    Read(Box<OwnedDirectReadHold>),
    Creation(Box<OwnedOffloadCreationHold>),
    SettlementFailure(Box<OwnedHeldDirectRead>),
    SettledRead {
        read: Box<OwnedDirectReadHold>,
        creation: Option<Box<SettledOffloadCreationReport>>,
    },
}

#[derive(Debug)]
pub enum OwnedOffloadPlanFinishRefusal {
    IncompletePlan,
    Creation,
    Readback(ReadbackError),
    Census(RecoveryObservationError),
    Obligations(Vec<NativeObligation>),
}

#[must_use]
pub struct OwnedOffloadPlanHold {
    carrier: Arc<RootActivityCarrier>,
    _input_carrier: Arc<RootActivityCarrier>,
    planning: OffloadPlanning,
    owner: ClosedOwner,
    reason: Option<Box<OffloadCreationRefusal>>,
    capture_failure: Option<Box<OwnedOffloadInputFailure>>,
    refusal: Option<Box<OwnedOffloadPlanFinishRefusal>>,
}
impl OwnedOffloadPlanHold {
    fn refuse(mut self, refusal: OwnedOffloadPlanFinishRefusal) -> Self {
        self.refusal = Some(Box::new(refusal));
        self
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        &self.carrier
    }
    pub fn planning(&self) -> &OffloadPlanning {
        &self.planning
    }
    pub fn reason(&self) -> Option<&OffloadCreationRefusal> {
        self.reason
            .as_deref()
            .or_else(|| {
                self.capture_failure
                    .as_ref()
                    .map(|failure| failure.reason())
            })
            .or_else(|| self.creation().and_then(OwnedOffloadCreationHold::reason))
    }
    pub fn finish_refusal(&self) -> Option<&OwnedOffloadPlanFinishRefusal> {
        self.refusal.as_deref()
    }
    pub fn capture_failure(&self) -> Option<&OwnedOffloadInputFailure> {
        self.capture_failure.as_deref()
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
            ClosedOwner::Read(read) | ClosedOwner::SettledRead { read, .. } => Some(read),
            _ => None,
        }
    }
    pub fn settlement_failure(&self) -> Option<&OwnedHeldDirectRead> {
        match &self.owner {
            ClosedOwner::SettlementFailure(owner) => Some(owner),
            _ => None,
        }
    }
    pub fn creation(&self) -> Option<&OwnedOffloadCreationHold> {
        match &self.owner {
            ClosedOwner::Creation(owner) => Some(owner),
            _ => None,
        }
    }
    pub fn settled_creation(&self) -> Option<&SettledOffloadCreationReport> {
        match &self.owner {
            ClosedOwner::SettledRead { creation, .. } => creation.as_deref(),
            _ => None,
        }
    }
    /// The actual read settled before a later final-check refusal.
    pub fn metadata_settled(&self) -> bool {
        matches!(&self.owner, ClosedOwner::SettledRead { creation: None, .. })
    }
}

pub struct SettledOffloadPlanReport {
    planning: OffloadPlanningReport,
    creation: Option<SettledOffloadCreationReport>,
}
impl SettledOffloadPlanReport {
    pub fn planning(&self) -> &OffloadPlanningReport {
        &self.planning
    }
    pub fn creation(&self) -> Option<&SettledOffloadCreationReport> {
        self.creation.as_ref()
    }
}
#[must_use]
pub struct SettledOffloadPlan {
    read: OwnedSettledDirectRead,
    report: SettledOffloadPlanReport,
}
impl SettledOffloadPlan {
    pub fn into_parts(self) -> (OwnedSettledDirectRead, SettledOffloadPlanReport) {
        (self.read, self.report)
    }
}
#[must_use]
pub enum OwnedOffloadPlanFinish {
    Settled(Box<SettledOffloadPlan>),
    Held(Box<OwnedOffloadPlanHold>),
}

pub fn create_owned_planned_offload_offers(
    admitted: OwnedAdmittedArchive,
    inputs: OwnedOffloadPlanInputs,
) -> OwnedOffloadPlan {
    let OwnedOffloadPlanInputs {
        carrier: input_carrier,
        planning,
        at,
    } = inputs;
    let mut plan = OwnedOffloadPlan {
        carrier: Arc::clone(admitted.carrier()),
        input_carrier,
        planning,
        owner: ActiveOwner::Admitted(Box::new(admitted)),
        reason: None,
        capture_failure: None,
    };
    let ActiveOwner::Admitted(admitted) = &plan.owner else {
        unreachable!()
    };
    if !Arc::ptr_eq(&plan.carrier, &plan.input_carrier) {
        plan.reason = Some(Box::new(OffloadCreationRefusal::Inputs(
            OffloadOfferRefusal::Changed,
        )));
        return plan;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _parameters = (admitted, at);
        plan.reason = Some(Box::new(OffloadCreationRefusal::Inputs(
            OffloadOfferRefusal::Unsupported,
        )));
        plan
    }
    #[cfg(any(unix, windows))]
    {
        let inspected = bound::inspect_plan(
            crate::run::current::CurrentArchive::Owned(admitted),
            &mut plan.planning,
            &at,
        );
        let selections = match inspected {
            Err(reason) => {
                plan.reason = Some(Box::new(reason));
                return plan;
            }
            Ok(None) => {
                plan.planning.phase = OffloadPlanningPhase::AlreadySatisfied;
                None
            }
            Ok(Some(selections)) if selections.is_empty() => {
                plan.planning.phase = OffloadPlanningPhase::Exhausted;
                None
            }
            Ok(Some(selections)) => Some(selections),
        };
        let Some(selections) = selections else {
            let ActiveOwner::Admitted(admitted) = plan.owner else {
                unreachable!()
            };
            plan.owner = match admitted.into_read_operation() {
                Ok(read) => ActiveOwner::Read(Box::new(read)),
                Err(failure) => {
                    plan.reason = Some(Box::new(OffloadCreationRefusal::Archive(failure.error())));
                    ActiveOwner::AdmissionFailure(Box::new(failure))
                }
            };
            return plan;
        };
        let captured = super::super::owned::capture_inputs(
            admitted.prepared_operation(),
            Arc::clone(&plan.carrier),
            &selections,
            at.date(),
            at.timestamp(),
        );
        let captured = match captured {
            Ok(captured) => captured,
            Err(failure) => {
                plan.capture_failure = Some(Box::new(failure));
                return plan;
            }
        };
        if let Err(reason) =
            bound::payload_capacity(admitted.prepared_operation(), captured.observed())
        {
            plan.capture_failure = Some(Box::new(captured.refuse(reason)));
            return plan;
        }
        let ActiveOwner::Admitted(admitted) = plan.owner else {
            unreachable!()
        };
        plan.owner = ActiveOwner::Creation(Box::new(create_owned_offload_offers(
            *admitted,
            captured,
            at.date(),
            at.timestamp(),
        )));
        plan
    }
}
