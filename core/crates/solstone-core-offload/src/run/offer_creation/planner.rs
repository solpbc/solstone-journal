// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Plan additional offers from the original admission and actual current relief.

use chrono::{DateTime, FixedOffset};
use solstone_core_retention::archive_offer::{
    OffloadReconciliation, OffloadReconciliationFailure, OffloadReconciliationReport,
};

use super::*;
use crate::measurement::{ProposedRelief, RawMediaPopulation};
use crate::run::{CurrentOffloadDecision, CurrentOffloadRefusalReason, CurrentOffloadReport};

#[path = "owned_planner.rs"]
mod owning;
pub use owning::{
    OwnedOffloadPlan, OwnedOffloadPlanFinish, OwnedOffloadPlanFinishRefusal, OwnedOffloadPlanHold,
    OwnedOffloadPlanInputFailure, OwnedOffloadPlanInputs, OwnedOffloadPlanOwner,
    SettledOffloadPlan, SettledOffloadPlanReport, create_owned_planned_offload_offers,
    prepare_owned_offload_plan,
};

#[cfg(all(test, unix, feature = "full-tests"))]
thread_local! {
    #[cfg(target_os = "android")]
    static PAYLOAD_CAPACITY_FAULT: Box<std::cell::Cell<bool>> = Box::new(std::cell::Cell::new(false));
    #[cfg(not(target_os = "android"))]
    static PAYLOAD_CAPACITY_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// Private, thread-bound one-shot qualification cut. Exact-selection creation
// never calls this path; every production capacity query remains native.
#[cfg(all(test, unix, feature = "full-tests"))]
pub(in crate::run) fn with_payload_capacity_fault<T>(run: impl FnOnce() -> T) -> (T, bool) {
    PAYLOAD_CAPACITY_FAULT.with(|armed| assert!(!armed.replace(true)));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            PAYLOAD_CAPACITY_FAULT.with(|armed| armed.set(false));
        }
    }
    let _reset = Reset;
    let result = run();
    let consumed = PAYLOAD_CAPACITY_FAULT.with(|armed| !armed.get());
    (result, consumed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OffloadPlanningPhase {
    Held,
    AlreadySatisfied,
    Exhausted,
    OffersPublished,
}

#[derive(Debug)]
pub enum OffloadPlanningRefusal {
    Disabled,
    Reconciliation,
    Local(String),
    Current(CurrentOffloadRefusalReason),
    Unavailable,
}

/// Owning observations only; this report supplies no release or finish authority.
pub struct OffloadPlanning {
    pub(super) phase: OffloadPlanningPhase,
    reconciliation: Option<Result<OffloadReconciliation, OffloadReconciliationFailure>>,
    budget: Option<u64>,
    local: Option<RawMediaPopulation>,
    decision: Option<CurrentOffloadDecision>,
    initial_projection: Option<ProposedRelief>,
}

impl OffloadPlanning {
    fn new() -> Self {
        Self {
            phase: OffloadPlanningPhase::Held,
            reconciliation: None,
            budget: None,
            local: None,
            decision: None,
            initial_projection: None,
        }
    }
    pub fn phase(&self) -> OffloadPlanningPhase {
        self.phase
    }
    pub fn reconciliation(
        &self,
    ) -> Option<&Result<OffloadReconciliation, OffloadReconciliationFailure>> {
        self.reconciliation.as_ref()
    }
    pub fn budget_bytes(&self) -> Option<u64> {
        self.budget
    }
    pub fn local(&self) -> Option<&RawMediaPopulation> {
        self.local.as_ref()
    }
    pub fn decision(&self) -> Option<&CurrentOffloadDecision> {
        self.decision.as_ref()
    }
    /// Projection before any new offers, never a post-publication storage total.
    pub fn initial_projection(&self) -> Option<&ProposedRelief> {
        self.initial_projection.as_ref()
    }

    fn into_report(self) -> Result<OffloadPlanningReport, Box<Self>> {
        if !self.reconciliation.as_ref().is_some_and(Result::is_ok) {
            return Err(Box::new(self));
        }
        let Some(Ok(reconciliation)) = self.reconciliation else {
            unreachable!("checked reconciliation")
        };
        Ok(OffloadPlanningReport {
            phase: self.phase,
            reconciliation: reconciliation.into_report(),
            budget: self.budget,
            local: self.local,
            decision: self.decision.map(CurrentOffloadDecision::into_report),
            initial_projection: self.initial_projection,
        })
    }
}

/// Observed planning facts after actual settlement, retaining no journal handles.
pub struct OffloadPlanningReport {
    phase: OffloadPlanningPhase,
    reconciliation: OffloadReconciliationReport,
    budget: Option<u64>,
    local: Option<RawMediaPopulation>,
    decision: Option<CurrentOffloadReport>,
    initial_projection: Option<ProposedRelief>,
}
impl OffloadPlanningReport {
    pub fn phase(&self) -> OffloadPlanningPhase {
        self.phase
    }
    pub fn reconciliation(&self) -> &OffloadReconciliationReport {
        &self.reconciliation
    }
    pub fn budget_bytes(&self) -> Option<u64> {
        self.budget
    }
    pub fn local(&self) -> Option<&RawMediaPopulation> {
        self.local.as_ref()
    }
    pub fn decision(&self) -> Option<&CurrentOffloadReport> {
        self.decision.as_ref()
    }
    pub fn initial_projection(&self) -> Option<&ProposedRelief> {
        self.initial_projection.as_ref()
    }
}

/// Reconcile, inspect actual pending relief, then create only needed offers.
/// The returned owner still requires its owning settlement boundary.
pub fn create_planned_offload_offers<'clock>(
    operation: PreparedArchiveOperation<'clock>,
    services: &BackupServices<'clock>,
    at: DateTime<FixedOffset>,
) -> OffloadCreation<'clock> {
    #[cfg(any(unix, windows))]
    {
        bound::create(operation, services, at)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _parameters = (services, at);
        OffloadCreation {
            owner: Owner::Prepared(Box::new(operation)),
            resources: Resources {
                planning: Some(OffloadPlanning::new()),
                reason: Some(OffloadCreationRefusal::Inputs(
                    OffloadOfferRefusal::Unsupported,
                )),
                ..Resources::default()
            },
        }
    }
}

#[cfg(any(unix, windows))]
mod bound {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use std::sync::Arc;

    use solstone_core_journal_io::{media::MediaDirectory, paths::SegmentLayout};
    use solstone_core_retention::{
        archive_catalog::read_complete_catalog,
        archive_offer::{capture_offload_offers, reconcile_offload_empty},
        content::{ClosedHandlerSet, JournalMedia},
        grouped::{DecisionPurpose, inspect_grouped},
        marks::RemovalClass,
        receipt::Target,
        review_context::load_at_root,
        scan::scan_segment_bound,
    };

    use super::*;
    use crate::measurement::{measure_local_media, project_current_offload};
    use crate::run::current::{CurrentArchive, CurrentPolicyObservation, MAX_CURRENT_ENTRIES};

    fn planning(error: OffloadPlanningRefusal) -> OffloadCreationRefusal {
        OffloadCreationRefusal::Planning(error)
    }
    fn current(error: CurrentOffloadRefusalReason) -> OffloadCreationRefusal {
        planning(OffloadPlanningRefusal::Current(error))
    }

    // Validate the sole local census before admission, including exact-key uniqueness.
    pub(super) fn complete(local: &RawMediaPopulation) -> Result<(), OffloadCreationRefusal> {
        local
            .usage()
            .map_err(|error| planning(OffloadPlanningRefusal::Local(error)))?;
        let mut keys = BTreeSet::new();
        if local.files.iter().any(|file| !keys.insert(&file.key)) {
            return Err(planning(OffloadPlanningRefusal::Local(
                "local media observation is duplicated".into(),
            )));
        }
        Ok(())
    }

    type GroupKey = (String, String, String, SegmentLayout);

    pub(super) fn payload_capacity(
        operation: &PreparedArchiveOperation<'_>,
        observed: &ObservedOffloadOffers,
    ) -> Result<Arc<MediaDirectory>, OffloadCreationRefusal> {
        #[cfg(all(test, unix, feature = "full-tests"))]
        if PAYLOAD_CAPACITY_FAULT.with(|armed| armed.replace(false)) {
            return solstone_core_journal_io::media::run_with_media_hook(
                solstone_core_journal_io::media::MediaPrimitive::CapacityQuery,
                || Err(std::io::Error::from_raw_os_error(13)),
                || native::capacity(operation, observed),
            )
            .0;
        }
        native::capacity(operation, observed)
    }

    pub(super) fn candidates(
        admitted: CurrentArchive<'_, '_>,
        report: &OffloadPlanning,
        at: DateTime<FixedOffset>,
    ) -> Result<Vec<OffloadOfferSelection>, OffloadCreationRefusal> {
        let local = report.local.as_ref().expect("complete local population");
        let decision = report.decision.as_ref().expect("current inspection");
        let projection = report
            .initial_projection
            .as_ref()
            .expect("current projection");
        let needed = projection
            .projected_local_bytes
            .saturating_sub(report.budget.unwrap_or(u64::MAX));
        let observation = CurrentPolicyObservation::capture(
            admitted.journal_root(),
            admitted
                .current_time()
                .map_err(OffloadCreationRefusal::Archive)?,
        )
        .map_err(current)?;
        let policy = observation.policy();
        let mut groups: BTreeMap<GroupKey, BTreeMap<String, u64>> = BTreeMap::new();
        for file in &local.files {
            groups
                .entry((
                    file.key.day.clone(),
                    file.key.stream.clone(),
                    file.key.segment_dir.clone(),
                    file.key.layout,
                ))
                .or_default()
                .insert(file.key.name.clone(), file.bytes);
        }
        // Intersect the owner's current refs with the measured local population.
        let credited = decision
            .eligible()
            .filter_map(|file| {
                let target = file.target();
                let key = (
                    target.day.clone(),
                    target.stream.clone(),
                    target.dir.clone(),
                    file.layout(),
                );
                (groups.get(&key)?.get(file.name()).copied() == Some(file.bytes()))
                    .then(|| (key, file.name().to_owned()))
            })
            .collect::<BTreeSet<_>>();
        let mut selected = Vec::new();
        let mut additional = 0u64;
        for ((day, stream, dir, layout), names) in groups {
            observation.revalidate(admitted).map_err(current)?;
            let parent = Path::new("chronicle").join(&day);
            let relative = match layout {
                SegmentLayout::Direct => parent.join(&dir),
                SegmentLayout::Named => parent.join(&stream).join(&dir),
            };
            let directory = MediaDirectory::open(Arc::clone(admitted.journal_root()), &relative)
                .map_err(|error| native::inputs(error.into()))?;
            let scan = scan_segment_bound(
                &directory,
                &ClosedHandlerSet,
                &JournalMedia,
                MAX_CURRENT_ENTRIES,
            )
            .map_err(|error| native::inputs(error.into()))?;
            let target = Target { day, stream, dir };
            let inspected = inspect_grouped(
                &scan,
                &target,
                layout,
                &policy,
                at.date_naive(),
                at.with_timezone(&Utc),
                DecisionPurpose::OffloadProposal,
            );
            let key = (
                target.day.clone(),
                target.stream.clone(),
                target.dir.clone(),
                layout,
            );
            let mut increment = 0u64;
            let mut selected_names = Vec::new();
            for file in inspected.eligible() {
                if names.get(file.name()).copied() != Some(file.bytes()) {
                    return Err(native::inputs(OffloadOfferRefusal::Changed));
                }
                if !credited.contains(&(key.clone(), file.name().to_owned())) {
                    increment = increment
                        .checked_add(file.bytes())
                        .ok_or(OffloadCreationRefusal::Metadata)?;
                }
                // Keep P siblings: partition/capture owns whole existing Mark coverage.
                selected_names.push(file.name().to_owned());
            }
            if increment == 0 {
                continue;
            }
            selected_names.sort();
            selected.push(OffloadOfferSelection {
                target,
                layout,
                names: selected_names,
            });
            additional = additional
                .checked_add(increment)
                .ok_or(OffloadCreationRefusal::Metadata)?;
            if additional >= needed {
                break;
            }
        }
        observation.revalidate(admitted).map_err(current)?;
        admitted
            .revalidate()
            .map_err(OffloadCreationRefusal::Archive)?;
        Ok(selected)
    }

    pub(super) fn preflight(
        operation: &PreparedArchiveOperation<'_>,
        report: &mut OffloadPlanning,
        at: DateTime<FixedOffset>,
    ) -> Result<(), OffloadCreationRefusal> {
        let today = at.date_naive();
        let now = at.with_timezone(&Utc);
        operation
            .revalidate()
            .map_err(|error| native::inputs(OffloadOfferRefusal::Admission(error)))?;
        // A paused backup plans no offload, so no local media is released.
        if !operation.configuration().offload_enabled() || operation.configuration().paused() {
            return Err(planning(OffloadPlanningRefusal::Disabled));
        }
        report.reconciliation = Some(reconcile_offload_empty(operation, today, now));
        if !report.reconciliation.as_ref().is_some_and(Result::is_ok) {
            return Err(planning(OffloadPlanningRefusal::Reconciliation));
        }
        report.budget = operation
            .configuration()
            .offload_budget_bytes()
            .map_err(|error| native::inputs(OffloadOfferRefusal::Admission(error)))?;
        report.local = Some(measure_local_media(
            operation.journal_root().canonical_path(),
        ));
        complete(report.local.as_ref().expect("local measurement"))?;
        native::capacity_for_payload(operation, 0)?;
        Ok(())
    }

    pub(super) fn inspect_plan(
        admitted: CurrentArchive<'_, '_>,
        report: &mut OffloadPlanning,
        at: DateTime<FixedOffset>,
    ) -> Result<Option<Vec<OffloadOfferSelection>>, OffloadCreationRefusal> {
        let register = load_at_root(admitted.journal_root())
            .map_err(|error| native::inputs(OffloadOfferRefusal::Review(error)))?;
        let expected = register
            .marks
            .values()
            .filter(|mark| mark.class == RemovalClass::OffloadRawRelease)
            .cloned()
            .collect::<Vec<_>>();
        report.decision = Some(crate::run::current::inspect(admitted, &expected, at));
        report.initial_projection = project_current_offload(
            report.local.as_ref().expect("complete local population"),
            report.decision.as_ref().expect("current inspection"),
        )
        .map_err(|error| planning(OffloadPlanningRefusal::Local(error)))?;
        let projection = report
            .initial_projection
            .as_ref()
            .ok_or_else(|| planning(OffloadPlanningRefusal::Unavailable))?;
        if measure_local_media(admitted.journal_root().canonical_path())
            != *report.local.as_ref().expect("complete local population")
        {
            return Err(native::inputs(OffloadOfferRefusal::Changed));
        }
        admitted
            .revalidate()
            .map_err(OffloadCreationRefusal::Archive)?;
        if report
            .budget
            .is_none_or(|budget| projection.projected_local_bytes <= budget)
        {
            return Ok(None);
        }
        let selected = candidates(admitted, report, at)?;
        Ok(Some(selected))
    }

    pub(super) fn create<'clock>(
        operation: PreparedArchiveOperation<'clock>,
        services: &BackupServices<'clock>,
        at: DateTime<FixedOffset>,
    ) -> OffloadCreation<'clock> {
        let today = at.date_naive();
        let now = at.with_timezone(&Utc);
        let mut report = OffloadPlanning::new();
        let mut resources = Resources::default();
        let prepared = preflight(&operation, &mut report, at);
        if let Err(reason) = prepared {
            resources.planning = Some(report);
            return native::fail(Owner::Prepared(Box::new(operation)), resources, reason);
        }
        let admitted =
            match solstone_core_backup_runtime::admit_prepared_archive(operation, services) {
                Ok(admitted) => admitted,
                Err(failure) => {
                    let reason = OffloadCreationRefusal::Archive(failure.error());
                    resources.planning = Some(report);
                    return native::fail(
                        Owner::AdmissionFailure(Box::new(failure)),
                        resources,
                        reason,
                    );
                }
            };
        let inspected = inspect_plan(CurrentArchive::Generic(&admitted), &mut report, at);
        let selections = match inspected {
            Err(reason) => {
                resources.planning = Some(report);
                return native::fail(Owner::Admitted(Box::new(admitted)), resources, reason);
            }
            Ok(None) => {
                report.phase = OffloadPlanningPhase::AlreadySatisfied;
                resources.planning = Some(report);
                return metadata_read(admitted, resources);
            }
            Ok(Some(selections)) if selections.is_empty() => {
                report.phase = OffloadPlanningPhase::Exhausted;
                resources.planning = Some(report);
                return metadata_read(admitted, resources);
            }
            Ok(Some(selections)) => selections,
        };
        resources.planning = Some(report);
        let captured = (|| {
            let catalog = read_complete_catalog(admitted.journal_root())
                .map_err(|error| native::inputs(OffloadOfferRefusal::Catalog(error)))?;
            let register = load_at_root(admitted.journal_root())
                .map_err(|error| native::inputs(OffloadOfferRefusal::Review(error)))?;
            let selections = native::partition(&catalog, &register, &selections)?;
            let operation = admitted.prepared_operation();
            resources.observed = Some(
                capture_offload_offers(operation, &selections, today, now)
                    .map_err(native::inputs)?,
            );
            let parent = payload_capacity(
                operation,
                resources.observed.as_ref().expect("captured inputs"),
            )?;
            native::copy_inputs(operation, parent, &mut resources)?;
            Ok(catalog)
        })();
        match captured {
            Ok(catalog) => native::continue_creation(admitted, resources, catalog, today, now),
            Err(reason) => native::fail(Owner::Admitted(Box::new(admitted)), resources, reason),
        }
    }

    fn metadata_read<'clock>(
        admitted: AdmittedArchive<'clock>,
        resources: Resources,
    ) -> OffloadCreation<'clock> {
        match admitted.into_read_operation() {
            Ok(read) => OffloadCreation {
                owner: Owner::Read(Box::new(read)),
                resources,
            },
            Err(failure) => {
                let reason = OffloadCreationRefusal::Readback(failure.reason());
                native::fail(
                    Owner::ReadAdmissionFailure(Box::new(failure)),
                    resources,
                    reason,
                )
            }
        }
    }
}
