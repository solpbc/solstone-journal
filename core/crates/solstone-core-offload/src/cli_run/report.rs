// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Diagnostic observations only. These copies supply no settlement authority.

use std::fmt::Write;
use std::io;

use solstone_core_backup_runtime::{ArchiveError, ArchiveRemoteEffects, ArchiveSnapshot};
use solstone_core_journal_io::media::{Cleanup, MediaDigest, MediaIdentity};
use solstone_core_retention::archive_offer::{
    OffloadOfferRefusal, OffloadReconciliationDisposition, OffloadReconciliationGroup,
};
use solstone_core_retention::mark_mutation::{BoundMutationOutcome, MutationStatus};

use super::*;
use crate::measurement::ProposedRelief;
use crate::run::{
    OffloadCreationRefusal, OffloadLedgerEffect, OffloadLedgerSync, OffloadLedgerWrite,
    OffloadPlanning, OffloadPlanningPhase, OwnedOffloadCreation, OwnedOffloadCreationHold,
    OwnedOffloadCreationOwner, OwnedOffloadPlan, OwnedOffloadPlanOwner,
    SettledOffloadCreationReport,
};

#[derive(Clone, Debug)]
pub struct CliMutationFact {
    pub status: MutationStatus,
    pub published: bool,
    pub durability_uncertain: bool,
    pub observation_uncertain: bool,
    pub binding_refusals: usize,
    pub lock_uncertain: bool,
}
#[derive(Clone, Debug)]
pub struct CliReconciliationFact {
    pub disposition: &'static str,
    pub mutation: Option<CliMutationFact>,
    pub followup_refusal: bool,
}
#[derive(Clone, Debug)]
pub enum CliLedgerFact {
    Recorded {
        effect: OffloadLedgerEffect,
        durability: [OffloadLedgerSync; 4],
        confirmation: OffloadLedgerSync,
        failure_count: usize,
    },
    Refused {
        reason: &'static str,
        detail_available: bool,
    },
}
#[derive(Clone, Debug)]
pub enum CliArchiveFact {
    Returned(ArchiveSnapshot),
    Refused {
        error: ArchiveError,
        effects: ArchiveRemoteEffects,
    },
}
#[derive(Clone, Debug)]
pub struct CliCopyFact {
    pub digest: MediaDigest,
    pub identity: MediaIdentity,
    pub cleanup: Option<Cleanup>,
}
#[derive(Clone, Debug, Default)]
pub struct CliCreationFacts {
    pub publication: Option<CliMutationFact>,
    pub publication_refused: bool,
    pub archives: Vec<CliArchiveFact>,
    pub ledger: Vec<CliLedgerFact>,
    pub source_copies: Vec<CliCopyFact>,
    pub source_cleanups: Vec<crate::run::OffloadSourceCleanup>,
    pub partial_copy: bool,
    pub native_error: Option<io::ErrorKind>,
}

/// Last actually borrowed facts, also retained when a consuming callee unwinds.
#[derive(Clone, Debug, Default)]
pub struct CliObservedEffects {
    pub planning_phase: Option<OffloadPlanningPhase>,
    pub budget_bytes: Option<u64>,
    pub initial_projection: Option<ProposedRelief>,
    pub reconciliation: Vec<CliReconciliationFact>,
    pub creation: Option<CliCreationFacts>,
}

pub(super) fn mutation(outcome: &BoundMutationOutcome) -> CliMutationFact {
    let (result, published, uncertainty) = match outcome {
        BoundMutationOutcome::Unchanged(result) => (result, false, None),
        BoundMutationOutcome::Published(result) => (result, true, None),
        BoundMutationOutcome::PublishedUncertain {
            candidate,
            uncertainty,
        } => (candidate, true, Some(uncertainty)),
    };
    CliMutationFact {
        status: result.status,
        published,
        durability_uncertain: uncertainty.is_some_and(|u| u.durability.is_some()),
        observation_uncertain: uncertainty.is_some_and(|u| u.observation.is_some()),
        binding_refusals: uncertainty.map_or(0, |u| u.bindings.len()),
        lock_uncertain: uncertainty.is_some_and(|u| u.lock.is_some()),
    }
}

fn reconciliation(groups: &[OffloadReconciliationGroup]) -> Vec<CliReconciliationFact> {
    groups
        .iter()
        .map(|group| {
            let (disposition, mutation, followup_refusal) = match group.disposition() {
                OffloadReconciliationDisposition::Unchanged => ("unchanged", None, false),
                OffloadReconciliationDisposition::Busy => ("busy", None, false),
                OffloadReconciliationDisposition::Refused(_) => ("refused", None, false),
                OffloadReconciliationDisposition::WriteRefused(_) => ("write_refused", None, false),
                OffloadReconciliationDisposition::Publication {
                    outcome,
                    observation,
                } => (
                    "publication",
                    Some(mutation(outcome)),
                    observation.is_some(),
                ),
            };
            CliReconciliationFact {
                disposition,
                mutation,
                followup_refusal,
            }
        })
        .collect()
}

fn ledger<'a>(writes: impl Iterator<Item = OffloadLedgerWrite<'a>>) -> Vec<CliLedgerFact> {
    writes
        .map(|write| match write {
            OffloadLedgerWrite::Recorded {
                effect,
                durability,
                confirmation,
                failures,
            } => CliLedgerFact::Recorded {
                effect,
                durability,
                confirmation,
                failure_count: failures.len(),
            },
            OffloadLedgerWrite::Refused { reason, detail } => CliLedgerFact::Refused {
                reason,
                detail_available: detail.is_some(),
            },
        })
        .collect()
}

fn archives(
    values: &[Result<ArchiveSnapshot, solstone_core_backup_runtime::ArchiveFailure>],
) -> Vec<CliArchiveFact> {
    values
        .iter()
        .map(|value| match value {
            Ok(snapshot) => CliArchiveFact::Returned(snapshot.clone()),
            Err(failure) => CliArchiveFact::Refused {
                error: failure.error,
                effects: failure.remote_effects.clone(),
            },
        })
        .collect()
}

fn active_creation(creation: &OwnedOffloadCreation) -> CliCreationFacts {
    let (publication, publication_refused) = match creation.owner() {
        OwnedOffloadCreationOwner::Published(outcome) => match outcome.publication() {
            Ok(outcome) => (Some(mutation(outcome)), false),
            Err(_) => (None, true),
        },
        _ => (None, false),
    };
    CliCreationFacts {
        publication,
        publication_refused,
        archives: archives(creation.archives()),
        ledger: ledger(creation.ledger_writes()),
        source_copies: creation
            .source_copies()
            .map(|copy| CliCopyFact {
                digest: copy.digest().clone(),
                identity: copy.identity(),
                cleanup: None,
            })
            .collect(),
        source_cleanups: Vec::new(),
        partial_copy: creation.partial_copy().is_some(),
        native_error: creation.native_error().map(io::Error::kind),
    }
}

fn held_creation(creation: &OwnedOffloadCreationHold) -> CliCreationFacts {
    let publication = creation
        .offers()
        .and_then(|offers| offers.publication())
        .or_else(|| creation.settled_offers().map(|offers| offers.publication()));
    CliCreationFacts {
        publication: publication.and_then(|p| p.as_ref().ok()).map(mutation),
        publication_refused: publication.is_some_and(Result::is_err),
        archives: archives(creation.archives()),
        ledger: ledger(creation.ledger_writes()),
        source_copies: creation
            .source_copies()
            .map(|copy| CliCopyFact {
                digest: copy.digest().clone(),
                identity: copy.identity(),
                cleanup: None,
            })
            .collect(),
        source_cleanups: creation.source_cleanups().to_vec(),
        partial_copy: creation.partial_copy().is_some(),
        native_error: creation.native_error().map(io::Error::kind),
    }
}

fn settled_creation(creation: &SettledOffloadCreationReport) -> CliCreationFacts {
    CliCreationFacts {
        publication: creation.offers().publication().as_ref().ok().map(mutation),
        publication_refused: creation.offers().publication().is_err(),
        archives: archives(creation.archives()),
        ledger: ledger(creation.ledger_writes()),
        source_copies: creation
            .source_copies()
            .iter()
            .map(|copy| CliCopyFact {
                digest: copy.digest.clone(),
                identity: copy.identity,
                cleanup: Some(copy.cleanup),
            })
            .collect(),
        ..CliCreationFacts::default()
    }
}

impl CliObservedEffects {
    pub(super) fn planning(planning: &OffloadPlanning) -> Self {
        let groups = planning
            .reconciliation()
            .map(|r| match r {
                Ok(r) => r.groups(),
                Err(r) => r.groups(),
            })
            .unwrap_or_default();
        Self {
            planning_phase: Some(planning.phase()),
            budget_bytes: planning.budget_bytes(),
            initial_projection: planning.initial_projection().cloned(),
            reconciliation: reconciliation(groups),
            creation: None,
        }
    }
    pub(super) fn active(plan: &OwnedOffloadPlan) -> Self {
        let mut facts = Self::planning(plan.planning());
        if let OwnedOffloadPlanOwner::Creation(creation) = plan.owner() {
            facts.creation = Some(active_creation(creation));
        }
        facts
    }
    pub(super) fn held(plan: &OwnedOffloadPlanHold) -> Self {
        let mut facts = Self::planning(plan.planning());
        facts.creation = plan
            .creation()
            .map(held_creation)
            .or_else(|| plan.settled_creation().map(settled_creation));
        facts
    }
    pub(super) fn settled(report: &SettledOffloadPlanReport) -> Self {
        let planning = report.planning();
        Self {
            planning_phase: Some(planning.phase()),
            budget_bytes: planning.budget_bytes(),
            initial_projection: planning.initial_projection().cloned(),
            reconciliation: reconciliation(planning.reconciliation().groups()),
            creation: report.creation().map(settled_creation),
        }
    }
    fn render(&self, output: &mut String) {
        if let Some(phase) = self.planning_phase {
            let phase = match phase {
                OffloadPlanningPhase::Held => "held",
                OffloadPlanningPhase::AlreadySatisfied => "already_satisfied",
                OffloadPlanningPhase::Exhausted => "exhausted",
                OffloadPlanningPhase::OffersPublished => "offers_published",
            };
            writeln!(output, "planning_phase={phase}").unwrap();
        }
        if let Some(projection) = &self.initial_projection {
            writeln!(
                output,
                "initial_local_bytes={} initial_eligible_bytes={} initial_projected_local_bytes={}",
                projection.local.total_bytes,
                projection.eligible_bytes,
                projection.projected_local_bytes
            )
            .unwrap();
        }
        for (index, group) in self.reconciliation.iter().enumerate() {
            writeln!(
                output,
                "reconciliation[{index}]={} followup_refusal={}",
                group.disposition, group.followup_refusal
            )
            .unwrap();
            if let Some(mutation) = &group.mutation {
                writeln!(output, "reconciliation[{index}].publication={mutation:?}").unwrap();
            }
        }
        if let Some(creation) = &self.creation {
            writeln!(
                output,
                "archives={} source_copies={} partial_copy={} native_error={:?}",
                creation.archives.len(),
                creation.source_copies.len(),
                creation.partial_copy,
                creation.native_error
            )
            .unwrap();
            if let Some(publication) = &creation.publication {
                writeln!(output, "offer_publication={publication:?}").unwrap();
            }
            writeln!(
                output,
                "offer_publication_refused={}",
                creation.publication_refused
            )
            .unwrap();
            for (index, archive) in creation.archives.iter().enumerate() {
                match archive {
                    CliArchiveFact::Returned(_) => {
                        writeln!(output, "archive[{index}]=returned").unwrap()
                    }
                    CliArchiveFact::Refused { effects, .. } => writeln!(
                        output,
                        "archive[{index}]=refused remote_effects={effects:?}"
                    )
                    .unwrap(),
                }
            }
            for (index, write) in creation.ledger.iter().enumerate() {
                writeln!(output, "ledger[{index}]={write:?}").unwrap();
            }
            for (index, copy) in creation.source_copies.iter().enumerate() {
                writeln!(
                    output,
                    "source_copy[{index}].bytes={} cleanup={:?}",
                    copy.digest.size, copy.cleanup
                )
                .unwrap();
            }
            for cleanup in &creation.source_cleanups {
                writeln!(
                    output,
                    "source_cleanup[{}]={:?}",
                    cleanup.index, cleanup.cleanup
                )
                .unwrap();
            }
        }
    }
}

/// The run stopped because the journal changed under it, before it proposed
/// anything: its configuration, media or proposals moved after this run read
/// them.
fn changed_before_proposal(plan: &OwnedOffloadPlanHold) -> bool {
    let offers = plan.creation().and_then(OwnedOffloadCreationHold::offers);
    plan.settled_creation().is_none()
        && offers.and_then(|offers| offers.publication()).is_none()
        && journal_changed(plan.reason(), offers.and_then(|offers| offers.reason()))
}

fn journal_changed(
    reason: Option<&OffloadCreationRefusal>,
    offers: Option<&OffloadOfferRefusal>,
) -> bool {
    match reason {
        Some(OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Changed)) => true,
        Some(OffloadCreationRefusal::Verification) => {
            matches!(offers, Some(OffloadOfferRefusal::Changed))
        }
        _ => false,
    }
}

impl OwnedOffloadCliRun {
    pub(super) fn render(&mut self) {
        let complete = matches!(self.custody, Custody::Finished { .. })
            && self
                .runtime()
                .is_some_and(|r| r.state() == DirectReadFinishState::Completed)
            && self.binding_errors.is_empty();
        let unknown = matches!(self.custody, Custody::Unknown { .. })
            || self
                .runtime()
                .is_some_and(|r| r.state() == DirectReadFinishState::Unknown);
        let skipped = matches!(self.custody, Custody::Skipped { .. });
        self.exit_code = if complete || skipped { 0 } else { 1 };
        let status = if complete {
            "completed"
        } else if skipped {
            "skipped"
        } else if unknown {
            "unknown"
        } else {
            "held"
        };
        let reason = match &self.custody {
            Custody::Prelaunch { refusal, .. } => match refusal {
                CliPrelaunchRefusal::DryRun => "dry_run_unavailable",
                CliPrelaunchRefusal::UnsupportedPlatform => "platform_unavailable",
                CliPrelaunchRefusal::NamespaceCooperationHeld => "namespace_cooperation_held",
                CliPrelaunchRefusal::Root(_) => "journal_unavailable",
                CliPrelaunchRefusal::Budget(_) => "deadline_unavailable",
            },
            Custody::Skipped { .. } => "offload_off",
            Custody::PreparationFailure { .. } => "preparation_refused",
            Custody::PreparedHold { refusal, .. } => match refusal {
                CliPreparedRefusal::Timestamp => "clock_unavailable",
                CliPreparedRefusal::Operation(_) | CliPreparedRefusal::Custody(_) => {
                    "original_changed"
                }
                CliPreparedRefusal::Planning(_) => "planning_refused",
                CliPreparedRefusal::Executable(_) | CliPreparedRefusal::Member(_) => {
                    "installed_tool_unavailable"
                }
            },
            Custody::AdmissionFailure { .. } => "admission_refused",
            Custody::PlanHold { .. } => "plan_held",
            Custody::Finished { .. } if !self.binding_errors.is_empty() => "original_changed",
            Custody::Finished { .. } if !complete => "runtime_held",
            Custody::Finished { .. } => "settled",
            Custody::Unknown { .. } => "consuming_phase_unknown",
        };
        let journal_changed = self.plan_hold().is_some_and(changed_before_proposal);
        let staged =
            self.observed.creation.as_ref().is_some_and(|creation| {
                !creation.source_copies.is_empty() || creation.partial_copy
            });
        self.stdout = format!("offload_status={status} reason={reason}\n");
        if journal_changed {
            self.stdout.push_str("hold_reason=journal_changed\n");
        }
        self.observed.render(&mut self.stdout);
        if let Some(runtime) = self.runtime() {
            let state = runtime.state();
            let retirement = runtime.retirement().is_some();
            writeln!(
                self.stdout,
                "runtime={state:?} retirement_receipt_available={retirement}"
            )
            .unwrap();
        }
        self.stderr = if complete || skipped {
            String::new()
        } else {
            let guidance = match reason {
                "dry_run_unavailable" => {
                    "a read-only offload preview is unavailable. no offload work was started."
                }
                "platform_unavailable" => "offload execution is unavailable on this platform.",
                "installed_tool_unavailable" => {
                    "use the installed solstone command with its packaged backup tool."
                }
                "journal_unavailable" | "original_changed" => {
                    "your journal could not be confirmed. keep its recovery files for review."
                }
                "deadline_unavailable" | "clock_unavailable" => {
                    "the operation clock could not be confirmed."
                }
                // Planning refuses before admission: no media has left or been removed.
                "preparation_refused" | "planning_refused" => "offload did not start.",
                // Copies this run staged stay until they are reviewed, so a
                // plain retry is only offered when it staged none.
                "plan_held" if journal_changed && !staged => {
                    "offload stopped because your journal changed while it was running. nothing was removed from this computer. run it again."
                }
                "plan_held" if journal_changed => {
                    "offload stopped because your journal changed while it was running. nothing was removed from this computer. keep offload's recovery files and review the recorded effects before retrying."
                }
                _ => {
                    "offload did not finish. keep its recovery files and review the recorded effects before retrying."
                }
            };
            format!("{guidance}\n")
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_journal_change_before_proposal_reads_as_changed() {
        let changed = OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Changed);
        assert!(journal_changed(Some(&changed), None));
        assert!(journal_changed(
            Some(&OffloadCreationRefusal::Verification),
            Some(&OffloadOfferRefusal::Changed),
        ));
        assert!(!journal_changed(
            Some(&OffloadCreationRefusal::Verification),
            Some(&OffloadOfferRefusal::Approval),
        ));
        assert!(!journal_changed(
            Some(&OffloadCreationRefusal::Verification),
            None
        ));
        for other in [
            OffloadCreationRefusal::Ledger,
            OffloadCreationRefusal::Publication,
            OffloadCreationRefusal::Inputs(OffloadOfferRefusal::Unproven),
        ] {
            assert!(!journal_changed(
                Some(&other),
                Some(&OffloadOfferRefusal::Changed)
            ));
        }
        assert!(!journal_changed(None, Some(&OffloadOfferRefusal::Changed)));
    }

    #[test]
    fn planning_output_keeps_initial_projection_and_distinct_phases() {
        for (phase, label) in [
            (OffloadPlanningPhase::Held, "held"),
            (OffloadPlanningPhase::AlreadySatisfied, "already_satisfied"),
            (OffloadPlanningPhase::Exhausted, "exhausted"),
            (OffloadPlanningPhase::OffersPublished, "offers_published"),
        ] {
            let observed = CliObservedEffects {
                planning_phase: Some(phase),
                budget_bytes: Some(7),
                initial_projection: Some(ProposedRelief {
                    local: crate::measurement::RawMediaUsage {
                        total_bytes: 20,
                        total_files: 2,
                        per_day: Vec::new(),
                    },
                    eligible_bytes: 5,
                    eligible_files: 1,
                    projected_local_bytes: 15,
                }),
                ..CliObservedEffects::default()
            };
            let mut output = String::new();
            observed.render(&mut output);
            assert!(output.contains(&format!("planning_phase={label}\n")));
            assert!(output.contains(
                "initial_local_bytes=20 initial_eligible_bytes=5 initial_projected_local_bytes=15\n"
            ));
            assert!(!output.contains("budget_satisfied"));
            assert!(!output.contains("archives="));
        }
    }
}
